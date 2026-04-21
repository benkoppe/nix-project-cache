mod cli;
mod git;
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

    let client = CacheClient::new(&cli.server, cli.auth_token)
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
