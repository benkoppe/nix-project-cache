use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsertProjectOidcIdentityRequest {
    pub provider: String,
    pub repository: String,
    #[serde(default)]
    pub ref_patterns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteProjectOidcIdentityRequest {
    pub provider: String,
    pub repository: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectOidcIdentityInfo {
    pub provider: String,
    pub repository: String,
    pub ref_patterns: Vec<String>,
}
