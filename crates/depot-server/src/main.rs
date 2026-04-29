use std::path::PathBuf;

use anyhow::Context as _;
use clap::Parser;
use tracing::info;
use tracing_subscriber::EnvFilter;

use depot_server::{AppConfig, build_app};

#[derive(Debug, Parser)]
#[command(name = "depot-server")]
#[command(about = "Run the repo-depot server")]
struct Cli {
    #[arg(long, env = "DEPOT_SERVER_CONFIG")]
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
        mode = ?config.server.mode,
        default_storage = %config.storage.default_storage_id,
        storage_backends = config.storage.backends.len(),
        address = %listener.local_addr()?,
        "starting depot server"
    );

    axum::serve(listener, app).await.context("serving HTTP")?;

    Ok(())
}
