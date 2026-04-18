use anyhow::{Context as _, Result};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use cache_core::project::ProjectSlug;
use cache_store::upstream::UpstreamCache;

#[derive(Debug, Clone)]
pub struct ProjectLookupRow {
    pub id: String,
    pub slug: String,
    pub display_name: String,
    pub public: i64,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct ProjectRecord {
    pub id: Uuid,
    pub slug: ProjectSlug,
    pub display_name: String,
    pub public: bool,
    pub created_at: OffsetDateTime,
}

impl ProjectLookupRow {
    pub fn into_record(self) -> Result<ProjectRecord> {
        Ok(ProjectRecord {
            id: Uuid::parse_str(&self.id).context("parsing project id")?,
            slug: ProjectSlug::parse(&self.slug)
                .map_err(|_| anyhow::anyhow!("invalid project slug {}", self.slug))?,
            display_name: self.display_name,
            public: self.public != 0,
            created_at: OffsetDateTime::parse(&self.created_at, &Rfc3339)
                .context("parsing project created_at")?,
        })
    }
}

#[derive(Debug, Clone)]
pub struct UpstreamCacheLookupRow {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub priority: i64,
    pub enabled: i64,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct UpstreamCacheRecord {
    pub id: Uuid,
    pub name: String,
    pub base_url: String,
    pub priority: u32,
    pub enabled: bool,
    pub created_at: OffsetDateTime,
}

impl UpstreamCacheLookupRow {
    pub fn into_record(self) -> Result<UpstreamCacheRecord> {
        Ok(UpstreamCacheRecord {
            id: Uuid::parse_str(&self.id).context("parsing upstream id")?,
            name: self.name,
            base_url: self.base_url,
            priority: u32::try_from(self.priority).context("converting upstream priority")?,
            enabled: self.enabled != 0,
            created_at: OffsetDateTime::parse(&self.created_at, &Rfc3339)
                .context("parsing upstream created_at")?,
        })
    }
}

impl UpstreamCacheRecord {
    pub fn to_runtime_config(&self) -> UpstreamCache {
        UpstreamCache::new(
            self.id,
            self.name.clone(),
            self.base_url.clone(),
            self.priority,
        )
    }

    pub fn into_runtime_config(self) -> UpstreamCache {
        UpstreamCache::new(self.id, self.name, self.base_url, self.priority)
    }
}

#[derive(Debug, Clone)]
pub struct PathInfoLookupRow {
    pub store_path_hash: String,
    pub store_path: String,
    pub url: String,
    pub compression: String,
    pub nar_hash: String,
    pub nar_size: i64,
    pub deriver: Option<String>,
    pub ca: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PathReferenceValueRow {
    pub reference_store_path: String,
}

#[derive(Debug, Clone)]
pub struct PathSignatureValueRow {
    pub signature: String,
}

#[derive(Debug, Clone)]
pub struct LocalObjectLookupRow {
    pub content_type: String,
    pub content_length: Option<i64>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub storage_backend: String,
    pub storage_key: String,
}
