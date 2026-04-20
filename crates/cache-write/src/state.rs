use std::sync::Arc;

use cache_auth::Authorizer;
use cache_db::SqliteDatabase;
use cache_ingest::{GcService, IngestService};

#[derive(Clone)]
pub struct WriteAppState {
    pub db: SqliteDatabase,
    pub ingest_service: Arc<IngestService>,
    pub gc_service: Arc<GcService>,
    pub authorizer: Arc<dyn Authorizer>,
}

impl WriteAppState {
    pub fn new(
        db: SqliteDatabase,
        ingest_service: Arc<IngestService>,
        gc_service: Arc<GcService>,
        authorizer: Arc<dyn Authorizer>,
    ) -> Self {
        Self {
            db,
            ingest_service,
            gc_service,
            authorizer,
        }
    }
}
