#[derive(Debug, Clone)]
pub struct ProjectRow {
    pub id: String,
    pub slug: String,
    pub display_name: String,
    pub public: i64,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct UpstreamCacheRow {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub priority: i64,
    pub enabled: i64,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct PathInfoRow {
    pub store_path_hash: String,
    pub store_path: String,
    pub url: String,
    pub compression: String,
    pub nar_hash: String,
    pub nar_size: i64,
    pub deriver: Option<String>,
    pub ca: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct PathReferenceRow {
    pub store_path_hash: String,
    pub reference_store_path: String,
    pub ordinal: i64,
}

#[derive(Debug, Clone)]
pub struct PathSignatureRow {
    pub store_path_hash: String,
    pub signature: String,
    pub ordinal: i64,
}

#[derive(Debug, Clone)]
pub struct LocalObjectRow {
    pub object_path: String,
    pub content_type: String,
    pub content_length: Option<i64>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub storage_backend: String,
    pub storage_key: String,
    pub created_at: String,
}
