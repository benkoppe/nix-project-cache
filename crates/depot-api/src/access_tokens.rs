use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateAccessTokenRequest {
    pub name: String,
    pub project: String,
    #[serde(default)]
    pub ref_patterns: Vec<String>,
    #[serde(default)]
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateAccessTokenResponse {
    pub token: String,
    pub info: AccessTokenInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessTokenInfo {
    pub id: String,
    pub name: String,
    pub project: String,
    pub ref_patterns: Vec<String>,
    pub created_at: String,
    pub expires_at: Option<String>,
    pub revoked_at: Option<String>,
}
