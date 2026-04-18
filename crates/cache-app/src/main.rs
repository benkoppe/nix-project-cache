use std::sync::Arc;

use anyhow::Context as _;
use tracing::info;
use tracing_subscriber::EnvFilter;

use cache_core::narinfo::NarInfoRenderer;
use cache_core::nix::StoreDir;
use cache_core::signing::NarInfoSigner;
use cache_db::SqliteDatabase;
use cache_read::{AppState, DbNarInfoResolver, ReadService, router};
use cache_store::local::InMemoryLocalObjectStore;
use cache_store::upstream::ReqwestUpstreamCacheClient;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("cache_app=info,cache_read=info")),
        )
        .init();

    let db_path = std::env::var("CACHE_DB_PATH").unwrap_or_else(|_| "./cache_db".to_owned());
    let db = SqliteDatabase::open(&db_path)
        .await
        .context("opening sqlite metadata database")?;

    let store_dir = StoreDir::default();
    let renderer = NarInfoRenderer::new(store_dir.clone());
    let signer = NarInfoSigner::new(store_dir, Vec::new());

    let upstreams = db
        .list_enabled_upstreams()
        .await
        .context("loading enabled upstream caches")?;

    let read_service = ReadService::new(
        Arc::new(DbNarInfoResolver::new(db)),
        Arc::new(InMemoryLocalObjectStore::new()),
        Arc::new(ReqwestUpstreamCacheClient::default()),
        upstreams,
        renderer,
        signer,
    );

    let state = AppState::new(Arc::new(read_service), 30);
    let app = router(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:8080")
        .await
        .context("binding TCP listener")?;

    info!(
        db_path = %db_path,
        address = %listener.local_addr()?,
        "starting cache read server"
    );

    axum::serve(listener, app).await.context("serving HTTP")?;

    Ok(())
}
