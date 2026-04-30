use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectRetentionPolicyInfo {
    pub project: String,
    pub inherited_default: bool,
    pub keep_latest_builds_per_ref: u32,
    pub object_delete_grace_seconds: u64,
    pub rules: Vec<ProjectRetentionRuleInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectRetentionRuleInfo {
    pub priority: u32,
    pub ref_pattern: String,
    pub ttl_seconds: Option<u64>,
    pub keep_builds: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsertProjectRetentionPolicyRequest {
    pub keep_latest_builds_per_ref: u32,
    pub object_delete_grace_seconds: u64,
    pub rules: Vec<ProjectRetentionRuleInfo>,
}
