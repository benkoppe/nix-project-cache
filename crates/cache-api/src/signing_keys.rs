use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSigningKeyInfo {
    pub project: String,
    pub public_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateProjectSigningKeyRequest {
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportProjectSigningKeyRequest {
    pub name: Option<String>,
    pub signing_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSigningKeyResponse {
    pub project: String,
    pub public_key: String,
}
