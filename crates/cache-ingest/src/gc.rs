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
    use std::sync::Arc;

    use tempfile::tempdir;

    use cache_core::narinfo::NarInfo;
    use cache_core::nix::{NixHash, StorePathHash};
    use cache_core::project::ProjectSlug;
    use cache_core::storage::{LocalBackendName, PathObjectKind};
    use cache_db::SqliteDatabase;
    use cache_store::blob::{BlobBytes, BlobMetadata};
    use cache_store::local::{
        FilesystemLocalObjectBackend, LocalObjectBackend, LocalObjectBackendRegistry,
    };

    use super::*;

    async fn setup() -> (
        SqliteDatabase,
        LocalObjectBackendRegistry,
        tempfile::TempDir,
    ) {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("cache.db");
        let objects_root = temp_dir.path().join("objects");

        let db = SqliteDatabase::open(&db_path).await.unwrap();
        let project = ProjectSlug::parse("example_repo").unwrap();
        db.insert_project(&project, "Example Repo", true)
            .await
            .unwrap();

        let mut backends = LocalObjectBackendRegistry::new();
        backends.register(
            LocalBackendName::fs(),
            Arc::new(FilesystemLocalObjectBackend::new(&objects_root)),
        );

        (db, backends, temp_dir)
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

        let store_path = "/nix/store/26xbg1ndr7hbcncrlf9nhx5is2b25d13-hello-2.12.1";
        let store_path_hash = StorePathHash::parse_from_store_path(store_path).unwrap();
        let object_path = "nar/020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz.nar.zst";

        let narinfo = NarInfo {
            store_path: store_path.to_owned(),
            url: object_path.to_owned(),
            compression: "zstd".to_owned(),
            nar_hash: NixHash::Raw(
                "sha256-n4bQgYhMfWWaL+qgxVrQFaO/TxsrC4Is0V1sFbDwCgg=".to_owned(),
            ),
            nar_size: 4,
            references: vec![],
            deriver: None,
            signatures: vec![],
            ca: None,
        };

        db.upsert_path_info(&narinfo).await.unwrap();
        backend
            .put_bytes(object_path, BlobBytes::from_static(b"live"))
            .await
            .unwrap();

        let metadata = BlobMetadata::new("application/octet-stream", Some(4), None, None);
        db.upsert_local_object(object_path, &metadata, &LocalBackendName::fs(), object_path)
            .await
            .unwrap();
        db.link_path_object(&store_path_hash, object_path, PathObjectKind::Nar)
            .await
            .unwrap();
        db.upsert_pin("myapp", None, &store_path_hash, store_path)
            .await
            .unwrap();

        let gc = GcService::new(db.clone(), backends);
        let result = gc.run_local_gc(false).await.unwrap();

        assert_eq!(result.deleted_count, 0);
        assert!(db.get_local_object(object_path).await.unwrap().is_some());
    }
}
