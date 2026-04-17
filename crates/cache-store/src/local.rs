use std::collections::HashMap;

use anyhow::Result;
use async_trait::async_trait;

use crate::blob::{BlobBytes, BlobMetadata};

#[async_trait]
pub trait LocalObjectStore: Send + Sync + 'static {
    async fn head(&self, object_path: &str) -> Result<Option<BlobMetadata>>;
    async fn get(&self, object_path: &str) -> Result<Option<(BlobMetadata, BlobBytes)>>;
}

#[derive(Debug, Default, Clone)]
pub struct InMemoryLocalObjectStore {
    objects: HashMap<String, (BlobMetadata, BlobBytes)>,
}

impl InMemoryLocalObjectStore {
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
impl LocalObjectStore for InMemoryLocalObjectStore {
    async fn head(&self, object_path: &str) -> Result<Option<BlobMetadata>> {
        Ok(self
            .objects
            .get(object_path)
            .map(|(metadata, _)| metadata.clone()))
    }

    async fn get(&self, object_path: &str) -> Result<Option<(BlobMetadata, BlobBytes)>> {
        Ok(self.objects.get(object_path).cloned())
    }
}
