use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};

use depot_core::storage::StorageId;
use depot_store::{
    CacheStorage, FilesystemStorage, S3Storage, S3StorageConfig as RuntimeS3StorageConfig,
    StorageCatalog,
};

#[derive(Clone)]
pub struct StorageConfig {
    pub default_storage_id: StorageId,
    pub backends: BTreeMap<StorageId, StorageBackendConfig>,
}

#[derive(Clone)]
pub enum StorageBackendConfig {
    Filesystem(FilesystemStorageConfig),
    S3(S3StorageConfig),
}

#[derive(Clone)]
pub struct FilesystemStorageConfig {
    pub root: PathBuf,
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
    default: Option<String>,
    backends: BTreeMap<String, RawStorageBackendConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
enum RawStorageBackendConfig {
    Filesystem {
        root: PathBuf,
    },
    S3 {
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
    },
}

impl Default for RawStorageConfig {
    fn default() -> Self {
        Self {
            default: Some(StorageId::main().as_str().to_owned()),
            backends: BTreeMap::from([(
                StorageId::main().as_str().to_owned(),
                RawStorageBackendConfig::Filesystem {
                    root: default_object_root(),
                },
            )]),
        }
    }
}

impl TryFrom<RawStorageConfig> for StorageConfig {
    type Error = anyhow::Error;

    fn try_from(raw: RawStorageConfig) -> Result<Self> {
        if raw.backends.is_empty() {
            bail!("storage.backends must contain at least one backend");
        }

        let mut backends = BTreeMap::new();

        for (raw_id, raw_backend) in raw.backends {
            let storage_id = StorageId::new(raw_id)
                .map_err(anyhow::Error::new)
                .context("parsing storage backend id")?;

            let backend = match raw_backend {
                RawStorageBackendConfig::Filesystem { root } => {
                    StorageBackendConfig::Filesystem(FilesystemStorageConfig { root })
                }
                RawStorageBackendConfig::S3 {
                    endpoint,
                    bucket,
                    region,
                    access_key_id,
                    secret_access_key,
                    force_path_style,
                    prefix,
                } => StorageBackendConfig::S3(S3StorageConfig {
                    endpoint: require_non_empty(endpoint, "storage backend s3.endpoint")?,
                    bucket: require_non_empty(bucket, "storage backend s3.bucket")?,
                    region: require_non_empty(region, "storage backend s3.region")?,
                    access_key_id: require_non_empty(
                        access_key_id,
                        "storage backend s3.access_key_id",
                    )?,
                    secret_access_key: require_non_empty(
                        secret_access_key,
                        "storage backend s3.secret_access_key",
                    )?,
                    force_path_style,
                    prefix: prefix
                        .map(|value| value.trim().to_owned())
                        .filter(|value| !value.is_empty()),
                }),
            };

            if backends.insert(storage_id.clone(), backend).is_some() {
                bail!("duplicate storage backend id {}", storage_id);
            }
        }

        let default_storage_id = match raw.default {
            Some(default) => StorageId::new(default)
                .map_err(anyhow::Error::new)
                .context("parsing storage.default")?,
            None if backends.len() == 1 => {
                backends.keys().next().expect("one backend exists").clone()
            }
            None => {
                bail!("storage.default is required when multiple storage backends are configured")
            }
        };

        if !backends.contains_key(&default_storage_id) {
            bail!(
                "storage.default {} does not reference a configured backend",
                default_storage_id
            );
        }

        Ok(Self {
            default_storage_id,
            backends,
        })
    }
}

impl StorageConfig {
    pub fn catalog(&self) -> Result<StorageCatalog> {
        let mut backends: BTreeMap<StorageId, Arc<dyn CacheStorage>> = BTreeMap::new();

        for (storage_id, backend) in &self.backends {
            let storage: Arc<dyn CacheStorage> = match backend {
                StorageBackendConfig::Filesystem(config) => {
                    Arc::new(FilesystemStorage::new(config.root.clone()))
                }
                StorageBackendConfig::S3(config) => {
                    Arc::new(S3Storage::new(config.backend_config())?)
                }
            };

            backends.insert(storage_id.clone(), storage);
        }

        StorageCatalog::new(self.default_storage_id.clone(), backends)
    }
}

impl S3StorageConfig {
    fn backend_config(&self) -> RuntimeS3StorageConfig {
        RuntimeS3StorageConfig {
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
