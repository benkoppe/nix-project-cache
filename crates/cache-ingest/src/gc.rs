use anyhow::{Context as _, Result};

use cache_api::RunGcResponse;
use cache_db::SqliteDatabase;
use cache_store::local::LocalObjectBackendRegistry;

const DEFAULT_GC_GRACE_PERIOD_SECONDS: i64 = 6 * 60 * 60;

#[derive(Clone)]
pub struct GcService {
    db: SqliteDatabase,
    local_backends: LocalObjectBackendRegistry,
}

impl GcService {
    pub fn new(db: SqliteDatabase, local_backends: LocalObjectBackendRegistry) -> Self {
        Self { db, local_backends }
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
            let stale_paths = self.db.list_stale_local_object_paths().await?;
            return Ok(RunGcResponse {
                deleted_count: stale_paths.len(),
                deleted_objects: stale_paths,
            });
        }

        self.db.clear_deleted_state_for_live_local_objects().await?;
        self.db.mark_stale_local_objects().await?;

        let ready_paths = self
            .db
            .list_local_objects_ready_for_deletion(grace_period_seconds)
            .await?;

        let mut deleted = Vec::with_capacity(ready_paths.len());

        for object_path in ready_paths {
            let Some(record) = self.db.get_local_object(&object_path).await? else {
                continue;
            };

            let backend = self
                .local_backends
                .require(&record.storage_backend)
                .with_context(|| {
                    format!(
                        "resolving backend {} for {}",
                        record.storage_backend, object_path
                    )
                })?;

            backend
                .delete(&record.storage_key)
                .await
                .with_context(|| format!("deleting backend object {}", object_path))?;

            deleted.push(object_path);
        }

        self.db.delete_local_objects_by_path(&deleted).await?;

        Ok(RunGcResponse {
            deleted_count: deleted.len(),
            deleted_objects: deleted,
        })
    }
}

#[cfg(test)]
mod tests {
    use cache_core::storage::{LocalBackendName, PathObjectKind};
    use cache_db::SqliteDatabase;
    use cache_store::blob::{BlobBytes, BlobMetadata};
    use cache_store::local::{
        FilesystemLocalObjectBackend, LocalObjectBackend, LocalObjectBackendRegistry,
    };
    use cache_test_utils::{TestDatabase, filesystem_backends_in, hello_path};

    use super::*;

    async fn setup() -> (
        SqliteDatabase,
        LocalObjectBackendRegistry,
        tempfile::TempDir,
    ) {
        let fixture = TestDatabase::new().await.unwrap();
        fixture.insert_example_project().await.unwrap();

        let backends = filesystem_backends_in(&fixture.temp_dir);

        (fixture.db, backends, fixture.temp_dir)
    }

    #[tokio::test]
    async fn dry_run_reports_stale_objects_without_deleting() {
        let (db, backends, _tmp) = setup().await;
        let gc = GcService::new(db.clone(), backends);

        let metadata = BlobMetadata::new("application/octet-stream", Some(4), None, None);
        db.upsert_local_object(
            "nar/stale.nar.zst",
            &metadata,
            &LocalBackendName::fs(),
            "nar/stale.nar.zst",
        )
        .await
        .unwrap();

        let result = gc.run_local_gc(true).await.unwrap();

        assert_eq!(result.deleted_count, 1);
        assert_eq!(result.deleted_objects, vec!["nar/stale.nar.zst".to_owned()]);
        assert!(
            db.get_local_object("nar/stale.nar.zst")
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn default_gc_grace_marks_but_does_not_delete() {
        let (db, backends, _tmp) = setup().await;
        let backend = backends.require(&LocalBackendName::fs()).unwrap();

        backend
            .put_bytes("nar/stale.nar.zst", BlobBytes::from_static(b"dead"))
            .await
            .unwrap();

        let metadata = BlobMetadata::new("application/octet-stream", Some(4), None, None);
        db.upsert_local_object(
            "nar/stale.nar.zst",
            &metadata,
            &LocalBackendName::fs(),
            "nar/stale.nar.zst",
        )
        .await
        .unwrap();

        let gc = GcService::new(db.clone(), backends.clone());
        let result = gc.run_local_gc(false).await.unwrap();

        assert_eq!(result.deleted_count, 0);
        assert!(
            db.get_local_object("nar/stale.nar.zst")
                .await
                .unwrap()
                .is_some()
        );

        let ready = db.list_local_objects_ready_for_deletion(0).await.unwrap();
        assert_eq!(ready, vec!["nar/stale.nar.zst".to_owned()]);
    }

    #[tokio::test]
    async fn gc_deletes_unreachable_local_object_when_grace_is_zero() {
        let (db, backends, tmp) = setup().await;

        let backend = backends.require(&LocalBackendName::fs()).unwrap();
        backend
            .put_bytes("nar/stale.nar.zst", BlobBytes::from_static(b"dead"))
            .await
            .unwrap();

        let metadata = BlobMetadata::new("application/octet-stream", Some(4), None, None);
        db.upsert_local_object(
            "nar/stale.nar.zst",
            &metadata,
            &LocalBackendName::fs(),
            "nar/stale.nar.zst",
        )
        .await
        .unwrap();

        let gc = GcService::new(db.clone(), backends.clone());
        let result = gc.run_local_gc_with_grace_period(false, 0).await.unwrap();

        assert_eq!(result.deleted_count, 1);
        assert!(
            db.get_local_object("nar/stale.nar.zst")
                .await
                .unwrap()
                .is_none()
        );

        let fs_backend = FilesystemLocalObjectBackend::new(tmp.path().join("objects"));
        assert!(!fs_backend.contains("nar/stale.nar.zst").await.unwrap());
    }

    #[tokio::test]
    async fn pinned_path_keeps_local_object_live() {
        let (db, backends, _tmp) = setup().await;
        let backend = backends.require(&LocalBackendName::fs()).unwrap();

        let path = hello_path();
        let hash = path.hash();
        let narinfo = path.narinfo();

        db.upsert_path_info(&narinfo).await.unwrap();
        backend
            .put_bytes(path.url(), BlobBytes::from_static(b"live"))
            .await
            .unwrap();

        let metadata = BlobMetadata::new("application/octet-stream", Some(4), None, None);
        db.upsert_local_object(path.url(), &metadata, &LocalBackendName::fs(), path.url())
            .await
            .unwrap();
        db.link_path_object(&hash, path.url(), PathObjectKind::Nar)
            .await
            .unwrap();
        db.upsert_pin("myapp", None, &hash, &narinfo.store_path)
            .await
            .unwrap();

        let gc = GcService::new(db.clone(), backends);
        let result = gc.run_local_gc(false).await.unwrap();

        assert_eq!(result.deleted_count, 0);
        assert!(db.get_local_object(path.url()).await.unwrap().is_some());
    }
}
