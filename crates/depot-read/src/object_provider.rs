use std::collections::HashMap;

use anyhow::Result;
use async_trait::async_trait;

use depot_core::view::DepotView;
use depot_store::blob::{BlobBytes, BlobMetadata};

use crate::local_objects::DbBackedObjectStore;

#[async_trait]
pub trait DepotObjectProvider: Send + Sync + 'static {
    async fn get_object(
        &self,
        view: &DepotView,
        object_path: &str,
    ) -> Result<Option<(BlobMetadata, BlobBytes)>>;
}

#[derive(Clone)]
pub struct DbBlobDepotObjectProvider {
    object_store: DbBackedObjectStore,
}

impl DbBlobDepotObjectProvider {
    pub fn new(object_store: DbBackedObjectStore) -> Self {
        Self { object_store }
    }
}

#[async_trait]
impl DepotObjectProvider for DbBlobDepotObjectProvider {
    async fn get_object(
        &self,
        view: &DepotView,
        object_path: &str,
    ) -> Result<Option<(BlobMetadata, BlobBytes)>> {
        self.object_store.get_visible(view, object_path).await
    }
}

#[derive(Debug, Default, Clone)]
pub struct InMemoryDepotObjectProvider {
    objects: HashMap<String, (BlobMetadata, BlobBytes)>,
}

impl InMemoryDepotObjectProvider {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(
        &mut self,
        object_path: impl Into<String>,
        metadata: BlobMetadata,
        bytes: BlobBytes,
    ) {
        self.objects.insert(object_path.into(), (metadata, bytes));
    }
}

#[async_trait]
impl DepotObjectProvider for InMemoryDepotObjectProvider {
    async fn get_object(
        &self,
        _view: &DepotView,
        object_path: &str,
    ) -> Result<Option<(BlobMetadata, BlobBytes)>> {
        Ok(self.objects.get(object_path).cloned())
    }
}
