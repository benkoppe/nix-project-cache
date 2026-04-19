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

    use cache_api::BeginBuildResponse;
    use cache_auth::StaticTokenAuthorizer;
    use cache_core::project::ProjectSlug;
    use cache_db::SqliteDatabase;
    use cache_ingest::IngestService;
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
        backends.register("fs", fs_backend);

        let ingest_service = IngestService::new(
            db,
            Arc::new(InMemoryLocalObjectStore::new()),
            backends,
            Arc::new(InMemoryUpstreamCacheClient::new()),
        );

        let state = WriteAppState {
            ingest_service: Arc::new(ingest_service),
            authorizer: Arc::new(StaticTokenAuthorizer::new(Some(WRITE_TOKEN.to_owned()))),
        };

        (write_router(state), temp_dir)
    }

    async fn body_bytes(response: axum::response::Response) -> Bytes {
        response.into_body().collect().await.unwrap().to_bytes()
    }

    async fn body_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
        serde_json::from_slice(&body_bytes(response).await).unwrap()
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
}
