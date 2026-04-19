mod config;

use std::sync::Arc;

use anyhow::Context as _;
use tracing::info;
use tracing_subscriber::EnvFilter;

use cache_auth::StaticTokenAuthorizer;
use cache_core::narinfo::NarInfoRenderer;
use cache_core::signing::NarInfoSigner;
use cache_db::SqliteDatabase;
use cache_ingest::IngestService;
use cache_read::{
    DbBackedLocalObjectStore, DbNarInfoResolver, DbUpstreamSelector, ReadAppState, ReadService,
    read_router,
};
use cache_store::upstream::ReqwestUpstreamCacheClient;
use cache_write::{WriteAppState, write_router};

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

    let local_backends = config.local_object_backends();

    let local_objects = DbBackedLocalObjectStore::new(db.clone(), config.local_object_backends());
    let upstream_selector = DbUpstreamSelector::new(db.clone());
    let upstream_client = Arc::new(ReqwestUpstreamCacheClient::default());

    let read_service = ReadService::new(
        Arc::new(DbNarInfoResolver::new(db.clone())),
        Arc::new(local_objects.clone()),
        upstream_client.clone(),
        Arc::new(upstream_selector),
        renderer,
        signer,
    );

    let ingest_service = IngestService::new(
        db.clone(),
        Arc::new(local_objects),
        local_backends,
        upstream_client,
    );

    let read_state = ReadAppState::new(Arc::new(read_service), 30);
    let write_state = WriteAppState::new(
        Arc::new(ingest_service),
        Arc::new(StaticTokenAuthorizer::new(config.write_token.clone())),
    );

    let app = read_router(read_state).merge(write_router(write_state));

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
