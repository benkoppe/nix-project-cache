pub mod config;

use std::sync::Arc;

use anyhow::Context as _;
use axum::Router;

use cache_auth::{
    Authorizer, ChainAuthorizer, OidcAuthorizer, OidcConfig, ReqwestOidcHttpClient,
    StaticTokenAuthorizer,
};
use cache_core::nix::StoreDir;
use cache_core::signing::{NamedSigningKey, NarInfoSigner};
use cache_core::storage::LocalBackendName;
use cache_db::SqliteDatabase;
use cache_ingest::{GcService, IngestService};
use cache_read::{
    DbBackedLocalObjectStore, DbNarInfoResolver, DbUpstreamSelector, ReadAppState, ReadService,
    read_router,
};
use cache_store::local::LocalObjectBackendRegistry;
use cache_store::upstream::{ReqwestUpstreamCacheClient, UpstreamCacheClient};
use cache_write::{AuthorizationService, WriteAppState, write_router};

pub use config::AppConfig;

pub async fn build_app(config: &AppConfig) -> anyhow::Result<Router> {
    let db = SqliteDatabase::open(&config.db_path)
        .await
        .context("opening sqlite metadata database")?;

    let authorizer = build_default_authorizer(config)?;

    Ok(build_app_with_authorizer(
        db,
        config.store_dir.clone(),
        config.signing_keys.clone(),
        config.local_object_backends(),
        config.writable_local_backend.clone(),
        authorizer,
        Arc::new(ReqwestUpstreamCacheClient::default()),
    ))
}

pub fn build_app_with_parts(
    db: SqliteDatabase,
    store_dir: StoreDir,
    signing_keys: Vec<NamedSigningKey>,
    local_backends: LocalObjectBackendRegistry,
    writable_local_backend: Option<LocalBackendName>,
    write_token: Option<String>,
    upstream_client: Arc<dyn UpstreamCacheClient>,
) -> Router {
    build_app_with_authorizer(
        db,
        store_dir,
        signing_keys,
        local_backends,
        writable_local_backend,
        Arc::new(StaticTokenAuthorizer::new(write_token)),
        upstream_client,
    )
}

pub fn build_app_with_authorizer(
    db: SqliteDatabase,
    store_dir: StoreDir,
    signing_keys: Vec<NamedSigningKey>,
    local_backends: LocalObjectBackendRegistry,
    writable_local_backend: Option<LocalBackendName>,
    authorizer: Arc<dyn Authorizer>,
    upstream_client: Arc<dyn UpstreamCacheClient>,
) -> Router {
    let renderer = cache_core::narinfo::NarInfoRenderer::new(store_dir.clone());
    let signer = NarInfoSigner::new(store_dir, signing_keys);

    let local_objects = DbBackedLocalObjectStore::new(db.clone(), local_backends.clone());
    let upstream_selector = DbUpstreamSelector::new(db.clone());

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
        local_backends.clone(),
        writable_local_backend,
        upstream_client,
    );

    let gc_service = GcService::new(db.clone(), local_backends);
    let authorization_service = AuthorizationService::new(db.clone(), authorizer);

    let read_state = ReadAppState::new(Arc::new(read_service), 30);
    let write_state = WriteAppState::new(
        db,
        Arc::new(ingest_service),
        Arc::new(gc_service),
        Arc::new(authorization_service),
    );

    read_router(read_state).merge(write_router(write_state))
}

fn build_default_authorizer(config: &AppConfig) -> anyhow::Result<Arc<dyn Authorizer>> {
    let mut chain = ChainAuthorizer::new();
    chain.push(Arc::new(StaticTokenAuthorizer::new(
        config.write_token.clone(),
    )));

    if let Some(path) = &config.oidc_config_path {
        let oidc_config = OidcConfig::load_from_path(path)
            .with_context(|| format!("loading OIDC config from {}", path.display()))?;

        chain.push(Arc::new(OidcAuthorizer::new(
            oidc_config,
            Arc::new(ReqwestOidcHttpClient::default()),
        )));
    }

    Ok(Arc::new(chain))
}
