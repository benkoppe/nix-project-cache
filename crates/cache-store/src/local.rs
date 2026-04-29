use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use async_trait::async_trait;
use time::OffsetDateTime;
use tokio::fs;
use tokio::io::{AsyncRead, AsyncWriteExt as _};
use uuid::Uuid;

use crate::blob::{BlobBytes, BlobMetadata};

pub type UploadReader = Pin<Box<dyn AsyncRead + Send>>;

#[derive(Debug, Clone)]
pub struct MultipartUpload {
    pub upload_id: String,
    pub part_size: u64,
}

#[derive(Debug, Clone)]
pub struct PresignedUploadPartUrl {
    pub url: String,
    pub expires_at: OffsetDateTime,
}

#[derive(Debug, Clone)]
pub struct CompletedMultipartUploadPart {
    pub part_number: i32,
    pub etag: String,
}

#[derive(Debug, Clone)]
pub struct CompletedMultipartUpload {
    pub content_length: u64,
    pub e_tag: Option<String>,
}

#[async_trait]
pub trait CacheStorage: Send + Sync + 'static {
    async fn head(&self, object_path: &str) -> Result<Option<BlobMetadata>>;

    async fn contains(&self, object_path: &str) -> Result<bool> {
        Ok(self.head(object_path).await?.is_some())
    }

    async fn get_bytes(&self, object_path: &str) -> Result<Option<BlobBytes>>;
    async fn put_bytes(&self, object_path: &str, bytes: BlobBytes) -> Result<()>;
    async fn put_stream(&self, object_path: &str, reader: UploadReader) -> Result<u64>;

    async fn create_multipart_upload(&self, _object_path: &str) -> Result<Option<MultipartUpload>> {
        Ok(None)
    }

    async fn presign_multipart_upload_part(
        &self,
        _object_path: &str,
        _upload_id: &str,
        _part_number: i32,
        _content_length: u64,
        _expires_in: Duration,
    ) -> Result<Option<PresignedUploadPartUrl>> {
        Ok(None)
    }

    async fn complete_multipart_upload(
        &self,
        _object_path: &str,
        _upload_id: &str,
        _parts: Vec<CompletedMultipartUploadPart>,
        _content_length: u64,
    ) -> Result<CompletedMultipartUpload> {
        Err(anyhow!(
            "storage backend does not support multipart uploads"
        ))
    }

    async fn abort_multipart_upload(&self, _object_path: &str, _upload_id: &str) -> Result<()> {
        Err(anyhow!(
            "storage backend does not support multipart uploads"
        ))
    }

    async fn delete(&self, object_path: &str) -> Result<()>;
}

#[async_trait]
pub trait ObjectStore: Send + Sync + 'static {
    async fn head(&self, object_path: &str) -> Result<Option<BlobMetadata>>;
    async fn get(&self, object_path: &str) -> Result<Option<(BlobMetadata, BlobBytes)>>;
}

#[derive(Debug, Clone)]
pub struct FilesystemStorage {
    root: PathBuf,
}

impl FilesystemStorage {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn resolve_object_path(&self, object_path: &str) -> Result<PathBuf> {
        let key_path = Path::new(object_path);
        let mut resolved = self.root.clone();

