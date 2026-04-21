use std::sync::Arc;

use anyhow::Result;
use bytes::Bytes;
use reqwest::StatusCode;
use tempfile::TempDir;
use uuid::Uuid;

use cache_api::BeginBuildRequest;
use cache_app::build_app_with_parts;
use cache_client::CacheClient;
use cache_core::narinfo::NarInfo;
use cache_core::nix::{NixHash, StoreDir, StorePathHash};
use cache_core::project::ProjectSlug;
use cache_core::signing::NamedSigningKey;
use cache_core::storage::LocalBackendName;
use cache_db::SqliteDatabase;
use cache_store::blob::BlobMetadata;
use cache_store::local::{FilesystemLocalObjectBackend, LocalObjectBackendRegistry};
use cache_store::upstream::{
    InMemoryUpstreamCacheClient, ReqwestUpstreamCacheClient, UpstreamCache,
};

const WRITE_TOKEN: &str = "secret-token";

struct TestApp {
    base_url: String,
    _temp_dir: TempDir,
    _join_handle: tokio::task::JoinHandle<()>,
}

impl TestApp {
    fn cache_client(&self) -> CacheClient {
        CacheClient::new(&self.base_url, WRITE_TOKEN).unwrap()
    }

    fn http_client(&self) -> reqwest::Client {
        reqwest::Client::new()
    }
}

