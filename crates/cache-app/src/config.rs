use std::env;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result};

use cache_core::nix::StoreDir;
use cache_core::signing::NamedSigningKey;
use cache_core::storage::LocalBackendName;
use cache_store::local::{FilesystemLocalObjectBackend, LocalObjectBackendRegistry};

#[derive(Clone)]
pub struct AppConfig {
    pub bind_address: String,
    pub db_path: PathBuf,
    pub store_dir: StoreDir,
    pub local_object_root: PathBuf,
    pub signing_keys: Vec<NamedSigningKey>,
    pub write_token: Option<String>,
    pub writable_local_backend: Option<LocalBackendName>,
}

impl AppConfig {
    pub fn from_env() -> Result<Self> {
        let bind_address =
            env::var("CACHE_BIND_ADDRESS").unwrap_or_else(|_| "127.0.0.1:8080".to_owned());

        let db_path = PathBuf::from(
            env::var("CACHE_DB_PATH").unwrap_or_else(|_| "./cache_db/cache.sqlite".to_owned()),
        );

        let store_dir_text =
            env::var("CACHE_STORE_DIR").unwrap_or_else(|_| "/nix/store".to_owned());
        let store_dir = StoreDir::new(store_dir_text)
            .map_err(anyhow::Error::new)
            .context("parsing CACHE_STORE_DIR")?;

        let local_object_root = PathBuf::from(
            env::var("CACHE_OBJECT_ROOT").unwrap_or_else(|_| "./cache_objects".to_owned()),
        );

        let signing_keys = match env::var("CACHE_SIGNING_KEYS") {
            Ok(value) => value
                .split(',')
                .filter(|entry| !entry.trim().is_empty())
                .map(|entry| NamedSigningKey::parse(entry.trim()).map_err(anyhow::Error::new))
                .collect::<Result<Vec<_>>>()
                .context("parsing CACHE_SIGNING_KEYS")?,
            Err(_) => Vec::new(),
        };

        let write_token = env::var("CACHE_WRITE_TOKEN").ok();

        let writable_local_backend = match env::var("CACHE_WRITABLE_LOCAL_BACKEND") {
            Ok(value) if value.trim().is_empty() => None,
            Ok(value) => Some(
                LocalBackendName::new(value.trim())
                    .map_err(anyhow::Error::new)
                    .context("parsing CACHE_WRITABLE_LOCAL_BACKEND")?,
            ),
            Err(_) => Some(LocalBackendName::fs()),
        };

        Ok(Self {
            bind_address,
            db_path,
            store_dir,
            local_object_root,
            signing_keys,
            write_token,
            writable_local_backend,
        })
    }

    pub fn local_object_backends(&self) -> LocalObjectBackendRegistry {
        let mut registry = LocalObjectBackendRegistry::new();
        let fs_backend = Arc::new(FilesystemLocalObjectBackend::new(
            self.local_object_root.clone(),
        ));

        registry.register(LocalBackendName::fs().as_str(), fs_backend.clone());

        if let Some(name) = &self.writable_local_backend
            && name.as_str() != LocalBackendName::fs().as_str()
        {
            registry.register(name.as_str(), fs_backend);
        }

        registry
    }
}
