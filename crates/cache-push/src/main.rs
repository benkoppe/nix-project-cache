mod cli;
mod git;
mod github_oidc;
mod nix;
mod push;

use anyhow::Context as _;
use clap::Parser as _;
use tracing_subscriber::EnvFilter;

use cache_client::CacheClient;
use cache_core::project::ProjectSlug;

use crate::cli::Cli;
use crate::push::{PushOptions, push_paths};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("cache_push=info,cache_client=info")),
        )
        .init();

    let cli = Cli::parse();
    cli.validate()?;

    let project = ProjectSlug::parse(&cli.project)
        .map_err(|_| anyhow::anyhow!("invalid project slug {}", cli.project))?;

    let ref_name = git::resolve_ref(cli.ref_name.as_deref()).await?;
    let revision = git::resolve_revision(cli.revision.as_deref()).await?;
    let auth_token = resolve_auth_token(&cli).await?;

    let client = CacheClient::new(&cli.server, auth_token)
        .with_context(|| format!("creating client for {}", cli.server))?;

    let options = PushOptions {
        project,
        ref_name,
        revision,
        pin: cli.pin,
        max_concurrent_uploads: cli.max_concurrent_uploads.max(1),
        paths: cli.paths,
    };

    push_paths(&client, options).await
}

async fn resolve_auth_token(cli: &Cli) -> anyhow::Result<String> {
    if let Some(token) = cli
        .auth_token
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        return Ok(token.to_owned());
    }

    let audience = cli
        .oidc_audience
        .as_deref()
        .map(str::trim)
        .filter(|audience| !audience.is_empty())
        .unwrap_or(&cli.server);

    github_oidc::request_github_actions_oidc_token(audience).await
}
