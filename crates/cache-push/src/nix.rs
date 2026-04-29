use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;

use anyhow::{Context as _, Result, bail};
use async_compression::tokio::bufread::ZstdEncoder;
use bytes::Bytes;
use tokio::fs;
use tokio::io::{AsyncRead, AsyncReadExt as _, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::io::StreamReader;

use cache_core::narinfo::NarInfo;
use cache_core::nix::{PathInfo, parse_path_info_json};

pub type BoxAsyncRead = Pin<Box<dyn AsyncRead + Send>>;

const DEFAULT_STORE_DIR: &str = "/nix/store";
const MAX_SYMLINK_DEPTH: usize = 255;
const STREAM_CHUNK_SIZE: usize = 64 * 1024;

pub async fn get_store_dir() -> Result<String> {
    if let Ok(store_dir) = std::env::var("NIX_STORE_DIR")
        && !store_dir.trim().is_empty()
    {
        return Ok(store_dir);
    }

    let output = Command::new("nix")
        .args([
            "--extra-experimental-features",
            "nix-command",
            "eval",
            "--raw",
            "--expr",
            "builtins.storeDir",
        ])
        .output()
        .await
        .context("running nix eval for builtins.storeDir")?;

    if output.status.success() {
        let store_dir = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if !store_dir.is_empty() {
            return Ok(store_dir);
        }
    }

    Ok(DEFAULT_STORE_DIR.to_owned())
}

pub async fn resolve_symlinks(paths: &[String], store_dir: &str) -> Result<Vec<String>> {
    let store_dir_path = Path::new(store_dir);
    let mut resolved = Vec::with_capacity(paths.len());

    for input in paths {
        let mut current = PathBuf::from(input);

        for depth in 0..=MAX_SYMLINK_DEPTH {
            if current.starts_with(store_dir_path) {
                break;
            }

            let metadata = fs::symlink_metadata(current.clone())
                .await
                .with_context(|| format!("reading metadata for {}", current.display()))?;

            if !metadata.file_type().is_symlink() {
                break;
            }

            if depth == MAX_SYMLINK_DEPTH {
                bail!("too many symlinks resolving {}", input);
            }

            let target = fs::read_link(current.clone())
                .await
                .with_context(|| format!("reading symlink {}", current.display()))?;

            let next = if target.is_absolute() {
                target
            } else {
                let parent = current.parent().unwrap_or_else(|| Path::new("."));
                parent.join(target)
            };

            current = next;
        }

        resolved.push(current.to_string_lossy().into_owned());
    }

    Ok(resolved)
}

pub async fn get_path_infos_recursive(paths: &[String]) -> Result<BTreeMap<String, PathInfo>> {
    let mut command = Command::new("nix");
    command.args([
        "--extra-experimental-features",
        "nix-command",
        "path-info",
        "--recursive",
        "--json",
        "--",
    ]);
    command.args(paths);

    let output = command
        .output()
        .await
        .context("running nix path-info --recursive --json")?;

    if !output.status.success() {
        bail!(
            "nix path-info failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    parse_path_info_json(&output.stdout).context("parsing nix path-info json")
}

pub fn narinfos_from_path_infos(path_infos: &BTreeMap<String, PathInfo>) -> Result<Vec<NarInfo>> {
    path_infos.values().map(narinfo_from_path_info).collect()
}

pub fn narinfo_from_path_info(path_info: &PathInfo) -> Result<NarInfo> {
    let normalized = path_info
        .nar_hash
        .normalize()
        .context("normalizing nar hash")?;

    Ok(NarInfo {
        store_path: path_info.path.clone(),
        url: format!("nar/{}.nar.zst", normalized.digest()),
        compression: "zstd".to_owned(),
        nar_hash: path_info.nar_hash.clone(),
        nar_size: path_info.nar_size,
        references: path_info.references.clone(),
        deriver: path_info.deriver.clone(),
        signatures: path_info.signatures.clone(),
        ca: path_info.ca.clone(),
    })
}

pub async fn compressed_nar_reader_for_path(path: &str) -> Result<BoxAsyncRead> {
    let mut child = Command::new("nix-store")
        .arg("--dump")
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("spawning nix-store --dump for {}", path))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("nix-store --dump did not provide stdout"))?;

    let (sender, receiver) = mpsc::channel::<Result<Bytes, io::Error>>(4);
    let path_text = path.to_owned();

    tokio::spawn(async move {
        let mut encoder = ZstdEncoder::new(BufReader::new(stdout));
        let mut buffer = vec![0_u8; STREAM_CHUNK_SIZE];

        loop {
            match encoder.read(&mut buffer).await {
                Ok(0) => break,
                Ok(read) => {
                    if sender
                        .send(Ok(Bytes::copy_from_slice(&buffer[..read])))
                        .await
                        .is_err()
                    {
                        let _ = child.kill().await;
                        return;
                    }
                }
                Err(error) => {
                    let _ = sender.send(Err(error)).await;
                    let _ = child.kill().await;
                    return;
                }
            }
        }

        match child.wait().await {
            Ok(status) if status.success() => {}
            Ok(status) => {
                let _ = sender
                    .send(Err(io::Error::other(format!(
                        "nix-store --dump exited with status {} for {}",
                        status, path_text
                    ))))
                    .await;
            }
            Err(error) => {
                let _ = sender
                    .send(Err(io::Error::other(format!(
                        "waiting for nix-store --dump for {}: {}",
                        path_text, error
                    ))))
                    .await;
            }
        }
    });

    Ok(Box::pin(StreamReader::new(ReceiverStream::new(receiver))))
}

#[cfg(test)]
mod tests {
    use cache_core::nix::NixHash;

    use super::*;

    #[test]
    fn narinfo_from_path_info_uses_normalized_nar_hash_digest_for_url() {
        let path_info = PathInfo {
            path: "/nix/store/26xbg1ndr7hbcncrlf9nhx5is2b25d13-hello-2.12.1".to_owned(),
            nar_hash: NixHash::Raw(
                "sha256-n4bQgYhMfWWaL+qgxVrQFaO/TxsrC4Is0V1sFbDwCgg=".to_owned(),
            ),
            nar_size: 226560,
            references: vec![],
            deriver: None,
            signatures: vec![],
            ca: None,
        };

        let narinfo = narinfo_from_path_info(&path_info).unwrap();

        assert_eq!(
            narinfo.url,
            "nar/020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz.nar.zst"
        );
        assert_eq!(narinfo.compression, "zstd");
        assert_eq!(narinfo.store_path, path_info.path);
    }
}
