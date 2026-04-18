mod config;

use std::sync::Arc;

use anyhow::Context as _;
use tracing::info;
use tracing_subscriber::EnvFilter;

use cache_core::narinfo::NarInfoRenderer;
use cache_core::signing::NarInfoSigner;
use cache_db::SqliteDatabase;
use cache_read::{
    AppState, DbBackedLocalObjectStore, DbNarInfoResolver, DbUpstreamSelector, ReadService, router,
};
use cache_store::upstream::ReqwestUpstreamCacheClient;

use crate::config::AppConfig;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("cache_app=info,cache_read=info")),
        )
        .init();

    let config = AppConfig::from_env().context("loading app config")?;
    let db = SqliteDatabase::open(&config.db_path)
        .await
        .context("opening sqlite metadata database")?;

    let renderer = NarInfoRenderer::new(config.store_dir.clone());
    let signer = NarInfoSigner::new(config.store_dir.clone(), config.signing_keys.clone());

    let local_objects = DbBackedLocalObjectStore::new(db.clone(), config.local_object_backends());
    let upstream_selector = DbUpstreamSelector::new(db.clone());

    let read_service = ReadService::new(
        Arc::new(DbNarInfoResolver::new(db)),
        Arc::new(local_objects),
        Arc::new(ReqwestUpstreamCacheClient::default()),
        Arc::new(upstream_selector),
        renderer,
        signer,
    );

    let state = AppState::new(Arc::new(read_service), 30);
    let app = router(state);

    let listener = tokio::net::TcpListener::bind(&config.bind_address)
        .await
        .with_context(|| format!("binding TCP listener to {}", config.bind_address))?;

    info!(
        db_path = %config.db_path.display(),
        local_object_root = %config.local_object_root.display(),
        address = %listener.local_addr()?,
        "starting cache read server"
    );

    axum::serve(listener, app).await.context("serving HTTP")?;

    Ok(())
}
