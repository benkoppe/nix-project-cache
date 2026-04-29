use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RunGcRequest {
    pub dry_run: bool,

    #[serde(default)]
    pub grace_period_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunGcResponse {
    pub deleted_objects: Vec<String>,
    pub deleted_count: usize,
}
