use std::io::Write;

use anyhow::{Context as _, Result, bail};

use cache_core::signing::NamedSigningKey;

use crate::cli::{GenerateKeyCommand, KeysCommand};
use crate::output;

pub async fn handle(
    writer: &mut impl Write,
    json_output: bool,
    command: KeysCommand,
) -> Result<()> {
    match command {
        KeysCommand::Generate(command) => generate_key(writer, json_output, command).await,
    }
}

async fn generate_key(
    writer: &mut impl Write,
    json_output: bool,
    command: GenerateKeyCommand,
) -> Result<()> {
    if command.name.trim().is_empty() {
        bail!("--name must not be empty");
    }

    let key = NamedSigningKey::generate(&command.name)
        .map_err(anyhow::Error::new)
        .context("generating signing key")?;

    let secret_text = key.private_key_text();
    let public_text = key.public_key_text();

    std::fs::write(&command.secret_file, format!("{secret_text}\n"))
        .with_context(|| format!("writing {}", command.secret_file.display()))?;
    std::fs::write(&command.public_file, format!("{public_text}\n"))
        .with_context(|| format!("writing {}", command.public_file.display()))?;

    if json_output {
        output::print_status_json(
            writer,
            "generated",
            [
                ("name", serde_json::json!(command.name)),
                (
                    "secret_file",
                    serde_json::json!(command.secret_file.display().to_string()),
                ),
                (
                    "public_file",
                    serde_json::json!(command.public_file.display().to_string()),
                ),
                ("public_key", serde_json::json!(public_text)),
            ],
        )?;
    } else {
        writeln!(writer, "generated signing key {}", command.name)?;
        writeln!(writer, "secret_file={}", command.secret_file.display())?;
        writeln!(writer, "public_file={}", command.public_file.display())?;
        writeln!(writer, "public_key={public_text}")?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use tempfile::tempdir;

    use cache_core::signing::NamedSigningKey;

    use super::*;

    fn nix_store_available() -> bool {
        Command::new("nix-store")
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    #[tokio::test]
    async fn generated_key_files_are_parseable_and_match_public_key() {
        let temp_dir = tempdir().unwrap();
        let secret_file = temp_dir.path().join("cache.sec");
        let public_file = temp_dir.path().join("cache.pub");

        let mut output = Vec::new();

        generate_key(
            &mut output,
            false,
            GenerateKeyCommand {
                name: "cache.example.com-1".to_owned(),
                secret_file: secret_file.clone(),
                public_file: public_file.clone(),
            },
        )
        .await
        .unwrap();

        let secret = std::fs::read_to_string(&secret_file).unwrap();
        let public = std::fs::read_to_string(&public_file).unwrap();

        let key = NamedSigningKey::parse(secret.trim()).unwrap();
        assert_eq!(key.public_key_text(), public.trim());
    }

    #[tokio::test]
    async fn generated_key_format_matches_nix_store_generated_format() {
        if !nix_store_available() {
            eprintln!("skipping nix-store key format compatibility test");
            return;
        }

        let temp_dir = tempdir().unwrap();

        let our_secret = temp_dir.path().join("our.sec");
        let our_public = temp_dir.path().join("our.pub");
        let nix_secret = temp_dir.path().join("nix.sec");
        let nix_public = temp_dir.path().join("nix.pub");

        let mut output = Vec::new();

        generate_key(
            &mut output,
            false,
            GenerateKeyCommand {
                name: "cache.example.com-1".to_owned(),
                secret_file: our_secret.clone(),
                public_file: our_public.clone(),
            },
        )
        .await
        .unwrap();

        let status = Command::new("nix-store")
            .args(["--generate-binary-cache-key", "cache.example.com-1"])
            .arg(&nix_secret)
            .arg(&nix_public)
            .status()
            .unwrap();

        assert!(status.success());

        let our_secret_key =
            NamedSigningKey::parse(std::fs::read_to_string(&our_secret).unwrap().trim()).unwrap();
        let nix_secret_key =
            NamedSigningKey::parse(std::fs::read_to_string(&nix_secret).unwrap().trim()).unwrap();

        assert_eq!(
            our_secret_key.public_key_text(),
            std::fs::read_to_string(&our_public).unwrap().trim()
        );
        assert_eq!(
            nix_secret_key.public_key_text(),
            std::fs::read_to_string(&nix_public).unwrap().trim()
        );
    }
}
