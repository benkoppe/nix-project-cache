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
    use cache_core::storage::LocalBackendName;
    use cache_db::SqliteDatabase;
    use cache_ingest::{GcService, IngestService};
    use cache_store::local::InMemoryLocalObjectStore;
    use cache_store::upstream::InMemoryUpstreamCacheClient;
    use cache_test_utils::{
        EXAMPLE_PROJECT_SLUG, TestDatabase, filesystem_backends_in, hello_path,
    };

    use super::*;

    const WRITE_TOKEN: &str = "secret-token";

    struct WriteTestApp {
        app: axum::Router,
        db: SqliteDatabase,
        _temp_dir: TempDir,
    }

    async fn build_test_app() -> WriteTestApp {
        let fixture = TestDatabase::new().await.unwrap();
        fixture.insert_example_project().await.unwrap();

        let backends = filesystem_backends_in(&fixture.temp_dir);

        let ingest_service = IngestService::new(
            fixture.db.clone(),
            Arc::new(InMemoryLocalObjectStore::new()),
            backends.clone(),
            Some(LocalBackendName::fs()),
            Arc::new(InMemoryUpstreamCacheClient::new()),
        );
        let gc_service = GcService::new(fixture.db.clone(), backends);

        let state = WriteAppState::new(
            fixture.db.clone(),
            Arc::new(ingest_service),
            Arc::new(gc_service),
            Arc::new(StaticTokenAuthorizer::new(Some(WRITE_TOKEN.to_owned()))),
        );

        WriteTestApp {
            app: write_router(state),
            db: fixture.db,
            _temp_dir: fixture.temp_dir,
        }
    }

    fn json_request(
        method: Method,
        uri: impl AsRef<str>,
        body: serde_json::Value,
    ) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri.as_ref())
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap()
    }

    fn authed_json(method: Method, uri: impl AsRef<str>, body: serde_json::Value) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri.as_ref())
            .header(header::AUTHORIZATION, format!("Bearer {}", WRITE_TOKEN))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap()
    }

    fn authed_empty(method: Method, uri: impl AsRef<str>) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri.as_ref())
            .header(header::AUTHORIZATION, format!("Bearer {}", WRITE_TOKEN))
            .body(Body::empty())
            .unwrap()
    }

    async fn body_bytes(response: axum::response::Response) -> Bytes {
        response.into_body().collect().await.unwrap().to_bytes()
    }

    async fn body_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
        serde_json::from_slice(&body_bytes(response).await).unwrap()
    }

    #[tokio::test]
    async fn write_api_rejects_missing_token() {
        let test_app = build_test_app().await;

        let request = json_request(
            Method::POST,
            "/api/builds",
            json!({
                "project": EXAMPLE_PROJECT_SLUG,
                "ref_name": "main",
                "revision": "deadbeef"
            }),
        );

        let response = test_app.app.clone().oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn begin_build_route_returns_build_id() {
        let test_app = build_test_app().await;

        let request = authed_json(
            Method::POST,
            "/api/builds",
            json!({
                "project": EXAMPLE_PROJECT_SLUG,
                "ref_name": "main",
                "revision": "deadbeef",
            }),
        );

        let response = test_app.app.clone().oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body: BeginBuildResponse = body_json(response).await;
        assert!(!body.build_id.is_empty());
    }

    #[tokio::test]
    async fn create_and_list_project_routes_work() {
        let test_app = build_test_app().await;

        let create_response = test_app
            .app
            .clone()
            .oneshot(authed_json(
                Method::POST,
                "/api/projects",
                json!({
                    "slug": "another_repo",
                    "display_name": "Another Repo",
                    "public": false,
                }),
            ))
            .await
            .unwrap();
        assert_eq!(create_response.status(), StatusCode::NO_CONTENT);

        let list_response = test_app
            .app
            .clone()
            .oneshot(authed_empty(Method::GET, "/api/projects"))
            .await
            .unwrap();
        assert_eq!(list_response.status(), StatusCode::OK);

        let body: Vec<ProjectInfo> = body_json(list_response).await;
        assert!(body.iter().any(|project| project.slug == "another_repo"));
    }

    #[tokio::test]
    async fn create_and_list_pin_routes_work() {
        let test_app = build_test_app().await;

        test_app
            .db
            .upsert_path_info(&hello_path().narinfo())
            .await
            .unwrap();

        let create_response = test_app
            .app
            .clone()
            .oneshot(authed_json(
                Method::POST,
                "/api/pins/release",
                json!({
                    "project": EXAMPLE_PROJECT_SLUG,
                    "store_path": hello_path().store_path(),
                }),
            ))
            .await
            .unwrap();
        assert_eq!(create_response.status(), StatusCode::NO_CONTENT);

        let list_response = test_app
            .app
            .clone()
            .oneshot(authed_empty(
                Method::GET,
                format!("/api/pins?project={EXAMPLE_PROJECT_SLUG}"),
            ))
            .await
            .unwrap();
        assert_eq!(list_response.status(), StatusCode::OK);

        let body: Vec<PinInfo> = body_json(list_response).await;
        assert!(body.iter().any(|pin| pin.name == "release"));

        let delete_response = test_app
            .app
            .clone()
            .oneshot(authed_empty(
                Method::DELETE,
                format!("/api/pins/release?project={EXAMPLE_PROJECT_SLUG}"),
            ))
            .await
            .unwrap();
        assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn gc_route_returns_response() {
        let test_app = build_test_app().await;

        let response = test_app
            .app
            .clone()
            .oneshot(authed_json(
                Method::POST,
                "/api/gc",
                json!({
                    "dry_run": true,
                }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body: RunGcResponse = body_json(response).await;
        assert_eq!(body.deleted_count, 0);
    }
}
