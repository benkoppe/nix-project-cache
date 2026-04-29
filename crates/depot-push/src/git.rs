use anyhow::{Context as _, Result, bail};
use tokio::process::Command;

pub async fn resolve_ref(cli_ref: Option<&str>) -> Result<String> {
    if let Some(value) = cli_ref {
        return Ok(value.to_owned());
    }

    if let Ok(value) = std::env::var("GITHUB_REF")
        && !value.trim().is_empty()
    {
        return Ok(value);
    }

    let output = run_git(["symbolic-ref", "-q", "HEAD"]).await?;
    let trimmed = output.trim();
    if !trimmed.is_empty() {
        return Ok(trimmed.to_owned());
    }

    bail!("could not determine git ref; pass --ref or set GITHUB_REF")
}

pub async fn resolve_revision(cli_revision: Option<&str>) -> Result<String> {
    if let Some(value) = cli_revision {
        return Ok(value.to_owned());
    }

    if let Ok(value) = std::env::var("GITHUB_SHA")
        && !value.trim().is_empty()
    {
        return Ok(value);
    }

    let output = run_git(["rev-parse", "HEAD"]).await?;
    let trimmed = output.trim();
    if !trimmed.is_empty() {
        return Ok(trimmed.to_owned());
    }

    bail!("could not determine git revision; pass --revision or set GITHUB_SHA")
}

async fn run_git<const N: usize>(args: [&str; N]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .await
        .with_context(|| format!("running git {}", args.join(" ")))?;

    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
