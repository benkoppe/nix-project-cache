use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};

use cache_core::storage::LocalBackendName;
use cache_store::local::{FilesystemLocalObjectBackend, LocalObjectBackendRegistry};
use cache_store::s3::{S3LocalObjectBackend, S3LocalObjectBackendConfig};

#[derive(Clone)]
pub struct StorageConfig {
    pub object_root: PathBuf,
    pub writable_backend: Option<LocalBackendName>,
    pub s3: Option<S3StorageConfig>,
}

#[derive(Clone)]
pub struct S3StorageConfig {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub force_path_style: bool,
    pub prefix: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct RawStorageConfig {
    object_root: PathBuf,
    write_backend: String,
    s3: Option<RawS3StorageConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RawS3StorageConfig {
    endpoint: String,
    bucket: String,

    #[serde(default = "default_s3_region")]
    region: String,

    access_key_id: String,
    secret_access_key: String,

    #[serde(default = "default_true")]
    force_path_style: bool,

    #[serde(default)]
    prefix: Option<String>,
}

impl Default for RawStorageConfig {
    fn default() -> Self {
        Self {
            object_root: default_object_root(),
            write_backend: "fs".to_owned(),
            s3: None,
        }
    }
}

impl TryFrom<RawStorageConfig> for StorageConfig {
    type Error = anyhow::Error;

    fn try_from(raw: RawStorageConfig) -> Result<Self> {
        let s3 = raw.s3.map(S3StorageConfig::try_from).transpose()?;

        let writable_backend = match raw.write_backend.trim() {
            "none" => None,
            "fs" => Some(LocalBackendName::fs()),
            "s3" => {
                if s3.is_none() {
                    bail!("storage.s3 is required when storage.write_backend is \"s3\"");
                }

                Some(s3_backend_name()?)
            }
            "" => bail!("storage.write_backend must not be empty"),
            other => bail!(
                "unknown storage.write_backend {:?}; expected \"fs\", \"s3\", or \"none\"",
                other
            ),
        };

        Ok(Self {
            object_root: raw.object_root,
            writable_backend,
            s3,
        })
    }
}

impl TryFrom<RawS3StorageConfig> for S3StorageConfig {
    type Error = anyhow::Error;

    fn try_from(raw: RawS3StorageConfig) -> Result<Self> {
        Ok(Self {
            endpoint: require_non_empty(raw.endpoint, "storage.s3.endpoint")?,
            bucket: require_non_empty(raw.bucket, "storage.s3.bucket")?,
            region: require_non_empty(raw.region, "storage.s3.region")?,
            access_key_id: require_non_empty(raw.access_key_id, "storage.s3.access_key_id")?,
            secret_access_key: require_non_empty(
                raw.secret_access_key,
                "storage.s3.secret_access_key",
            )?,
            force_path_style: raw.force_path_style,
            prefix: raw
                .prefix
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty()),
        })
    }
}

impl StorageConfig {
    pub fn local_object_backends(&self) -> Result<LocalObjectBackendRegistry> {
        let mut registry = LocalObjectBackendRegistry::new();

        let fs_backend = Arc::new(FilesystemLocalObjectBackend::new(self.object_root.clone()));
        registry.register(LocalBackendName::fs(), fs_backend);

        if let Some(s3) = &self.s3 {
            registry.register(
                s3_backend_name()?,
                Arc::new(S3LocalObjectBackend::new(s3.backend_config())?),
            );
        }

        if let Some(name) = &self.writable_backend {
            registry
                .require(name)
                .with_context(|| format!("validating writable storage backend {}", name))?;
        }

        Ok(registry)
    }
}

impl S3StorageConfig {
    fn backend_config(&self) -> S3LocalObjectBackendConfig {
        S3LocalObjectBackendConfig {
            endpoint: self.endpoint.clone(),
            bucket: self.bucket.clone(),
            region: self.region.clone(),
            access_key_id: self.access_key_id.clone(),
            secret_access_key: self.secret_access_key.clone(),
            force_path_style: self.force_path_style,
            prefix: self.prefix.clone(),
        }
    }
}

fn s3_backend_name() -> Result<LocalBackendName> {
    LocalBackendName::new("s3").map_err(anyhow::Error::new)
}

fn default_object_root() -> PathBuf {
    PathBuf::from("./cache_objects")
}

fn default_s3_region() -> String {
    "us-east-1".to_owned()
}

fn default_true() -> bool {
    true
}

fn require_non_empty(value: String, name: &str) -> Result<String> {
    let value = value.trim().to_owned();
    if value.is_empty() {
        bail!("{name} must not be empty");
    }

    Ok(value)
}