        for component in key_path.components() {
            match component {
                Component::Normal(segment) => resolved.push(segment),
                Component::CurDir => {}
                Component::Prefix(_) | Component::RootDir | Component::ParentDir => {
                    return Err(anyhow!("invalid object path {object_path}"));
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

    async fn prepare_write_path(&self, object_path: &str) -> Result<(PathBuf, PathBuf)> {
        let final_path = self.resolve_object_path(object_path)?;
        let parent = final_path
            .parent()
            .ok_or_else(|| anyhow!("object path {} resolved without parent", object_path))?;

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
impl CacheStorage for FilesystemStorage {
    async fn head(&self, object_path: &str) -> Result<Option<BlobMetadata>> {
        let path = self.resolve_object_path(object_path)?;

        match fs::metadata(&path).await {
            Ok(metadata) if metadata.is_file() => Ok(Some(BlobMetadata::new(
                "application/octet-stream",
                Some(metadata.len()),
                None,
                None,
            ))),
            Ok(_) => Ok(None),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error)
                .with_context(|| format!("reading filesystem metadata for {}", path.display())),
        }
    }

    async fn get_bytes(&self, object_path: &str) -> Result<Option<BlobBytes>> {
        let path = self.resolve_object_path(object_path)?;

        match fs::read(&path).await {
            Ok(bytes) => Ok(Some(BlobBytes::from(bytes))),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => {
                Err(error).with_context(|| format!("reading filesystem object {}", path.display()))
            }
        }
    }

    async fn put_bytes(&self, object_path: &str, bytes: BlobBytes) -> Result<()> {
        let (final_path, temp_path) = self.prepare_write_path(object_path).await?;

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

    async fn put_stream(&self, object_path: &str, mut reader: UploadReader) -> Result<u64> {
        let (final_path, temp_path) = self.prepare_write_path(object_path).await?;

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

    async fn delete(&self, object_path: &str) -> Result<()> {
        let path = self.resolve_object_path(object_path)?;

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
pub struct InMemoryObjectStore {
    objects: HashMap<String, (BlobMetadata, BlobBytes)>,
}

impl InMemoryObjectStore {
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
impl ObjectStore for InMemoryObjectStore {
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

    use super::*;

    fn stream_reader(bytes: &'static [u8]) -> UploadReader {
        let (mut writer, reader) = tokio::io::duplex(bytes.len().max(1));

        tokio::spawn(async move {
            writer.write_all(bytes).await.unwrap();
            writer.shutdown().await.unwrap();
        });

        Box::pin(reader)
    }

    #[tokio::test]
    async fn filesystem_storage_reads_existing_bytes() {
        let temp_dir = tempdir().unwrap();
        let storage = FilesystemStorage::new(temp_dir.path());
        let path = temp_dir.path().join("objects").join("nar").join("test.nar");
        fs::create_dir_all(path.parent().unwrap()).await.unwrap();
        fs::write(&path, b"hello world").await.unwrap();

        let bytes = storage
            .get_bytes("objects/nar/test.nar")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(bytes, BlobBytes::from_static(b"hello world"));
    }

    #[tokio::test]
    async fn filesystem_storage_contains_existing_object() {
        let temp_dir = tempdir().unwrap();
        let storage = FilesystemStorage::new(temp_dir.path());
        let path = temp_dir.path().join("objects").join("nar").join("test.nar");
        fs::create_dir_all(path.parent().unwrap()).await.unwrap();
        fs::write(&path, b"hello world").await.unwrap();

        assert!(storage.contains("objects/nar/test.nar").await.unwrap());
    }

    #[tokio::test]
    async fn filesystem_storage_rejects_parent_dir_segments() {
        let temp_dir = tempdir().unwrap();
        let storage = FilesystemStorage::new(temp_dir.path());

        let result = storage.get_bytes("../escape").await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn filesystem_storage_put_bytes_writes_and_reads_back() {
        let temp_dir = tempdir().unwrap();
        let storage = FilesystemStorage::new(temp_dir.path());

        storage
            .put_bytes(
                "objects/nar/test.nar",
                BlobBytes::from_static(b"hello world"),
            )
            .await
            .unwrap();
        let bytes = storage
            .get_bytes("objects/nar/test.nar")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(bytes, BlobBytes::from_static(b"hello world"));
    }

    #[tokio::test]
    async fn filesystem_storage_put_stream_writes_and_reads_back() {
        let temp_dir = tempdir().unwrap();
        let storage = FilesystemStorage::new(temp_dir.path());

        let written = storage
            .put_stream(
                "objects/nar/test.nar",
                stream_reader(b"hello streamed world"),
            )
            .await
            .unwrap();

        let bytes = storage
            .get_bytes("objects/nar/test.nar")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(written, 20);
        assert_eq!(bytes, BlobBytes::from_static(b"hello streamed world"));
    }

    #[tokio::test]
    async fn filesystem_storage_put_bytes_overwrites_existing_content() {
        let temp_dir = tempdir().unwrap();
        let storage = FilesystemStorage::new(temp_dir.path());

        storage
            .put_bytes("objects/nar/test.nar", BlobBytes::from_static(b"old"))
            .await
            .unwrap();
        storage
            .put_bytes("objects/nar/test.nar", BlobBytes::from_static(b"new"))
            .await
            .unwrap();

        let bytes = storage
            .get_bytes("objects/nar/test.nar")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(bytes, BlobBytes::from_static(b"new"));
    }
}
