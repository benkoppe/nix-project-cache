use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamInfo {
    pub name: String,
    pub base_url: String,
    pub priority: u32,
    pub enabled: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsertUpstreamRequest {
    pub name: String,
    pub base_url: String,
    pub priority: u32,
    pub enabled: bool,
}
