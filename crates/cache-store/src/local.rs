use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow};
use async_trait::async_trait;
use tokio::fs;

use crate::blob::{BlobBytes, BlobMetadata};

#[async_trait]
pub trait LocalObjectStore: Send + Sync + 'static {
    async fn head(&self, object_path: &str) -> Result<Option<BlobMetadata>>;
    async fn get(&self, object_path: &str) -> Result<Option<(BlobMetadata, BlobBytes)>>;
}

#[async_trait]
pub trait LocalObjectBackend: Send + Sync + 'static {
    async fn contains(&self, storage_key: &str) -> Result<bool>;
    async fn get_bytes(&self, storage_key: &str) -> Result<Option<BlobBytes>>;
}

#[derive(Clone, Default)]
pub struct LocalObjectBackendRegistry {
    backends: HashMap<String, Arc<dyn LocalObjectBackend>>,
}

impl LocalObjectBackendRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        backend_name: impl Into<String>,
        backend: Arc<dyn LocalObjectBackend>,
    ) -> Option<Arc<dyn LocalObjectBackend>> {
        self.backends.insert(backend_name.into(), backend)
    }

    pub fn get(&self, backend_name: &str) -> Option<Arc<dyn LocalObjectBackend>> {
        self.backends.get(backend_name).cloned()
    }
}

#[derive(Debug, Clone)]
pub struct FilesystemLocalObjectBackend {
    root: PathBuf,
}

impl FilesystemLocalObjectBackend {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn resolve_storage_key(&self, storage_key: &str) -> Result<PathBuf> {
        let key_path = Path::new(storage_key);
        let mut resolved = self.root.clone();

        for component in key_path.components() {
            match component {
                Component::Normal(segment) => resolved.push(segment),
                Component::CurDir => {}
                Component::Prefix(_) | Component::RootDir | Component::ParentDir => {
                    return Err(anyhow!("invalid storage key {storage_key}"));
                }
            }
        }

        Ok(resolved)
    }
}

#[async_trait]
impl LocalObjectBackend for FilesystemLocalObjectBackend {
    async fn contains(&self, storage_key: &str) -> Result<bool> {
        let path = self.resolve_storage_key(storage_key)?;

        match fs::metadata(&path).await {
            Ok(metadata) => Ok(metadata.is_file()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error)
                .with_context(|| format!("reading filesystem metadata for {}", path.display())),
        }
    }

    async fn get_bytes(&self, storage_key: &str) -> Result<Option<BlobBytes>> {
        let path = self.resolve_storage_key(storage_key)?;

        match fs::read(&path).await {
            Ok(bytes) => Ok(Some(BlobBytes::from(bytes))),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => {
                Err(error).with_context(|| format!("reading filesystem object {}", path.display()))
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn filesystem_backend_reads_existing_bytes() {
        let temp_dir = tempdir().unwrap();
        let backend = FilesystemLocalObjectBackend::new(temp_dir.path());
        let path = temp_dir.path().join("objects").join("nar").join("test.nar");
        fs::create_dir_all(path.parent().unwrap()).await.unwrap();
        fs::write(&path, b"hello world").await.unwrap();

        let bytes = backend
            .get_bytes("objects/nar/test.nar")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(bytes, BlobBytes::from_static(b"hello world"));
    }

    #[tokio::test]
    async fn filesystem_backend_contains_existing_object() {
        let temp_dir = tempdir().unwrap();
        let backend = FilesystemLocalObjectBackend::new(temp_dir.path());
        let path = temp_dir.path().join("objects").join("nar").join("test.nar");
        fs::create_dir_all(path.parent().unwrap()).await.unwrap();
        fs::write(&path, b"hello world").await.unwrap();

        assert!(backend.contains("objects/nar/test.nar").await.unwrap());
    }

    #[tokio::test]
    async fn filesystem_backend_rejects_parent_dir_segments() {
        let temp_dir = tempdir().unwrap();
        let backend = FilesystemLocalObjectBackend::new(temp_dir.path());

        let result = backend.get_bytes("../escape").await;

        assert!(result.is_err());
    }
}
