use anyhow::Result;
use async_trait::async_trait;
use tracing::warn;

use cache_core::storage::StorageId;
use cache_core::view::CacheView;
use cache_db::SqliteDatabase;
use cache_store::blob::{BlobBytes, BlobMetadata};
use cache_store::{ObjectStore, StorageCatalog};

#[derive(Clone)]
pub struct DbBackedObjectStore {
    db: SqliteDatabase,
    catalog: StorageCatalog,
}

impl DbBackedObjectStore {
    pub fn new(db: SqliteDatabase, catalog: StorageCatalog) -> Self {
        Self { db, catalog }
    }

    async fn preferred_storage_id(&self, view: &CacheView) -> Result<StorageId> {
        match view {
            CacheView::Aggregate => Ok(self.catalog.default_storage_id().clone()),
            CacheView::Project(project) => Ok(self
                .db
                .get_project_storage_id(project)
                .await?
                .unwrap_or_else(|| self.catalog.default_storage_id().clone())),
        }
    }

    async fn get_unscoped(
        &self,
        preferred_storage_id: Option<&StorageId>,
        object_path: &str,
    ) -> Result<Option<(BlobMetadata, BlobBytes)>> {
        let mut records = self.db.list_storage_objects(object_path).await?;

        if let Some(preferred_storage_id) = preferred_storage_id {
            records.sort_by_key(|record| {
                if &record.storage_id == preferred_storage_id {
                    (0, record.storage_id.clone())
                } else {
                    (1, record.storage_id.clone())
                }
            });
        }

        for record in records {
            let storage = match self.catalog.storage(&record.storage_id) {
                Ok(storage) => storage,
                Err(error) => {
                    warn!(
                        ?error,
                        storage_id = %record.storage_id,
                        object_path,
                        "storage object references unavailable backend"
                    );
                    continue;
                }
            };

            match storage.get_bytes(object_path).await? {
                Some(bytes) => return Ok(Some((record.metadata, bytes))),
                None => warn!(
                    storage_id = %record.storage_id,
                    object_path,
                    "storage object metadata exists but backend object is missing"
                ),
            }
        }

        Ok(None)
    }

    pub async fn get_visible(
        &self,
        view: &CacheView,
        object_path: &str,
    ) -> Result<Option<(BlobMetadata, BlobBytes)>> {
        if !self
            .db
            .storage_object_visible_in_view(view, object_path)
            .await?
        {
            return Ok(None);
        }

        let preferred_storage_id = self.preferred_storage_id(view).await?;
        self.get_unscoped(Some(&preferred_storage_id), object_path)
            .await
    }
}

#[async_trait]
impl ObjectStore for DbBackedObjectStore {
    async fn head(&self, object_path: &str) -> Result<Option<BlobMetadata>> {
        Ok(self
            .get_unscoped(Some(self.catalog.default_storage_id()), object_path)
            .await?
            .map(|(metadata, _)| metadata))
    }

    async fn get(&self, object_path: &str) -> Result<Option<(BlobMetadata, BlobBytes)>> {
        self.get_unscoped(Some(self.catalog.default_storage_id()), object_path)
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::sync::Arc;

    use tempfile::tempdir;
    use tokio::fs;

    use cache_core::storage::StorageId;
    use cache_store::blob::BlobMetadata;
    use cache_store::{CacheStorage, FilesystemStorage, StorageCatalog};

    use super::*;

    fn catalog_for(root: &Path) -> StorageCatalog {
        let storage_id = StorageId::main();
        let storage: Arc<dyn CacheStorage> = Arc::new(FilesystemStorage::new(root));
        StorageCatalog::new(storage_id.clone(), BTreeMap::from([(storage_id, storage)])).unwrap()
    }

    #[tokio::test]
    async fn db_backed_object_store_reads_bytes_from_configured_storage() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("cache.db");
        let objects_root = temp_dir.path().join("objects-root");
        let object_path = "nar/test.nar";

        let db = SqliteDatabase::open(&db_path).await.unwrap();

        let file_path = objects_root.join(object_path);
        fs::create_dir_all(file_path.parent().unwrap())
            .await
            .unwrap();
        fs::write(&file_path, b"local-bytes").await.unwrap();

        let metadata = BlobMetadata::new("application/octet-stream", Some(11), None, None);
        db.upsert_storage_object(&StorageId::main(), object_path, &metadata)
            .await
            .unwrap();

        let store = DbBackedObjectStore::new(db, catalog_for(&objects_root));

        let loaded = store.get(object_path).await.unwrap().unwrap();

        assert_eq!(loaded.0.content_type, "application/octet-stream");
        assert_eq!(loaded.1, BlobBytes::from_static(b"local-bytes"));
    }

    #[tokio::test]
    async fn db_backed_object_store_returns_none_when_backend_object_missing() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("cache.db");
        let objects_root = temp_dir.path().join("objects-root");
        let object_path = "nar/test.nar";

        let db = SqliteDatabase::open(&db_path).await.unwrap();

        let metadata = BlobMetadata::new("application/octet-stream", Some(11), None, None);
        db.upsert_storage_object(&StorageId::main(), object_path, &metadata)
            .await
            .unwrap();

        let store = DbBackedObjectStore::new(db, catalog_for(&objects_root));

        assert!(store.get(object_path).await.unwrap().is_none());
    }
}
