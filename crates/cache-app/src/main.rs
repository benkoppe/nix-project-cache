use anyhow::Context as _;
use tracing::info;
use tracing_subscriber::EnvFilter;

use cache_app::{AppConfig, build_app};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("cache_app=info,cache_read=info")),
        )
        .init();

    let config = AppConfig::from_env().context("loading app config")?;
    let app = build_app(&config).await?;

    let listener = tokio::net::TcpListener::bind(&config.bind_address)
        .await
        .with_context(|| format!("binding TCP listener to {}", config.bind_address))?;

    info!(
        db_path = %config.db_path.display(),
        local_object_root = %config.local_object_root.display(),
        address = %listener.local_addr()?,
        "starting cache server"
    );

    axum::serve(listener, app).await.context("serving HTTP")?;

    Ok(())
}
