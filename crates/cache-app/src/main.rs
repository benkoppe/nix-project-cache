use std::path::PathBuf;

use anyhow::Context as _;
use clap::Parser;
use tracing::info;
use tracing_subscriber::EnvFilter;

use cache_app::{AppConfig, build_app};

#[derive(Debug, Parser)]
#[command(name = "cache-app")]
#[command(about = "Run the nix-project-cache server")]
struct Cli {
    #[arg(long, env = "CACHE_APP_CONFIG")]
    config: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let config = AppConfig::load(cli.config.as_deref()).context("loading app config")?;

    let filter = EnvFilter::try_new(&config.logging.filter)
        .with_context(|| format!("parsing logging.filter {:?}", config.logging.filter))?;

    tracing_subscriber::fmt().with_env_filter(filter).init();

    let app = build_app(&config).await?;

    let listener = tokio::net::TcpListener::bind(&config.server.bind_address)
        .await
        .with_context(|| format!("binding TCP listener to {}", config.server.bind_address))?;

    info!(
        db_path = %config.database.path.display(),
        local_object_root = %config.storage.object_root.display(),
        writable_backend = ?config.storage.writable_backend.as_ref().map(|name| name.as_str()),
        address = %listener.local_addr()?,
        "starting cache server"
    );

    axum::serve(listener, app).await.context("serving HTTP")?;

    Ok(())
}
