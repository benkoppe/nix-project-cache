use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatePinRequest {
    pub project: Option<String>,
    pub store_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinInfo {
    pub name: String,
    pub project: Option<String>,
    pub store_path: String,
    pub created_at: String,
    pub updated_at: String,
}
