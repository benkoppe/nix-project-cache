use anyhow::{Result, anyhow};
use async_trait::async_trait;
use tracing::warn;

use cache_db::SqliteDatabase;
use cache_store::blob::{BlobBytes, BlobMetadata};
use cache_store::local::{LocalObjectBackendRegistry, LocalObjectStore};

#[derive(Clone)]
pub struct DbBackedLocalObjectStore {
    db: SqliteDatabase,
    backends: LocalObjectBackendRegistry,
}

impl DbBackedLocalObjectStore {
    pub fn new(db: SqliteDatabase, backends: LocalObjectBackendRegistry) -> Self {
        Self { db, backends }
    }
}

#[async_trait]
impl LocalObjectStore for DbBackedLocalObjectStore {
    async fn head(&self, object_path: &str) -> Result<Option<BlobMetadata>> {
        let Some(record) = self.db.get_local_object(object_path).await? else {
            return Ok(None);
        };

        let Some(backend) = self.backends.get(&record.storage_backend) else {
            return Err(anyhow!(
                "no local object backend registered for {}",
                record.storage_backend
            ));
        };

        if backend.contains(&record.storage_key).await? {
            Ok(Some(record.metadata))
        } else {
            warn!(
                object_path,
                storage_backend = %record.storage_backend,
                storage_key = %record.storage_key,
                "local object metadata exists but backend object is missing"
            );
            Ok(None)
        }
    }

    async fn get(&self, object_path: &str) -> Result<Option<(BlobMetadata, BlobBytes)>> {
        let Some(record) = self.db.get_local_object(object_path).await? else {
            return Ok(None);
        };

        let Some(backend) = self.backends.get(&record.storage_backend) else {
            return Err(anyhow!(
                "no local object backend registered for {}",
                record.storage_backend
            ));
        };

        match backend.get_bytes(&record.storage_key).await? {
            Some(bytes) => Ok(Some((record.metadata, bytes))),
            None => {
                warn!(
                    object_path,
                    storage_backend = %record.storage_backend,
                    storage_key = %record.storage_key,
                    "local object metadata exists but backend object is missing"
                );
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;
    use tokio::fs;

    use cache_core::storage::LocalBackendName;
    use cache_store::blob::BlobMetadata;
    use cache_store::local::{FilesystemLocalObjectBackend, LocalObjectBackendRegistry};

    use super::*;

    #[tokio::test]
    async fn db_backed_local_object_store_reads_bytes_from_registered_backend() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("cache.db");
        let objects_root = temp_dir.path().join("objects-root");

        let db = SqliteDatabase::open(&db_path).await.unwrap();

        let file_path = objects_root.join("objects").join("nar").join("test.nar");
        fs::create_dir_all(file_path.parent().unwrap())
            .await
            .unwrap();
        fs::write(&file_path, b"local-bytes").await.unwrap();

        let metadata = BlobMetadata::new("application/octet-stream", Some(11), None, None);
        let backend_name = LocalBackendName::fs();

        db.upsert_local_object(
            "nar/test.nar",
            &metadata,
            &backend_name,
            "objects/nar/test.nar",
        )
        .await
        .unwrap();

        let mut backends = LocalObjectBackendRegistry::new();
        backends.register(
            backend_name.as_str(),
            std::sync::Arc::new(FilesystemLocalObjectBackend::new(&objects_root)),
        );

        let store = DbBackedLocalObjectStore::new(db, backends);

        let loaded = store.get("nar/test.nar").await.unwrap().unwrap();

        assert_eq!(loaded.0.content_type, "application/octet-stream");
        assert_eq!(loaded.1, BlobBytes::from_static(b"local-bytes"));
    }

    #[tokio::test]
    async fn db_backed_local_object_store_returns_none_when_backend_object_missing() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("cache.db");
        let objects_root = temp_dir.path().join("objects-root");

        let db = SqliteDatabase::open(&db_path).await.unwrap();

        let metadata = BlobMetadata::new("application/octet-stream", Some(11), None, None);
        let backend_name = LocalBackendName::fs();

        db.upsert_local_object(
            "nar/test.nar",
            &metadata,
            &backend_name,
            "objects/nar/test.nar",
        )
        .await
        .unwrap();

        let mut backends = LocalObjectBackendRegistry::new();
        backends.register(
            backend_name.as_str(),
            std::sync::Arc::new(FilesystemLocalObjectBackend::new(&objects_root)),
        );

        let store = DbBackedLocalObjectStore::new(db, backends);

        assert!(store.get("nar/test.nar").await.unwrap().is_none());
    }
}
