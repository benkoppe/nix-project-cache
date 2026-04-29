use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{Result, anyhow};

use depot_core::storage::StorageId;

use crate::local::DepotStorage;

#[derive(Clone)]
pub struct StorageCatalog {
    default_storage_id: StorageId,
    backends: Arc<BTreeMap<StorageId, Arc<dyn DepotStorage>>>,
}

impl StorageCatalog {
    pub fn new(
        default_storage_id: StorageId,
        backends: BTreeMap<StorageId, Arc<dyn DepotStorage>>,
    ) -> Result<Self> {
        if backends.is_empty() {
            return Err(anyhow!("at least one storage backend must be configured"));
        }

        if !backends.contains_key(&default_storage_id) {
            return Err(anyhow!(
                "default storage backend {} is not configured",
                default_storage_id
            ));
        }

        Ok(Self {
            default_storage_id,
            backends: Arc::new(backends),
        })
    }

    pub fn default_storage_id(&self) -> &StorageId {
        &self.default_storage_id
    }

    pub fn ids(&self) -> impl Iterator<Item = &StorageId> {
        self.backends.keys()
    }

    pub fn len(&self) -> usize {
        self.backends.len()
    }

    pub fn is_empty(&self) -> bool {
        self.backends.is_empty()
    }

    pub fn is_single_backend(&self) -> bool {
        self.backends.len() == 1
    }

    pub fn storage(&self, storage_id: &StorageId) -> Result<Arc<dyn DepotStorage>> {
        self.backends
            .get(storage_id)
            .cloned()
            .ok_or_else(|| anyhow!("storage backend {} is not configured", storage_id))
    }

    pub fn default_storage(&self) -> Result<Arc<dyn DepotStorage>> {
        self.storage(&self.default_storage_id)
    }

    pub fn resolve_optional_storage_id(&self, storage_id: Option<&StorageId>) -> Result<StorageId> {
        match storage_id {
            Some(storage_id) => {
                self.storage(storage_id)?;
                Ok(storage_id.clone())
            }
            None if self.is_single_backend() => Ok(self.default_storage_id.clone()),
            None => Err(anyhow!(
                "storage id is required when multiple storage backends are configured"
            )),
        }
    }
}
