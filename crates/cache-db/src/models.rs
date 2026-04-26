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
    pub storage_id: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct ProjectRecord {
    pub id: Uuid,
    pub slug: ProjectSlug,
    pub display_name: String,
    pub public: bool,
    pub storage_id: Option<cache_core::storage::StorageId>,
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
            storage_id: self
                .storage_id
                .map(cache_core::storage::StorageId::new)
                .transpose()
                .map_err(anyhow::Error::new)
                .context("parsing project storage_id")?,
            created_at: OffsetDateTime::parse(&self.created_at, &Rfc3339)
                .context("parsing project created_at")?,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ProjectOidcIdentityLookupRow {
    pub project_slug: String,
    pub provider: String,
    pub repository: String,
    pub ref_pattern: String,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct ProjectOidcIdentityRecord {
    pub project_slug: ProjectSlug,
    pub provider: String,
    pub repository: String,
    pub ref_pattern: Option<String>,
    pub created_at: OffsetDateTime,
}

impl ProjectOidcIdentityLookupRow {
    pub fn into_record(self) -> Result<ProjectOidcIdentityRecord> {
        Ok(ProjectOidcIdentityRecord {
            project_slug: ProjectSlug::parse(&self.project_slug)
                .map_err(|_| anyhow::anyhow!("invalid project slug {}", self.project_slug))?,
            provider: self.provider,
            repository: self.repository,
            ref_pattern: if self.ref_pattern.is_empty() {
                None
            } else {
                Some(self.ref_pattern)
            },
            created_at: OffsetDateTime::parse(&self.created_at, &Rfc3339)
                .context("parsing project_oidc_identity created_at")?,
        })
    }
}

#[derive(Debug, Clone)]
pub struct AccessTokenLookupRow {
    pub id: String,
    pub name: String,
    pub project_slug: String,
    pub ref_pattern: String,
    pub created_at: String,
    pub expires_at: Option<String>,
    pub revoked_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AccessTokenRecord {
    pub id: String,
    pub name: String,
    pub project_slug: ProjectSlug,
    pub ref_pattern: Option<String>,
    pub created_at: OffsetDateTime,
    pub expires_at: Option<OffsetDateTime>,
    pub revoked_at: Option<OffsetDateTime>,
}

impl AccessTokenLookupRow {
    pub fn into_record(self) -> Result<AccessTokenRecord> {
        Ok(AccessTokenRecord {
            id: self.id,
            name: self.name,
            project_slug: ProjectSlug::parse(&self.project_slug)
                .map_err(|_| anyhow::anyhow!("invalid project slug {}", self.project_slug))?,
            ref_pattern: if self.ref_pattern.is_empty() {
                None
            } else {
                Some(self.ref_pattern)
            },
            created_at: OffsetDateTime::parse(&self.created_at, &Rfc3339)
                .context("parsing access token created_at")?,
            expires_at: self
                .expires_at
                .map(|value| {
                    OffsetDateTime::parse(&value, &Rfc3339)
                        .context("parsing access token expires_at")
                })
                .transpose()?,
            revoked_at: self
                .revoked_at
                .map(|value| {
                    OffsetDateTime::parse(&value, &Rfc3339)
                        .context("parsing access token revoked_at")
                })
                .transpose()?,
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
pub struct StorageObjectLookupRow {
    pub storage_id: String,
    pub content_type: String,
    pub content_length: Option<i64>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildStatus {
    Pending,
    Finalized,
}

impl BuildStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Finalized => "finalized",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "finalized" => Ok(Self::Finalized),
            other => Err(anyhow::anyhow!("invalid build status {}", other)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct BuildLookupRow {
    pub id: String,
    pub project_id: String,
    pub ref_name: String,
    pub revision: Option<String>,
    pub status: String,
    pub created_at: String,
    pub finalized_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BuildRecord {
    pub id: Uuid,
    pub project_id: Uuid,
    pub ref_name: String,
    pub revision: Option<String>,
    pub status: BuildStatus,
    pub created_at: OffsetDateTime,
    pub finalized_at: Option<OffsetDateTime>,
}

impl BuildLookupRow {
    pub fn into_record(self) -> Result<BuildRecord> {
        Ok(BuildRecord {
            id: Uuid::parse_str(&self.id).context("parsing build id")?,
            project_id: Uuid::parse_str(&self.project_id).context("parsing build project_id")?,
            ref_name: self.ref_name,
            revision: self.revision,
            status: BuildStatus::parse(&self.status)?,
            created_at: OffsetDateTime::parse(&self.created_at, &Rfc3339)
                .context("parsing build created_at")?,
            finalized_at: self
                .finalized_at
                .as_deref()
                .map(|value| OffsetDateTime::parse(value, &Rfc3339))
                .transpose()
                .context("parsing build finalized_at")?,
        })
    }
}

#[derive(Debug, Clone)]
pub struct BuildContextRow {
    pub build_id: String,
    pub project_id: String,
    pub project_slug: String,
    pub ref_name: String,
    pub revision: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct BuildContextRecord {
    pub build_id: Uuid,
    pub project_id: Uuid,
    pub project_slug: ProjectSlug,
    pub ref_name: String,
    pub revision: Option<String>,
    pub status: BuildStatus,
}

impl BuildContextRow {
    pub fn into_record(self) -> Result<BuildContextRecord> {
        Ok(BuildContextRecord {
            build_id: Uuid::parse_str(&self.build_id).context("parsing build context build_id")?,
            project_id: Uuid::parse_str(&self.project_id)
                .context("parsing build context project_id")?,
            project_slug: ProjectSlug::parse(&self.project_slug)
                .map_err(|_| anyhow::anyhow!("invalid project slug {}", self.project_slug))?,
            ref_name: self.ref_name,
            revision: self.revision,
            status: BuildStatus::parse(&self.status)?,
        })
    }
}

#[derive(Debug, Clone)]
pub struct PinLookupRow {
    pub scope_key: String,
    pub name: String,
    pub project_slug: Option<String>,
    pub store_path_hash: String,
    pub store_path: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct PinRecord {
    pub scope_key: String,
    pub name: String,
    pub project_slug: Option<ProjectSlug>,
    pub store_path_hash: String,
    pub store_path: String,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

impl PinLookupRow {
    pub fn into_record(self) -> Result<PinRecord> {
        Ok(PinRecord {
            scope_key: self.scope_key,
            name: self.name,
            project_slug: self
                .project_slug
                .map(|slug| {
                    ProjectSlug::parse(&slug)
                        .map_err(|_| anyhow::anyhow!("invalid project slug {}", slug))
                })
                .transpose()?,
            store_path_hash: self.store_path_hash,
            store_path: self.store_path,
            created_at: OffsetDateTime::parse(&self.created_at, &Rfc3339)
                .context("parsing pin created_at")?,
            updated_at: OffsetDateTime::parse(&self.updated_at, &Rfc3339)
                .context("parsing pin updated_at")?,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ProjectSigningKeyLookupRow {
    pub id: String,
    pub project_slug: String,
    pub name: String,
    pub public_key: String,
    pub encrypted_private_key: String,
    pub nonce: String,
    pub created_at: String,
    pub retired_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProjectSigningKeyRecord {
    pub id: String,
    pub project_slug: ProjectSlug,
    pub name: String,
    pub public_key: String,
    pub encrypted_private_key: String,
    pub nonce: String,
    pub created_at: OffsetDateTime,
    pub retired_at: Option<OffsetDateTime>,
}

impl ProjectSigningKeyLookupRow {
    pub fn into_record(self) -> Result<ProjectSigningKeyRecord> {
        Ok(ProjectSigningKeyRecord {
            id: self.id,
            project_slug: ProjectSlug::parse(&self.project_slug)
                .map_err(|_| anyhow::anyhow!("invalid project slug {}", self.project_slug))?,
            name: self.name,
            public_key: self.public_key,
            encrypted_private_key: self.encrypted_private_key,
            nonce: self.nonce,
            created_at: OffsetDateTime::parse(&self.created_at, &Rfc3339)
                .context("parsing project signing key created_at")?,
            retired_at: self
                .retired_at
                .map(|value| {
                    OffsetDateTime::parse(&value, &Rfc3339)
                        .context("parsing project signing key retired_at")
                })
                .transpose()?,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ProjectRetentionPolicyLookupRow {
    pub project_slug: String,
    pub inherited_default: i64,
    pub keep_latest_builds_per_ref: i64,
    pub object_delete_grace_seconds: i64,
}

#[derive(Debug, Clone)]
pub struct ProjectRetentionRuleLookupRow {
    pub priority: i64,
    pub ref_pattern: String,
    pub ttl_seconds: Option<i64>,
    pub keep_builds: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ProjectRetentionPolicyRecord {
    pub project_slug: ProjectSlug,
    pub inherited_default: bool,
    pub keep_latest_builds_per_ref: u32,
    pub object_delete_grace_seconds: u64,
    pub rules: Vec<ProjectRetentionRuleRecord>,
}

#[derive(Debug, Clone)]
pub struct ProjectRetentionRuleRecord {
    pub priority: u32,
    pub ref_pattern: String,
    pub ttl_seconds: Option<u64>,
    pub keep_builds: Option<u32>,
}

impl ProjectRetentionPolicyLookupRow {
    pub fn into_record(
        self,
        rules: Vec<ProjectRetentionRuleRecord>,
    ) -> Result<ProjectRetentionPolicyRecord> {
        Ok(ProjectRetentionPolicyRecord {
            project_slug: ProjectSlug::parse(&self.project_slug)
                .map_err(|_| anyhow::anyhow!("invalid project slug {}", self.project_slug))?,
            inherited_default: self.inherited_default != 0,
            keep_latest_builds_per_ref: u32::try_from(self.keep_latest_builds_per_ref)
                .context("converting keep_latest_builds_per_ref")?,
            object_delete_grace_seconds: u64::try_from(self.object_delete_grace_seconds)
                .context("converting object_delete_grace_seconds")?,
            rules,
        })
    }
}

impl ProjectRetentionRuleLookupRow {
    pub fn into_record(self) -> Result<ProjectRetentionRuleRecord> {
        Ok(ProjectRetentionRuleRecord {
            priority: u32::try_from(self.priority).context("converting retention priority")?,
            ref_pattern: self.ref_pattern,
            ttl_seconds: self
                .ttl_seconds
                .map(|value| u64::try_from(value).context("converting retention ttl_seconds"))
                .transpose()?,
            keep_builds: self
                .keep_builds
                .map(|value| u32::try_from(value).context("converting retention keep_builds"))
                .transpose()?,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ProjectRefRetentionRow {
    pub project_slug: String,
    pub ref_name: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct RetainedBuildLookupRow {
    pub build_id: String,
}
