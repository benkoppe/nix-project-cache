use std::sync::Arc;

use cache_core::key_crypto::KeyEncryptionKey;
use cache_db::SqliteDatabase;
use cache_ingest::{GcService, IngestService};

use crate::authz::AuthorizationService;

#[derive(Clone)]
pub struct WriteAppState {
    pub db: SqliteDatabase,
    pub ingest_service: Arc<IngestService>,
    pub gc_service: Arc<GcService>,
    pub authorization_service: Arc<AuthorizationService>,
    pub key_encryption_key: Option<KeyEncryptionKey>,
}

impl WriteAppState {
    pub fn new(
        db: SqliteDatabase,
        ingest_service: Arc<IngestService>,
        gc_service: Arc<GcService>,
        authorization_service: Arc<AuthorizationService>,
        key_encryption_key: Option<KeyEncryptionKey>,
    ) -> Self {
        Self {
            db,
            ingest_service,
            gc_service,
            authorization_service,
            key_encryption_key,
        }
    }
}
