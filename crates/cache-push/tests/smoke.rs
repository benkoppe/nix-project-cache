use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use reqwest::StatusCode;
use tokio::fs;
use tokio::process::Command;

use cache_app::build_app_with_parts;
use cache_core::nix::parse_path_info_json;
use cache_store::upstream::ReqwestUpstreamCacheClient;
use cache_test_utils::{
    EXAMPLE_PROJECT_NAME, TestDatabase, TestServer, example_project, filesystem_backends_in,
    test_signing_keys,
};

const WRITE_TOKEN: &str = "secret-token";

async fn command_available(command: &str) -> bool {
    match Command::new(command).arg("--version").output().await {
        Ok(output) => output.status.success(),
        Err(_) => false,
    }
}

async fn skip_unless_nix_available() -> Result<()> {
    if !command_available("nix").await || !command_available("nix-store").await {
        eprintln!("skipping cache-push smoke test because nix/nix-store is unavailable");
        return Err(anyhow::anyhow!("skip"));
    }

    Ok(())
}

async fn add_fixed_store_path(path: &std::path::Path) -> Result<String> {
    let output = Command::new("nix-store")
        .args(["--add-fixed", "--recursive", "sha256"])
        .arg(path)
        .output()
        .await
        .context("running nix-store --add-fixed")?;

    if !output.status.success() {
        bail!(
            "nix-store --add-fixed failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

async fn path_info_for(store_path: &str) -> Result<cache_core::nix::PathInfo> {
    let output = Command::new("nix")
        .args([
            "--extra-experimental-features",
            "nix-command",
            "path-info",
            "--json",
            "--",
        ])
        .arg(store_path)
        .output()
        .await
        .with_context(|| format!("running nix path-info for {}", store_path))?;

    if !output.status.success() {
        bail!(
            "nix path-info failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let mut infos =
        parse_path_info_json(&output.stdout).context("parsing nix path-info json in smoke test")?;

    infos
        .remove(store_path)
        .ok_or_else(|| anyhow::anyhow!("missing path info for {}", store_path))
}

async fn expected_nar_dump_bytes(store_path: &str) -> Result<Vec<u8>> {
    let output = Command::new("nix-store")
        .args(["--dump", store_path])
        .output()
        .await
        .with_context(|| format!("running nix-store --dump for {}", store_path))?;

    if !output.status.success() {
        bail!(
            "nix-store --dump failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(output.stdout)
}

#[tokio::test]
async fn cache_push_can_publish_and_read_back_path() -> Result<()> {
    if skip_unless_nix_available().await.is_err() {
        return Ok(());
    }

    let fixture = TestDatabase::new().await?;
    let project = example_project();
    fixture.insert_example_project().await?;

    let app = build_app_with_parts(
        fixture.db.clone(),
        cache_core::nix::StoreDir::default(),
        test_signing_keys(),
        filesystem_backends_in(&fixture.temp_dir),
        Some(cache_core::storage::LocalBackendName::fs()),
        Some(WRITE_TOKEN.to_owned()),
        Arc::new(ReqwestUpstreamCacheClient::default()),
    );
    let server = TestServer::spawn(app).await?;

    let input_path = fixture.temp_dir.path().join("hello.txt");
    fs::write(&input_path, b"hello from cache-push smoke test")
        .await
        .context("writing smoke-test input file")?;

    let store_path = add_fixed_store_path(&input_path).await?;
    let path_info = path_info_for(&store_path).await?;
    let store_path_hash = path_info
        .store_path_hash()
        .context("deriving store path hash in smoke test")?;
    let object_path = format!("nar/{}.nar.zst", path_info.nar_hash.normalize()?.digest());
    let expected_nar_dump = expected_nar_dump_bytes(&store_path).await?;

    let binary = env!("CARGO_BIN_EXE_cache-push");
    let output = Command::new(binary)
        .args([
            "--server",
            &server.base_url,
            "--auth-token",
            WRITE_TOKEN,
            "--project",
            project.as_str(),
            "--ref",
            "refs/heads/main",
            "--revision",
            "deadbeef",
            "--max-concurrent-uploads",
            "1",
            &store_path,
        ])
        .output()
        .await
        .context("running cache-push smoke test binary")?;

    if !output.status.success() {
        bail!(
            "cache-push failed\nstdout:\n{}\n\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let read_client = reqwest::Client::new();

    let narinfo_response = read_client
        .get(server.url(format!("{}.narinfo", store_path_hash.as_str())))
        .send()
        .await
        .context("fetching aggregate narinfo after cache-push")?;

    assert_eq!(narinfo_response.status(), StatusCode::OK);
    assert_eq!(
        narinfo_response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .unwrap(),
        "text/x-nix-narinfo"
    );

    let narinfo_body = narinfo_response.text().await?;
    assert!(narinfo_body.contains(&format!("StorePath: {}", store_path)));
    assert!(narinfo_body.contains(&format!("URL: {}", object_path)));
    assert!(narinfo_body.contains("Sig: cache.example.com-1:"));

    let object_response = read_client
        .get(server.url(&object_path))
        .send()
        .await
        .context("fetching aggregate nar object after cache-push")?;

    assert_eq!(object_response.status(), StatusCode::OK);
    assert_eq!(
        object_response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .unwrap(),
        "application/octet-stream"
    );

    let object_bytes = object_response.bytes().await?;
    let decoded_object_bytes =
        zstd::stream::decode_all(object_bytes.as_ref()).context("decoding uploaded nar object")?;
    assert_eq!(decoded_object_bytes, expected_nar_dump);

    let project_narinfo_response = read_client
        .get(server.url(format!(
            "p/{}/{}.narinfo",
            project.as_str(),
            store_path_hash.as_str()
        )))
        .send()
        .await
        .context("fetching project narinfo after cache-push")?;

    assert_eq!(project_narinfo_response.status(), StatusCode::OK);

    let project_object_response = read_client
        .get(server.url(format!("p/{}/{}", project.as_str(), object_path)))
        .send()
        .await
        .context("fetching project nar object after cache-push")?;

    assert_eq!(project_object_response.status(), StatusCode::OK);

    let _ = EXAMPLE_PROJECT_NAME;

    Ok(())
}
