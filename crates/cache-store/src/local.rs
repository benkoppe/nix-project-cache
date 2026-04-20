use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow};
use async_trait::async_trait;
use tokio::fs;

use cache_core::storage::LocalBackendName;

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
    async fn put_bytes(&self, storage_key: &str, bytes: BlobBytes) -> Result<()>;
}

#[derive(Clone, Default)]
pub struct LocalObjectBackendRegistry {
    backends: HashMap<LocalBackendName, Arc<dyn LocalObjectBackend>>,
}

impl LocalObjectBackendRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        backend_name: LocalBackendName,
        backend: Arc<dyn LocalObjectBackend>,
    ) -> Option<Arc<dyn LocalObjectBackend>> {
        self.backends.insert(backend_name, backend)
    }

    pub fn get(&self, backend_name: &LocalBackendName) -> Option<Arc<dyn LocalObjectBackend>> {
        self.backends.get(backend_name).cloned()
    }

    pub fn require(&self, backend_name: &LocalBackendName) -> Result<Arc<dyn LocalObjectBackend>> {
        self.get(backend_name)
            .ok_or_else(|| anyhow!("no local object backend registered for {}", backend_name))
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

    async fn put_bytes(&self, storage_key: &str, bytes: BlobBytes) -> Result<()> {
        let path = self.resolve_storage_key(storage_key)?;
        let parent = path
            .parent()
            .ok_or_else(|| anyhow!("storage key {} resolved without parent", storage_key))?;

        fs::create_dir_all(parent)
            .await
            .with_context(|| anyhow!("creating parent directories for {}", path.display()))?;
        //
        let temp_path = parent.join(format!(
            ".{}.tmp",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("object")
        ));

        fs::write(&temp_path, &bytes)
            .await
            .with_context(|| format!("writing temporary object {}", temp_path.display()))?;

        fs::rename(&temp_path, &path)
            .await
            .with_context(|| format!("renaming {} to {}", temp_path.display(), path.display()))?;

        Ok(())
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
    use tempfile::tempdir;

    use cache_core::storage::LocalBackendName;

    use super::*;

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

    #[tokio::test]
    async fn filesystem_backend_put_bytes_writes_and_reads_back() {
        let temp_dir = tempdir().unwrap();
        let backend = FilesystemLocalObjectBackend::new(temp_dir.path());

        backend
            .put_bytes(
                "objects/nar/test.nar",
                BlobBytes::from_static(b"hello world"),
            )
            .await
            .unwrap();
        let bytes = backend
            .get_bytes("objects/nar/test.nar")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(bytes, BlobBytes::from_static(b"hello world"));
    }

    #[tokio::test]
    async fn filesystem_backend_put_bytes_overwrites_existing_content() {
        let temp_dir = tempdir().unwrap();
        let backend = FilesystemLocalObjectBackend::new(temp_dir.path());

        backend
            .put_bytes("objects/nar/test.nar", BlobBytes::from_static(b"old"))
            .await
            .unwrap();
        backend
            .put_bytes("objects/nar/test.nar", BlobBytes::from_static(b"new"))
            .await
            .unwrap();

        let bytes = backend
            .get_bytes("objects/nar/test.nar")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(bytes, BlobBytes::from_static(b"new"));
    }

    #[tokio::test]
    async fn backend_registry_require_returns_registered_backend() {
        let temp_dir = tempdir().unwrap();
        let backend = std::sync::Arc::new(FilesystemLocalObjectBackend::new(temp_dir.path()));
        let mut registry = LocalObjectBackendRegistry::new();

        registry.register(LocalBackendName::fs(), backend);

        assert!(registry.require(&LocalBackendName::fs()).is_ok());
        assert!(
            registry
                .require(&LocalBackendName::new("missing").unwrap())
                .is_err()
        );
    }
}
