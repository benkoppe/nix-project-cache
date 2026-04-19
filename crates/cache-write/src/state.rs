use std::sync::Arc;

use cache_auth::Authorizer;
use cache_ingest::IngestService;

#[derive(Clone)]
pub struct WriteAppState {
    pub ingest_service: Arc<IngestService>,
    pub authorizer: Arc<dyn Authorizer>,
}
