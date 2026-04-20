use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RunGcRequest {
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunGcResponse {
    pub deleted_objects: Vec<String>,
    pub deleted_count: usize,
}
