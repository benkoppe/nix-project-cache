use anyhow::{Context as _, Result};

use cache_api::RunGcResponse;
use cache_db::SqliteDatabase;
use cache_store::StorageCatalog;

const DEFAULT_GC_GRACE_PERIOD_SECONDS: i64 = 6 * 60 * 60;

#[derive(Clone)]
pub struct GcService {
    db: SqliteDatabase,
    storage_catalog: StorageCatalog,
}

impl GcService {
    pub fn new(db: SqliteDatabase, storage_catalog: StorageCatalog) -> Self {
        Self {
            db,
            storage_catalog,
        }
    }

    pub async fn run_local_gc(&self, dry_run: bool) -> Result<RunGcResponse> {
        self.run_local_gc_with_grace_period(dry_run, DEFAULT_GC_GRACE_PERIOD_SECONDS)
            .await
    }

    pub async fn run_local_gc_with_grace_period(
        &self,
        dry_run: bool,
        grace_period_seconds: i64,
    ) -> Result<RunGcResponse> {
        if dry_run {
            let stale_objects = self.db.list_stale_storage_objects().await?;
            return Ok(RunGcResponse {
                deleted_count: stale_objects.len(),
                deleted_objects: stale_objects
                    .into_iter()
                    .map(|object| object.object_path)
                    .collect(),
            });
        }

        self.db
            .clear_deleted_state_for_live_storage_objects()
            .await?;
        self.db.mark_stale_storage_objects().await?;

        let ready_objects = self
            .db
            .list_storage_objects_ready_for_deletion(grace_period_seconds)
            .await?;

        let mut deleted_rows = Vec::with_capacity(ready_objects.len());
        let mut deleted_paths = Vec::with_capacity(ready_objects.len());

        for object in ready_objects {
            let storage = self
                .storage_catalog
                .storage(&object.storage_id)
                .with_context(|| {
                    format!(
                        "resolving storage backend {} for {}",
                        object.storage_id, object.object_path
                    )
                })?;

            storage.delete(&object.object_path).await.with_context(|| {
                format!(
                    "deleting object {} from storage {}",
                    object.object_path, object.storage_id
                )
            })?;

            deleted_paths.push(object.object_path.clone());
            deleted_rows.push(object);
        }

        self.db.delete_storage_objects(&deleted_rows).await?;

        Ok(RunGcResponse {
            deleted_count: deleted_paths.len(),
            deleted_objects: deleted_paths,
        })
    }
}

#[cfg(test)]
mod tests {
    use cache_core::storage::{PathObjectKind, StorageId};
    use cache_db::SqliteDatabase;
    use cache_store::StorageCatalog;
    use cache_store::blob::{BlobBytes, BlobMetadata};
    use cache_test_utils::{TestDatabase, filesystem_storage_in, hello_path};

    use super::*;

    async fn setup() -> (SqliteDatabase, StorageCatalog, tempfile::TempDir) {
        let fixture = TestDatabase::new().await.unwrap();
        fixture.insert_example_project().await.unwrap();

        let storage_catalog = filesystem_storage_in(&fixture.temp_dir);

        (fixture.db, storage_catalog, fixture.temp_dir)
    }

    #[tokio::test]
    async fn dry_run_reports_stale_objects_without_deleting() {
        let (db, storage_catalog, _tmp) = setup().await;
        let gc = GcService::new(db.clone(), storage_catalog);

        let metadata = BlobMetadata::new("application/octet-stream", Some(4), None, None);
        let path = "nar/stale.nar.zst";

        db.upsert_storage_object(&StorageId::main(), path, &metadata)
            .await
            .unwrap();

        let result = gc.run_local_gc(true).await.unwrap();

        assert_eq!(result.deleted_count, 1);
        assert_eq!(result.deleted_objects, vec![path.to_owned()]);
        assert!(!db.list_storage_objects(path).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn default_gc_grace_marks_but_does_not_delete() {
        let (db, storage_catalog, _tmp) = setup().await;
        let storage = storage_catalog.default_storage().unwrap();

        let path = "nar/stale.nar.zst";
        storage
            .put_bytes(path, BlobBytes::from_static(b"dead"))
            .await
            .unwrap();

        let metadata = BlobMetadata::new("application/octet-stream", Some(4), None, None);
        db.upsert_storage_object(&StorageId::main(), path, &metadata)
            .await
            .unwrap();

        let gc = GcService::new(db.clone(), storage_catalog);
        let result = gc.run_local_gc(false).await.unwrap();

        assert_eq!(result.deleted_count, 0);
        assert!(storage.contains(path).await.unwrap());

        let ready = db.list_storage_objects_ready_for_deletion(0).await.unwrap();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].storage_id, StorageId::main());
        assert_eq!(ready[0].object_path, path);
    }

    #[tokio::test]
    async fn gc_deletes_unreachable_local_object_when_grace_is_zero() {
        let (db, storage_catalog, _tmp) = setup().await;
        let storage = storage_catalog.default_storage().unwrap();

        let path = "nar/stale.nar.zst";
        storage
            .put_bytes(path, BlobBytes::from_static(b"dead"))
            .await
            .unwrap();

        let metadata = BlobMetadata::new("application/octet-stream", Some(4), None, None);
        db.upsert_storage_object(&StorageId::main(), path, &metadata)
            .await
            .unwrap();

        let gc = GcService::new(db.clone(), storage_catalog.clone());
        let result = gc.run_local_gc_with_grace_period(false, 0).await.unwrap();

        assert_eq!(result.deleted_count, 1);
        assert!(db.list_storage_objects(path).await.unwrap().is_empty());
        assert!(!storage.contains(path).await.unwrap());
    }

    #[tokio::test]
    async fn pinned_path_keeps_local_object_live() {
        let (db, storage_catalog, _tmp) = setup().await;
        let storage = storage_catalog.default_storage().unwrap();

        let path = hello_path();
        let hash = path.hash();
        let narinfo = path.narinfo();
        let object_path = path.url();

        db.upsert_path_info(&narinfo).await.unwrap();

        storage
            .put_bytes(object_path, BlobBytes::from_static(b"live"))
            .await
            .unwrap();

        let metadata = BlobMetadata::new("application/octet-stream", Some(4), None, None);
        db.upsert_storage_object(&StorageId::main(), object_path, &metadata)
            .await
            .unwrap();

        db.link_path_object(&hash, object_path, PathObjectKind::Nar)
            .await
            .unwrap();

        db.upsert_pin("myapp", None, &hash, &narinfo.store_path)
            .await
            .unwrap();

        let gc = GcService::new(db.clone(), storage_catalog);
        let result = gc.run_local_gc(false).await.unwrap();

        assert_eq!(result.deleted_count, 0);
        assert!(
            !db.list_storage_objects(object_path)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
