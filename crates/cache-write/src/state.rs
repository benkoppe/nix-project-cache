use std::sync::Arc;

use cache_core::key_crypto::KeyEncryptionKey;
use cache_db::SqliteDatabase;
use cache_ingest::{GcService, IngestService};
use cache_store::StorageCatalog;

use crate::authz::AuthorizationService;

#[derive(Clone)]
pub struct WriteAppState {
    pub db: SqliteDatabase,
    pub storage_catalog: StorageCatalog,
    pub ingest_service: Arc<IngestService>,
    pub gc_service: Arc<GcService>,
    pub authorization_service: Arc<AuthorizationService>,
    pub key_encryption_key: Option<KeyEncryptionKey>,
}

impl WriteAppState {
    pub fn new(
        db: SqliteDatabase,
        storage_catalog: StorageCatalog,
        ingest_service: Arc<IngestService>,
        gc_service: Arc<GcService>,
        authorization_service: Arc<AuthorizationService>,
        key_encryption_key: Option<KeyEncryptionKey>,
    ) -> Self {
        Self {
            db,
            storage_catalog,
            ingest_service,
            gc_service,
            authorization_service,
            key_encryption_key,
        }
    }
}
