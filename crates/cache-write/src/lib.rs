pub mod handlers;
pub mod router;
pub mod state;

pub use router::write_router;
pub use state::WriteAppState;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode, header};
    use bytes::Bytes;
    use http_body_util::BodyExt as _;
    use serde_json::json;
    use tempfile::TempDir;
    use tower::util::ServiceExt as _;

    use cache_api::{BeginBuildResponse, PinInfo, ProjectInfo, RunGcResponse};
    use cache_auth::StaticTokenAuthorizer;
    use cache_core::narinfo::NarInfo;
    use cache_core::nix::NixHash;
    use cache_core::project::ProjectSlug;
    use cache_core::storage::LocalBackendName;
    use cache_db::SqliteDatabase;
    use cache_ingest::{GcService, IngestService};
    use cache_store::local::{
        FilesystemLocalObjectBackend, InMemoryLocalObjectStore, LocalObjectBackendRegistry,
    };
    use cache_store::upstream::InMemoryUpstreamCacheClient;

    use super::*;

    const WRITE_TOKEN: &str = "secret-token";

    async fn build_test_app() -> (axum::Router, TempDir) {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("cache.db");
        let objects_root = temp_dir.path().join("objects");

        let db = SqliteDatabase::open(&db_path).await.unwrap();
        let project = ProjectSlug::parse("example_repo").unwrap();
        db.insert_project(&project, "Example Repo", true)
            .await
            .unwrap();

        let fs_backend = Arc::new(FilesystemLocalObjectBackend::new(&objects_root));
        let mut backends = LocalObjectBackendRegistry::new();
        backends.register(LocalBackendName::fs(), fs_backend);

        let ingest_service = IngestService::new(
            db.clone(),
            Arc::new(InMemoryLocalObjectStore::new()),
            backends.clone(),
            Some(LocalBackendName::fs()),
            Arc::new(InMemoryUpstreamCacheClient::new()),
        );
        let gc_service = GcService::new(db.clone(), backends);

        let state = WriteAppState::new(
            db,
            Arc::new(ingest_service),
            Arc::new(gc_service),
            Arc::new(StaticTokenAuthorizer::new(Some(WRITE_TOKEN.to_owned()))),
        );

        (write_router(state), temp_dir)
    }

    async fn body_bytes(response: axum::response::Response) -> Bytes {
        response.into_body().collect().await.unwrap().to_bytes()
    }

    async fn body_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
        serde_json::from_slice(&body_bytes(response).await).unwrap()
    }

    fn sample_narinfo() -> NarInfo {
        NarInfo {
            store_path: "/nix/store/26xbg1ndr7hbcncrlf9nhx5is2b25d13-hello-2.12.1".to_owned(),
            url: "nar/020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz.nar.zst".to_owned(),
            compression: "zstd".to_owned(),
            nar_hash: NixHash::Raw(
                "sha256-n4bQgYhMfWWaL+qgxVrQFaO/TxsrC4Is0V1sFbDwCgg=".to_owned(),
            ),
            nar_size: 226560,
            references: vec![],
            deriver: None,
            signatures: vec![],
            ca: None,
        }
    }

    #[tokio::test]
    async fn write_api_rejects_missing_token() {
        let (app, _tmp) = build_test_app().await;

        let request = Request::builder()
            .method(Method::POST)
            .uri("/api/builds")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "project": "example_repo",
                    "ref_name": "main",
                    "revision": "deadbeef"
                }))
                .unwrap(),
            ))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn begin_build_route_returns_build_id() {
        let (app, _tmp) = build_test_app().await;

        let request = Request::builder()
            .method(Method::POST)
            .uri("/api/builds")
            .header(header::AUTHORIZATION, format!("Bearer {}", WRITE_TOKEN))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "project": "example_repo",
                    "ref_name": "main",
                    "revision": "deadbeef"
                }))
                .unwrap(),
            ))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body: BeginBuildResponse = body_json(response).await;
        assert!(!body.build_id.is_empty());
    }

    #[tokio::test]
    async fn create_and_list_project_routes_work() {
        let (app, _tmp) = build_test_app().await;

        let create_request = Request::builder()
            .method(Method::POST)
            .uri("/api/projects")
            .header(header::AUTHORIZATION, format!("Bearer {}", WRITE_TOKEN))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "slug": "another_repo",
                    "display_name": "Another Repo",
                    "public": false
                }))
                .unwrap(),
            ))
            .unwrap();

        let create_response = app.clone().oneshot(create_request).await.unwrap();
        assert_eq!(create_response.status(), StatusCode::NO_CONTENT);

        let list_request = Request::builder()
            .method(Method::GET)
            .uri("/api/projects")
            .header(header::AUTHORIZATION, format!("Bearer {}", WRITE_TOKEN))
            .body(Body::empty())
            .unwrap();

        let list_response = app.oneshot(list_request).await.unwrap();
        assert_eq!(list_response.status(), StatusCode::OK);

        let body: Vec<ProjectInfo> = body_json(list_response).await;
        assert!(body.iter().any(|project| project.slug == "another_repo"));
    }

    #[tokio::test]
    async fn create_and_list_pin_routes_work() {
        let (app, tmp) = build_test_app().await;
        let db = SqliteDatabase::open(&tmp.path().join("cache.db"))
            .await
            .unwrap();

        db.upsert_path_info(&sample_narinfo()).await.unwrap();

        let create_request = Request::builder()
            .method(Method::POST)
            .uri("/api/pins/release")
            .header(header::AUTHORIZATION, format!("Bearer {}", WRITE_TOKEN))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "project": "example_repo",
                    "store_path": "/nix/store/26xbg1ndr7hbcncrlf9nhx5is2b25d13-hello-2.12.1"
                }))
                .unwrap(),
            ))
            .unwrap();

        let create_response = app.clone().oneshot(create_request).await.unwrap();
        assert_eq!(create_response.status(), StatusCode::NO_CONTENT);

        let list_request = Request::builder()
            .method(Method::GET)
            .uri("/api/pins?project=example_repo")
            .header(header::AUTHORIZATION, format!("Bearer {}", WRITE_TOKEN))
            .body(Body::empty())
            .unwrap();

        let list_response = app.clone().oneshot(list_request).await.unwrap();
        assert_eq!(list_response.status(), StatusCode::OK);

        let body: Vec<PinInfo> = body_json(list_response).await;
        assert!(body.iter().any(|pin| pin.name == "release"));

        let delete_request = Request::builder()
            .method(Method::DELETE)
            .uri("/api/pins/release?project=example_repo")
            .header(header::AUTHORIZATION, format!("Bearer {}", WRITE_TOKEN))
            .body(Body::empty())
            .unwrap();

        let delete_response = app.oneshot(delete_request).await.unwrap();
        assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn gc_route_returns_response() {
        let (app, _tmp) = build_test_app().await;

        let request = Request::builder()
            .method(Method::POST)
            .uri("/api/gc")
            .header(header::AUTHORIZATION, format!("Bearer {}", WRITE_TOKEN))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "dry_run": true
                }))
                .unwrap(),
            ))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body: RunGcResponse = body_json(response).await;
        assert_eq!(body.deleted_count, 0);
    }
}
