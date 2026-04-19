use std::sync::Arc;

use cache_auth::Authorizer;
use cache_ingest::IngestService;

#[derive(Clone)]
pub struct WriteAppState {
    pub ingest_service: Arc<IngestService>,
    pub authorizer: Arc<dyn Authorizer>,
}

impl WriteAppState {
    pub fn new(ingest_service: Arc<IngestService>, authorizer: Arc<dyn Authorizer>) -> Self {
        Self {
            ingest_service,
            authorizer,
        }
    }
}
