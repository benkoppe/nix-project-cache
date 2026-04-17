use std::sync::Arc;

use anyhow::Context as _;
use tracing::info;
use tracing_subscriber::EnvFilter;

use cache_core::narinfo::NarInfoRenderer;
use cache_core::nix::StoreDir;
use cache_core::signing::NarInfoSigner;
use cache_read::{AppState, InMemoryNarInfoResolver, ReadService, router};
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

    let store_dir = StoreDir::default();
    let renderer = NarInfoRenderer::new(store_dir.clone());
    let signer = NarInfoSigner::new(store_dir, Vec::new());

    let read_service = ReadService::new(
        Arc::new(InMemoryNarInfoResolver::new()),
        Arc::new(InMemoryLocalObjectStore::new()),
        Arc::new(ReqwestUpstreamCacheClient::default()),
        Vec::new(),
        renderer,
        signer,
    );

    let state = AppState::new(Arc::new(read_service), 30);
    let app = router(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:8080")
        .await
        .context("binding TCP listener")?;

    info!(address = %listener.local_addr()?, "starting cache read server");

    axum::serve(listener, app).await.context("serving HTTP")?;

    Ok(())
}
