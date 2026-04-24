pub mod config;

use std::sync::Arc;

use anyhow::Context as _;
use axum::Router;

use cache_auth::{
    Authorizer, ChainAuthorizer, OidcAuthorizer, OidcConfig, ReqwestOidcHttpClient,
    StaticTokenAuthorizer,
};
use cache_core::key_crypto::KeyEncryptionKey;
use cache_core::nix::StoreDir;
use cache_core::signing::NamedSigningKey;
use cache_core::storage::LocalBackendName;
use cache_db::SqliteDatabase;
use cache_ingest::{GcService, IngestService};
use cache_read::{
    DbBackedLocalObjectStore, DbNarInfoResolver, DbProjectSigningKeys, DbUpstreamSelector,
    ReadAppState, ReadService, read_router,
};
use cache_store::local::LocalObjectBackendRegistry;
use cache_store::upstream::{ReqwestUpstreamCacheClient, UpstreamCacheClient};
use cache_write::{AuthorizationService, WriteAppState, write_router};

pub use config::AppConfig;

pub struct AppParts {
    pub db: SqliteDatabase,
    pub store_dir: StoreDir,
    pub aggregate_signing_key: Option<NamedSigningKey>,
    pub key_encryption_key: Option<KeyEncryptionKey>,
    pub local_backends: LocalObjectBackendRegistry,
    pub writable_local_backend: Option<LocalBackendName>,
    pub upstream_client: Arc<dyn UpstreamCacheClient>,
}

pub async fn build_app(config: &AppConfig) -> anyhow::Result<Router> {
    let db = SqliteDatabase::open(&config.db_path)
        .await
        .context("opening sqlite metadata database")?;

    let authorizer = build_default_authorizer(config)?;

    Ok(build_app_with_authorizer(
        AppParts {
            db,
            store_dir: config.store_dir.clone(),
            aggregate_signing_key: config.aggregate_signing_key.clone(),
            key_encryption_key: config.key_encryption_key.clone(),
            local_backends: config.local_object_backends(),
            writable_local_backend: config.writable_local_backend.clone(),
            upstream_client: Arc::new(ReqwestUpstreamCacheClient::default()),
        },
        authorizer,
    ))
}

pub fn build_app_with_parts(parts: AppParts, write_token: Option<String>) -> Router {
    build_app_with_authorizer(parts, Arc::new(StaticTokenAuthorizer::new(write_token)))
}

pub fn build_app_with_authorizer(parts: AppParts, authorizer: Arc<dyn Authorizer>) -> Router {
    let AppParts {
        db,
        store_dir,
        aggregate_signing_key,
        key_encryption_key,
        local_backends,
        writable_local_backend,
        upstream_client,
    } = parts;

    let renderer = cache_core::narinfo::NarInfoRenderer::new(store_dir.clone());

    let local_objects = DbBackedLocalObjectStore::new(db.clone(), local_backends.clone());
    let upstream_selector = DbUpstreamSelector::new(db.clone());

    let project_signing_keys = key_encryption_key
        .clone()
        .map(|key| DbProjectSigningKeys::new(db.clone(), key));

    let read_service = ReadService::new(
        Arc::new(DbNarInfoResolver::new(db.clone())),
        Arc::new(local_objects.clone()),
        upstream_client.clone(),
        Arc::new(upstream_selector),
        renderer,
        aggregate_signing_key.clone(),
        project_signing_keys,
    );

    let ingest_service = IngestService::new(
        db.clone(),
        store_dir.clone(),
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
        key_encryption_key,
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
