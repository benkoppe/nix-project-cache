mod access_tokens;
mod builds;
mod gc;
mod models;
mod objects;
mod paths;
mod pins;
mod pool;
mod project_oidc_identities;
mod project_signing_keys;
mod projects;
mod retention;
mod upstreams;

pub use models::{
    AccessTokenRecord, BuildContextRecord, BuildRecord, BuildStatus, PinRecord,
    ProjectOidcIdentityRecord, ProjectRecord, ProjectRetentionPolicyRecord,
    ProjectRetentionRuleRecord, ProjectSigningKeyRecord, UpstreamCacheRecord,
};
pub use objects::StorageObjectRecord;
pub use pool::SqliteDatabase;
pub use retention::default_retention_rules;

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use cache_core::narinfo::NarInfo;
    use cache_core::nix::{NixHash, StorePathHash};
    use cache_core::project::ProjectSlug;
    use cache_core::storage::{PathObjectKind, StorageId};
    use cache_store::blob::BlobMetadata;
    use cache_store::upstream::UpstreamCache;

    use super::{BuildStatus, SqliteDatabase};

    fn sample_narinfo() -> NarInfo {
        NarInfo {
            store_path: "/nix/store/26xbg1ndr7hbcncrlf9nhx5is2b25d13-hello-2.12.1".to_owned(),
            url: "nar/020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz.nar.zst".to_owned(),
            compression: "zstd".to_owned(),
            nar_hash: NixHash::Raw(
                "sha256-n4bQgYhMfWWaL+qgxVrQFaO/TxsrC4Is0V1sFbDwCgg=".to_owned(),
            ),
            nar_size: 226560,
            references: vec!["/nix/store/aaa-package".to_owned()],
            deriver: Some("/nix/store/example.drv".to_owned()),
            signatures: vec!["cache.example.com-1:abc".to_owned()],
            ca: None,
        }
    }

    fn sample_hash() -> StorePathHash {
        StorePathHash::parse_from_store_path(
            "/nix/store/26xbg1ndr7hbcncrlf9nhx5is2b25d13-hello-2.12.1",
        )
        .unwrap()
    }

    #[tokio::test]
    async fn upsert_and_get_project_narinfo_round_trips() {
        let (db, _tmp) = SqliteDatabase::open_temp_for_tests().await.unwrap();
        let project = ProjectSlug::parse("example_repo").unwrap();
        let narinfo = sample_narinfo();
        let hash = sample_hash();

        db.insert_project(&project, "Example Repo", true)
            .await
            .unwrap();
        db.upsert_path_info(&narinfo).await.unwrap();

        let build = db.begin_build(&project, "main", None).await.unwrap();
        db.attach_build_path(build.id, &hash).await.unwrap();
        db.publish_build_to_ref(&project, "main", build.id)
            .await
            .unwrap();

        let loaded = db
            .get_project_narinfo(&project, &hash)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(loaded.store_path, narinfo.store_path);
        assert_eq!(loaded.references, narinfo.references);
        assert_eq!(loaded.signatures, narinfo.signatures);
    }

    #[tokio::test]
    async fn aggregate_only_sees_public_projects() {
        let (db, _tmp) = SqliteDatabase::open_temp_for_tests().await.unwrap();
        let public_project = ProjectSlug::parse("public_repo").unwrap();
        let private_project = ProjectSlug::parse("private_repo").unwrap();
        let narinfo = sample_narinfo();
        let hash = sample_hash();

        db.insert_project(&public_project, "Public", true)
            .await
            .unwrap();
        db.insert_project(&private_project, "Private", false)
            .await
            .unwrap();
        db.upsert_path_info(&narinfo).await.unwrap();

        let private_build = db
            .begin_build(&private_project, "main", None)
            .await
            .unwrap();
        db.attach_build_path(private_build.id, &hash).await.unwrap();
        db.publish_build_to_ref(&private_project, "main", private_build.id)
            .await
            .unwrap();

        assert!(db.get_aggregate_narinfo(&hash).await.unwrap().is_none());

        let public_build = db.begin_build(&public_project, "main", None).await.unwrap();
        db.attach_build_path(public_build.id, &hash).await.unwrap();
        db.publish_build_to_ref(&public_project, "main", public_build.id)
            .await
            .unwrap();

        assert!(db.get_aggregate_narinfo(&hash).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn upstream_links_round_trip() {
        let (db, _tmp) = SqliteDatabase::open_temp_for_tests().await.unwrap();
        let project = ProjectSlug::parse("example_repo").unwrap();
        let upstream = UpstreamCache::new(
            Uuid::now_v7(),
            "cache.nixos.org",
            "https://cache.nixos.org",
            10,
        );

        db.insert_project(&project, "Example Repo", true)
            .await
            .unwrap();
        db.insert_upstream_cache(&upstream, true).await.unwrap();
        db.link_project_upstream(&project, upstream.id)
            .await
            .unwrap();

        let all = db.list_enabled_upstreams().await.unwrap();
        let project_upstreams = db
            .list_enabled_upstreams_for_project(&project)
            .await
            .unwrap();

        assert_eq!(all.len(), 1);
        assert_eq!(project_upstreams.len(), 1);
        assert_eq!(project_upstreams[0].id, upstream.id);
        assert!(project_upstreams[0].enabled);
    }

    #[tokio::test]
    async fn storage_object_round_trips() {
        let (db, _tmp) = SqliteDatabase::open_temp_for_tests().await.unwrap();

        let storage_id = StorageId::main();
        let object_path = "nar/020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz.nar.zst";
        let metadata = BlobMetadata::new("application/octet-stream", Some(9), None, None);

        db.upsert_storage_object(&storage_id, object_path, &metadata)
            .await
            .unwrap();

        let loaded = db.list_storage_objects(object_path).await.unwrap();

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].storage_id, storage_id);
        assert_eq!(loaded[0].metadata.content_type, "application/octet-stream");
        assert_eq!(loaded[0].metadata.content_length, Some(9));
    }

    #[tokio::test]
    async fn begin_build_and_get_context_round_trip() {
        let (db, _tmp) = SqliteDatabase::open_temp_for_tests().await.unwrap();
        let project = ProjectSlug::parse("example_repo").unwrap();

        db.insert_project(&project, "Example Repo", true)
            .await
            .unwrap();

        let build = db
            .begin_build(&project, "main", Some("deadbeef"))
            .await
            .unwrap();

        let context = db.get_build_context(build.id).await.unwrap().unwrap();

        assert_eq!(context.project_slug, project);
        assert_eq!(context.ref_name, "main");
        assert_eq!(context.revision.as_deref(), Some("deadbeef"));
        assert_eq!(context.status, BuildStatus::Pending);
    }

    #[tokio::test]
    async fn refresh_project_paths_from_refs_uses_union_of_active_refs() {
        let (db, _tmp) = SqliteDatabase::open_temp_for_tests().await.unwrap();
        let project = ProjectSlug::parse("example_repo").unwrap();

        db.insert_project(&project, "Example Repo", true)
            .await
            .unwrap();

        let narinfo_a = sample_narinfo();
        let mut narinfo_b = sample_narinfo();
        narinfo_b.store_path = "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-world-1.0".to_owned();

        let hash_a = sample_hash();
        let hash_b = StorePathHash::parse_from_store_path(&narinfo_b.store_path).unwrap();

        db.upsert_path_info(&narinfo_a).await.unwrap();
        db.upsert_path_info(&narinfo_b).await.unwrap();

        let build_a = db.begin_build(&project, "main", None).await.unwrap();
        db.attach_build_path(build_a.id, &hash_a).await.unwrap();
        db.publish_build_to_ref(&project, "main", build_a.id)
            .await
            .unwrap();

        let build_b = db.begin_build(&project, "pr-123", None).await.unwrap();
        db.attach_build_path(build_b.id, &hash_b).await.unwrap();
        db.publish_build_to_ref(&project, "pr-123", build_b.id)
            .await
            .unwrap();

        assert!(
            db.get_project_narinfo(&project, &hash_a)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            db.get_project_narinfo(&project, &hash_b)
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn link_path_object_succeeds_for_existing_path() {
        let (db, _tmp) = SqliteDatabase::open_temp_for_tests().await.unwrap();
        let narinfo = sample_narinfo();
        let hash = sample_hash();
        let object_path = "nar/020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz.nar.zst";

        db.upsert_path_info(&narinfo).await.unwrap();

        db.link_path_object(&hash, object_path, PathObjectKind::Nar)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn project_pin_makes_path_visible_in_project_view() {
        let (db, _tmp) = SqliteDatabase::open_temp_for_tests().await.unwrap();
        let project = ProjectSlug::parse("example_repo").unwrap();
        let narinfo = sample_narinfo();
        let hash = sample_hash();

        db.insert_project(&project, "Example Repo", true)
            .await
            .unwrap();
        db.upsert_path_info(&narinfo).await.unwrap();
        db.upsert_pin("hello-1.0", Some(&project), &hash, &narinfo.store_path)
            .await
            .unwrap();

        assert!(
            db.get_project_narinfo(&project, &hash)
                .await
                .unwrap()
                .is_some()
        );
        assert!(db.get_aggregate_narinfo(&hash).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn global_pin_makes_path_visible_in_aggregate_view() {
        let (db, _tmp) = SqliteDatabase::open_temp_for_tests().await.unwrap();
        let narinfo = sample_narinfo();
        let hash = sample_hash();

        db.upsert_path_info(&narinfo).await.unwrap();
        db.upsert_pin("hello-1.0", None, &hash, &narinfo.store_path)
            .await
            .unwrap();

        assert!(db.get_aggregate_narinfo(&hash).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn upsert_storage_object_clears_tombstones() {
        let (db, _tmp) = SqliteDatabase::open_temp_for_tests().await.unwrap();

        let storage_id = StorageId::main();
        let storage_id_str = storage_id.as_str();
        let object_path = "nar/test.nar.zst";
        let metadata = BlobMetadata::new("application/octet-stream", Some(9), None, None);

        db.upsert_storage_object(&storage_id, object_path, &metadata)
            .await
            .unwrap();

        sqlx::query!(
            r#"
            UPDATE storage_objects
            SET
                deleted_at = '2026-04-20T00:00:00.000Z',
                first_deleted_at = '2026-04-20T00:00:00.000Z'
            WHERE storage_id = ?
              AND object_path = ?
        "#,
            storage_id_str,
            object_path,
        )
        .execute(db.pool())
        .await
        .unwrap();

        db.upsert_storage_object(&storage_id, object_path, &metadata)
            .await
            .unwrap();

        let row = sqlx::query!(
            r#"
            SELECT deleted_at, first_deleted_at
            FROM storage_objects
            WHERE storage_id = ?
              AND object_path = ?
        "#,
            storage_id_str,
            object_path,
        )
        .fetch_one(db.pool())
        .await
        .unwrap();

        assert!(row.deleted_at.is_none());
        assert!(row.first_deleted_at.is_none());
    }

    #[tokio::test]
    async fn project_retention_policy_defaults_and_override_round_trip() {
        let (db, _tmp) = SqliteDatabase::open_temp_for_tests().await.unwrap();
        let project = ProjectSlug::parse("example_repo").unwrap();

        db.insert_project(&project, "Example Repo", true)
            .await
            .unwrap();

        let default_policy = db.get_project_retention_policy(&project).await.unwrap();
        assert!(default_policy.inherited_default);
        assert_eq!(default_policy.keep_latest_builds_per_ref, 2);
        assert!(
            default_policy
                .rules
                .iter()
                .any(|rule| rule.ref_pattern == "refs/pull/*")
        );

        db.replace_project_retention_policy(
            &project,
            3,
            3600,
            &[crate::ProjectRetentionRuleRecord {
                priority: 10,
                ref_pattern: "*".to_owned(),
                ttl_seconds: Some(7 * 24 * 60 * 60),
                keep_builds: Some(1),
            }],
        )
        .await
        .unwrap();

        let custom_policy = db.get_project_retention_policy(&project).await.unwrap();
        assert!(!custom_policy.inherited_default);
        assert_eq!(custom_policy.keep_latest_builds_per_ref, 3);
        assert_eq!(custom_policy.object_delete_grace_seconds, 3600);
        assert_eq!(custom_policy.rules.len(), 1);

        assert!(db.delete_project_retention_policy(&project).await.unwrap());

        let reset_policy = db.get_project_retention_policy(&project).await.unwrap();
        assert!(reset_policy.inherited_default);
    }

    #[tokio::test]
    async fn project_storage_id_round_trips() {
        let (db, _tmp) = SqliteDatabase::open_temp_for_tests().await.unwrap();

        let project = ProjectSlug::parse("example_repo").unwrap();
        let storage_id = StorageId::new("large").unwrap();

        db.insert_project_with_storage(&project, "Example Repo", true, Some(&storage_id))
            .await
            .unwrap();

        let loaded = db.get_project_by_slug(&project).await.unwrap().unwrap();

        assert_eq!(loaded.storage_id, Some(storage_id.clone()));
        assert_eq!(
            db.get_project_storage_id(&project).await.unwrap(),
            Some(storage_id)
        );
    }

    #[tokio::test]
    async fn gc_retains_latest_builds_for_active_refs_according_to_policy() {
        let (db, _tmp) = SqliteDatabase::open_temp_for_tests().await.unwrap();
        let project = ProjectSlug::parse("example_repo").unwrap();

        db.insert_project(&project, "Example Repo", true)
            .await
            .unwrap();

        let narinfo = sample_narinfo();
        let hash = sample_hash();
        db.upsert_path_info(&narinfo).await.unwrap();

        let old_build = db
            .begin_build(&project, "refs/heads/main", None)
            .await
            .unwrap();
        db.attach_build_path(old_build.id, &hash).await.unwrap();
        db.publish_build_to_ref(&project, "refs/heads/main", old_build.id)
            .await
            .unwrap();

        let new_build = db
            .begin_build(&project, "refs/heads/main", None)
            .await
            .unwrap();
        db.attach_build_path(new_build.id, &hash).await.unwrap();
        db.publish_build_to_ref(&project, "refs/heads/main", new_build.id)
            .await
            .unwrap();

        let retained = db.list_retained_build_ids_for_gc().await.unwrap();

        assert!(retained.contains(&old_build.id.to_string()));
        assert!(retained.contains(&new_build.id.to_string()));
    }
}
