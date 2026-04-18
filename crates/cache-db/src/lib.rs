mod models;
mod objects;
mod paths;
mod pool;
mod projects;
mod upstreams;

pub use objects::LocalObjectRecord;
pub use pool::SqliteDatabase;

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use cache_core::narinfo::NarInfo;
    use cache_core::nix::{NixHash, StorePathHash};
    use cache_core::project::ProjectSlug;
    use cache_store::blob::BlobMetadata;
    use cache_store::upstream::UpstreamCache;

    use super::SqliteDatabase;

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
        db.link_path_to_project(&project, &hash).await.unwrap();

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
        db.link_path_to_project(&private_project, &hash)
            .await
            .unwrap();

        assert!(db.get_aggregate_narinfo(&hash).await.unwrap().is_none());

        db.link_path_to_project(&public_project, &hash)
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
    }

    #[tokio::test]
    async fn local_object_round_trips() {
        let (db, _tmp) = SqliteDatabase::open_temp_for_tests().await.unwrap();

        let metadata = BlobMetadata::new("application/octet-stream", Some(9), None, None);
        db.upsert_local_object(
            "nar/020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz.nar.zst",
            &metadata,
            "fs",
            "objects/020a",
        )
        .await
        .unwrap();

        let loaded = db
            .get_local_object("nar/020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz.nar.zst")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(loaded.metadata.content_type, "application/octet-stream");
        assert_eq!(loaded.metadata.content_length, Some(9));
        assert_eq!(loaded.storage_backend, "fs");
        assert_eq!(loaded.storage_key, "objects/020a");
    }
}
