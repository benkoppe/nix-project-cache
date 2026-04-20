use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow};
use async_trait::async_trait;
use tokio::fs;
use tokio::io::{AsyncRead, AsyncWriteExt as _};
use uuid::Uuid;

use cache_core::storage::LocalBackendName;

use crate::blob::{BlobBytes, BlobMetadata};

pub type LocalUploadReader = Pin<Box<dyn AsyncRead + Send>>;

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
    async fn put_stream(&self, storage_key: &str, reader: LocalUploadReader) -> Result<u64>;
    async fn delete(&self, storage_key: &str) -> Result<()>;
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

    fn temporary_path_for(final_path: &Path) -> PathBuf {
        let file_name = final_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("object");

        final_path.with_file_name(format!(".{file_name}.{}.tmp", Uuid::now_v7()))
    }

    async fn prepare_write_path(&self, storage_key: &str) -> Result<(PathBuf, PathBuf)> {
        let final_path = self.resolve_storage_key(storage_key)?;
        let parent = final_path
            .parent()
            .ok_or_else(|| anyhow!("storage key {} resolved without parent", storage_key))?;

        fs::create_dir_all(parent)
            .await
            .with_context(|| anyhow!("creating parent directories for {}", final_path.display()))?;

        let temp_path = Self::temporary_path_for(&final_path);

        Ok((final_path, temp_path))
    }

    async fn cleanup_temp_file(temp_path: &Path) {
        let _ = fs::remove_file(temp_path).await;
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
        let (final_path, temp_path) = self.prepare_write_path(storage_key).await?;

        let result = async {
            fs::write(&temp_path, &bytes)
                .await
                .with_context(|| format!("writing temporary object {}", temp_path.display()))?;

            fs::rename(&temp_path, &final_path).await.with_context(|| {
                format!(
                    "renaming {} to {}",
                    temp_path.display(),
                    final_path.display()
                )
            })?;

            Ok(())
        }
        .await;

        if result.is_err() {
            Self::cleanup_temp_file(&temp_path).await;
        }

        result
    }

    async fn put_stream(&self, storage_key: &str, mut reader: LocalUploadReader) -> Result<u64> {
        let (final_path, temp_path) = self.prepare_write_path(storage_key).await?;

        let result = async {
            let mut temp_file = fs::File::create(&temp_path)
                .await
                .with_context(|| format!("creating temporary object {}", temp_path.display()))?;

            let written = tokio::io::copy(&mut reader, &mut temp_file)
                .await
                .with_context(|| format!("streaming object into {}", temp_path.display()))?;

            temp_file
                .flush()
                .await
                .with_context(|| format!("flushing temporary object {}", temp_path.display()))?;
            temp_file
                .sync_data()
                .await
                .with_context(|| format!("syncing temporary object {}", temp_path.display()))?;

            drop(temp_file);

            fs::rename(&temp_path, &final_path).await.with_context(|| {
                format!(
                    "renaming {} to {}",
                    temp_path.display(),
                    final_path.display()
                )
            })?;

            Ok(written)
        }
        .await;

        if result.is_err() {
            Self::cleanup_temp_file(&temp_path).await;
        }

        result
    }

    async fn delete(&self, storage_key: &str) -> Result<()> {
        let path = self.resolve_storage_key(storage_key)?;

        match fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => {
                Err(error).with_context(|| format!("deleting filesystem object {}", path.display()))
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
    use tempfile::tempdir;
    use tokio::io::AsyncWriteExt as _;

    use cache_core::storage::LocalBackendName;

    use super::*;

    fn stream_reader(bytes: &'static [u8]) -> LocalUploadReader {
        let (mut writer, reader) = tokio::io::duplex(bytes.len().max(1));

        tokio::spawn(async move {
            writer.write_all(bytes).await.unwrap();
            writer.shutdown().await.unwrap();
        });

        Box::pin(reader)
    }

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
    async fn filesystem_backend_put_stream_writes_and_reads_back() {
        let temp_dir = tempdir().unwrap();
        let backend = FilesystemLocalObjectBackend::new(temp_dir.path());

        let written = backend
            .put_stream(
                "objects/nar/test.nar",
                stream_reader(b"hello streamed world"),
            )
            .await
            .unwrap();

        let bytes = backend
            .get_bytes("objects/nar/test.nar")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(written, 20);
        assert_eq!(bytes, BlobBytes::from_static(b"hello streamed world"));
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
