pub mod config;
pub mod storage;

use std::sync::Arc;

use anyhow::Context as _;
use axum::Router;

use cache_auth::{
    Authorizer, ChainAuthorizer, OidcAuthorizer, ReqwestOidcHttpClient, StaticTokenAuthorizer,
};
use cache_core::key_crypto::KeyEncryptionKey;
use cache_core::nix::StoreDir;
use cache_core::signing::NamedSigningKey;
use cache_db::SqliteDatabase;
use cache_ingest::{GcService, IngestService};
use cache_read::{
    DbBackedObjectStore, DbBlobCacheObjectProvider, DbNarInfoResolver, DbProjectSigningKeys,
    DbUpstreamSelector, ReadAppState, ReadService, read_router,
};
use cache_store::StorageCatalog;
use cache_store::upstream::{ReqwestUpstreamCacheClient, UpstreamCacheClient};
use cache_write::{AuthorizationService, DbAccessTokenAuthorizer, WriteAppState, write_router};

pub use config::{AppConfig, AppMode};
pub use storage::{S3StorageConfig, StorageConfig};

pub struct AppParts {
    pub db: SqliteDatabase,
    pub mode: AppMode,
    pub store_dir: StoreDir,
    pub aggregate_signing_key: Option<NamedSigningKey>,
    pub key_encryption_key: Option<KeyEncryptionKey>,
    pub storage_catalog: StorageCatalog,
    pub upstream_client: Arc<dyn UpstreamCacheClient>,
    pub cache_priority: u32,
}

pub async fn build_app(config: &AppConfig) -> anyhow::Result<Router> {
    let db = match config.server.mode {
        AppMode::ReadWrite => SqliteDatabase::open(&config.database.path)
            .await
            .context("opening sqlite metadata database")?,
        AppMode::ReadOnly => SqliteDatabase::open_read_only(&config.database.path)
            .await
            .context("opening sqlite metadata database read-only")?,
    };

    let authorizer = build_default_authorizer(config)?;

    let storage_catalog = config.storage.catalog()?;

    if config.server.mode == AppMode::ReadWrite {
        validate_project_storage_ids(&db, &storage_catalog).await?;
    }

    Ok(build_app_with_authorizer(
        AppParts {
            db,
            mode: config.server.mode,
            store_dir: config.nix.store_dir.clone(),
            aggregate_signing_key: config.signing.aggregate_signing_key.clone(),
            key_encryption_key: config.signing.project_key_encryption_key.clone(),
            storage_catalog,
            upstream_client: Arc::new(ReqwestUpstreamCacheClient::default()),
            cache_priority: config.server.priority,
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
        mode,
        store_dir,
        aggregate_signing_key,
        key_encryption_key,
        storage_catalog,
        upstream_client,
        cache_priority,
    } = parts;

    let renderer = cache_core::narinfo::NarInfoRenderer::new(store_dir.clone());

    let object_store = DbBackedObjectStore::new(db.clone(), storage_catalog.clone());
    let object_provider = DbBlobCacheObjectProvider::new(object_store.clone());
    let upstream_selector = DbUpstreamSelector::new(db.clone());

    let project_signing_keys = key_encryption_key
        .clone()
        .map(|key| DbProjectSigningKeys::new(db.clone(), key));

    let read_service = ReadService::new(
        Arc::new(DbNarInfoResolver::new(db.clone())),
        Arc::new(object_provider.clone()),
        upstream_client.clone(),
        Arc::new(upstream_selector),
        renderer,
        aggregate_signing_key.clone(),
        project_signing_keys,
    );

    let read_state = ReadAppState::new(Arc::new(read_service), cache_priority);
    let read_routes = read_router(read_state);

    if mode == AppMode::ReadOnly {
        return read_routes;
    }

    let ingest_service = IngestService::new(
        db.clone(),
        store_dir,
        storage_catalog.clone(),
        upstream_client,
    );

    let gc_service = GcService::new(db.clone(), storage_catalog.clone());

    let mut write_authorizer = ChainAuthorizer::new();
    write_authorizer.push(authorizer);
    write_authorizer.push(Arc::new(DbAccessTokenAuthorizer::new(db.clone())));

    let authorization_service = AuthorizationService::new(db.clone(), Arc::new(write_authorizer));

    let write_state = WriteAppState::new(
        db,
        storage_catalog,
        Arc::new(ingest_service),
        Arc::new(gc_service),
        Arc::new(authorization_service),
        key_encryption_key,
    );
    let write_routes = write_router(write_state);

    read_routes.merge(write_routes)
}

fn build_default_authorizer(config: &AppConfig) -> anyhow::Result<Arc<dyn Authorizer>> {
    let mut chain = ChainAuthorizer::new();

    chain.push(Arc::new(StaticTokenAuthorizer::new(
        config.auth.write_token.clone(),
    )));

    if let Some(oidc_config) = config.auth.oidc.clone() {
        chain.push(Arc::new(OidcAuthorizer::new(
            oidc_config,
            Arc::new(ReqwestOidcHttpClient::default()),
        )));
    }

    Ok(Arc::new(chain))
}

async fn validate_project_storage_ids(
    db: &SqliteDatabase,
    storage_catalog: &StorageCatalog,
) -> anyhow::Result<()> {
    for (project, storage_id) in db.list_project_storage_ids().await? {
        storage_catalog.storage(&storage_id).with_context(|| {
            format!(
                "project {} references unavailable storage backend {}",
                project.as_str(),
                storage_id
            )
        })?;
    }

    Ok(())
}
