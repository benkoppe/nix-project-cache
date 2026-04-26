use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsertProjectRequest {
    pub slug: String,
    pub display_name: String,
    pub public: bool,
    pub storage_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub slug: String,
    pub display_name: String,
    pub public: bool,
    pub storage_id: Option<String>,
    pub created_at: String,
}