async fn spawn_test_app() -> Result<TestApp> {
    let temp_dir = tempfile::tempdir()?;
    let db_path = temp_dir.path().join("cache.db");
    let objects_root = temp_dir.path().join("objects");

    let db = SqliteDatabase::open(&db_path).await?;

    let mut backends = LocalObjectBackendRegistry::new();
    backends.register(
        LocalBackendName::fs(),
        Arc::new(FilesystemLocalObjectBackend::new(&objects_root)),
    );

    let app = build_app_with_parts(
        db,
        StoreDir::default(),
        vec![
            NamedSigningKey::parse(
                "cache.example.com-1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
            )
            .unwrap(),
        ],
        backends,
        Some(LocalBackendName::fs()),
        Some(WRITE_TOKEN.to_owned()),
        Arc::new(ReqwestUpstreamCacheClient::default()),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let join_handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    Ok(TestApp {
        base_url: format!("http://{address}"),
        _temp_dir: temp_dir,
        _join_handle: join_handle,
    })
}

async fn spawn_test_app_with_prepared_upstream(
    temp_dir: TempDir,
    db: SqliteDatabase,
    upstream_client: InMemoryUpstreamCacheClient,
) -> Result<TestApp> {
    let objects_root = temp_dir.path().join("objects");

    let mut backends = LocalObjectBackendRegistry::new();
    backends.register(
        LocalBackendName::fs(),
        Arc::new(FilesystemLocalObjectBackend::new(&objects_root)),
    );

    let app = build_app_with_parts(
        db,
        StoreDir::default(),
        vec![
            NamedSigningKey::parse(
                "cache.example.com-1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
            )
            .unwrap(),
        ],
        backends,
        Some(LocalBackendName::fs()),
        Some(WRITE_TOKEN.to_owned()),
        Arc::new(upstream_client),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let join_handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    Ok(TestApp {
        base_url: format!("http://{address}"),
        _temp_dir: temp_dir,
        _join_handle: join_handle,
    })
}

fn narinfo_a() -> NarInfo {
    NarInfo {
        store_path: "/nix/store/26xbg1ndr7hbcncrlf9nhx5is2b25d13-hello-2.12.1".to_owned(),
        url: "nar/020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz.nar.zst".to_owned(),
        compression: "zstd".to_owned(),
        nar_hash: NixHash::Raw("sha256-n4bQgYhMfWWaL+qgxVrQFaO/TxsrC4Is0V1sFbDwCgg=".to_owned()),
        nar_size: 9,
        references: vec![],
        deriver: None,
        signatures: vec![],
        ca: None,
    }
}

fn narinfo_b() -> NarInfo {
    NarInfo {
        store_path: "/nix/store/11111111111111111111111111111111-goodbye-1.0".to_owned(),
        url: "nar/1111111111111111111111111111111111111111111111111111.nar.zst".to_owned(),
        compression: "zstd".to_owned(),
        nar_hash: NixHash::Raw("sha256-n4bQgYhMfWWaL+qgxVrQFaO/TxsrC4Is0V1sFbDwCgg=".to_owned()),
        nar_size: 10,
        references: vec![],
        deriver: None,
        signatures: vec![],
        ca: None,
    }
}

fn store_path_hash(narinfo: &NarInfo) -> StorePathHash {
    StorePathHash::parse_from_store_path(&narinfo.store_path).unwrap()
}

async fn publish_single_path(
    client: &CacheClient,
    project: &ProjectSlug,
    ref_name: &str,
    revision: &str,
    narinfo: NarInfo,
    body: Bytes,
) -> Result<()> {
    let begin = client
        .begin_build(BeginBuildRequest {
            project: project.as_str().to_owned(),
            ref_name: ref_name.to_owned(),
            revision: Some(revision.to_owned()),
        })
        .await?;

    let register = client
        .register_paths(&begin.build_id, vec![narinfo.clone()])
        .await?;

    assert_eq!(register.required_uploads.len(), 1);

    client
        .upload_object_bytes(
            &begin.build_id,
            &store_path_hash(&narinfo),
            &narinfo.url,
            body,
        )
        .await?;

    client.finalize_build(&begin.build_id).await?;

    Ok(())
}

#[tokio::test]
async fn aggregate_and_project_routes_serve_published_path() -> Result<()> {
    let app = spawn_test_app().await?;
    let write_client = app.cache_client();
    let read_client = app.http_client();
    let project = ProjectSlug::parse("example_repo").unwrap();
    let narinfo = narinfo_a();
    let hash = store_path_hash(&narinfo);

    write_client
        .upsert_project(&project, "Example Repo", true)
        .await?;

    publish_single_path(
        &write_client,
        &project,
        "refs/heads/main",
        "deadbeef",
        narinfo.clone(),
        Bytes::from_static(b"nar-bytes"),
    )
    .await?;

    let aggregate_narinfo = read_client
        .get(format!("{}/{}.narinfo", app.base_url, hash.as_str()))
        .send()
        .await?;
    assert_eq!(aggregate_narinfo.status(), StatusCode::OK);
    assert_eq!(
        aggregate_narinfo
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .unwrap(),
        "text/x-nix-narinfo"
    );
    let aggregate_narinfo_body = aggregate_narinfo.text().await?;
    assert!(aggregate_narinfo_body.contains(&format!("StorePath: {}", narinfo.store_path)));
    assert!(aggregate_narinfo_body.contains(&format!("URL: {}", narinfo.url)));

    let aggregate_object = read_client
        .get(format!("{}/{}", app.base_url, narinfo.url))
        .send()
        .await?;
    assert_eq!(aggregate_object.status(), StatusCode::OK);
    assert_eq!(
        aggregate_object.bytes().await?,
        Bytes::from_static(b"nar-bytes")
    );

    let project_narinfo = read_client
        .get(format!(
            "{}/p/{}/{}.narinfo",
            app.base_url,
            project.as_str(),
            hash.as_str()
        ))
        .send()
        .await?;
    assert_eq!(project_narinfo.status(), StatusCode::OK);

    let project_object = read_client
        .get(format!(
            "{}/p/{}/{}",
            app.base_url,
            project.as_str(),
            narinfo.url
        ))
        .send()
        .await?;
    assert_eq!(project_object.status(), StatusCode::OK);
    assert_eq!(
        project_object.bytes().await?,
        Bytes::from_static(b"nar-bytes")
    );

    Ok(())
}

#[tokio::test]
async fn project_scoped_pin_keeps_old_path_visible_after_ref_advances() -> Result<()> {
    let app = spawn_test_app().await?;
    let write_client = app.cache_client();
    let read_client = app.http_client();
    let project = ProjectSlug::parse("example_repo").unwrap();

    let narinfo_first = narinfo_a();
    let narinfo_second = narinfo_b();

    let first_hash = store_path_hash(&narinfo_first);
    let second_hash = store_path_hash(&narinfo_second);

    write_client
        .upsert_project(&project, "Example Repo", true)
        .await?;

    publish_single_path(
        &write_client,
        &project,
        "refs/heads/main",
        "rev-a",
        narinfo_first.clone(),
        Bytes::from_static(b"old-bytes"),
    )
    .await?;

    write_client
        .create_pin("release", Some(&project), &narinfo_first.store_path)
        .await?;

    publish_single_path(
        &write_client,
        &project,
        "refs/heads/main",
        "rev-b",
        narinfo_second.clone(),
        Bytes::from_static(b"new-bytes"),
    )
    .await?;

    let aggregate_old = read_client
        .get(format!("{}/{}.narinfo", app.base_url, first_hash.as_str()))
        .send()
        .await?;
    assert_eq!(aggregate_old.status(), StatusCode::NOT_FOUND);

    let project_old = read_client
        .get(format!(
            "{}/p/{}/{}.narinfo",
            app.base_url,
            project.as_str(),
            first_hash.as_str()
        ))
        .send()
        .await?;
    assert_eq!(project_old.status(), StatusCode::OK);

    let project_old_object = read_client
        .get(format!(
            "{}/p/{}/{}",
            app.base_url,
            project.as_str(),
            narinfo_first.url
        ))
        .send()
        .await?;
    assert_eq!(project_old_object.status(), StatusCode::OK);
    assert_eq!(
        project_old_object.bytes().await?,
        Bytes::from_static(b"old-bytes")
    );

    let aggregate_new = read_client
        .get(format!("{}/{}.narinfo", app.base_url, second_hash.as_str()))
        .send()
        .await?;
    assert_eq!(aggregate_new.status(), StatusCode::OK);

    let project_new_object = read_client
        .get(format!(
            "{}/p/{}/{}",
            app.base_url,
            project.as_str(),
            narinfo_second.url
        ))
        .send()
        .await?;
    assert_eq!(project_new_object.status(), StatusCode::OK);
    assert_eq!(
        project_new_object.bytes().await?,
        Bytes::from_static(b"new-bytes")
    );

    Ok(())
}

#[tokio::test]
async fn project_route_serves_upstream_backed_path_without_local_upload() -> Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let db_path = temp_dir.path().join("cache.db");
    let db = SqliteDatabase::open(&db_path).await?;

    let project = ProjectSlug::parse("example_repo").unwrap();
    let narinfo = narinfo_a();
    let hash = store_path_hash(&narinfo);

    let upstream = UpstreamCache::new(
        Uuid::now_v7(),
        "cache.nixos.org",
        "https://cache.nixos.org",
        10,
    );

    db.insert_project(&project, "Example Repo", true).await?;
    db.insert_upstream_cache(&upstream, true).await?;
    db.link_project_upstream(&project, upstream.id).await?;

    let mut upstream_client = InMemoryUpstreamCacheClient::new();
    upstream_client.insert_object(
        upstream.id,
        narinfo.url.clone(),
        BlobMetadata::new("application/octet-stream", Some(12), None, None),
        Bytes::from_static(b"upstream-nar"),
    );

    let app = spawn_test_app_with_prepared_upstream(temp_dir, db, upstream_client).await?;

    let write_client = app.cache_client();
    let read_client = app.http_client();

    let begin = write_client
        .begin_build(BeginBuildRequest {
            project: project.as_str().to_owned(),
            ref_name: "refs/heads/main".to_owned(),
            revision: Some("deadbeef".to_owned()),
        })
        .await?;

    let register = write_client
        .register_paths(&begin.build_id, vec![narinfo.clone()])
        .await?;

    assert_eq!(register.required_uploads.len(), 0);

    write_client.finalize_build(&begin.build_id).await?;

    let project_narinfo = read_client
        .get(format!(
            "{}/p/{}/{}.narinfo",
            app.base_url,
            project.as_str(),
            hash.as_str()
        ))
        .send()
        .await?;
    assert_eq!(project_narinfo.status(), StatusCode::OK);
    assert_eq!(
        project_narinfo
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .unwrap(),
        "text/x-nix-narinfo"
    );
    let project_narinfo_body = project_narinfo.text().await?;
    assert!(project_narinfo_body.contains(&format!("StorePath: {}", narinfo.store_path)));
    assert!(project_narinfo_body.contains(&format!("URL: {}", narinfo.url)));

    let project_object = read_client
        .get(format!(
            "{}/p/{}/{}",
            app.base_url,
            project.as_str(),
            narinfo.url
        ))
        .send()
        .await?;
    assert_eq!(project_object.status(), StatusCode::OK);
    assert_eq!(
        project_object.bytes().await?,
        Bytes::from_static(b"upstream-nar")
    );

    Ok(())
}
