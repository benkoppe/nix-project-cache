pub mod handlers;
pub mod local_objects;
pub mod object_provider;
pub mod resolver;
pub mod router;
pub mod service;
pub mod signing_keys;
pub mod state;
pub mod upstreams;

pub use local_objects::DbBackedObjectStore;
pub use object_provider::{
    CacheObjectProvider, DbBlobCacheObjectProvider, InMemoryCacheObjectProvider,
};
pub use resolver::{DbNarInfoResolver, InMemoryNarInfoResolver, NarInfoResolver};
pub use router::read_router;
pub use service::ReadService;
pub use signing_keys::DbProjectSigningKeys;
pub use state::ReadAppState;
pub use upstreams::{DbUpstreamSelector, StaticUpstreamSelector, UpstreamSelector};

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode, header};
    use bytes::Bytes;
    use cache_test_utils::fixtures::test_signing_key;
    use http_body_util::BodyExt as _;
    use tower::util::ServiceExt as _;

    use cache_core::narinfo::NarInfoRenderer;
    use cache_core::nix::StoreDir;
    use cache_store::blob::BlobMetadata;
    use cache_store::upstream::InMemoryUpstreamCacheClient;
    use cache_test_utils::{
        EXAMPLE_PROJECT_SLUG, SamplePath, example_project, hello_path, sample_upstream,
    };

    use crate::{
        InMemoryCacheObjectProvider, InMemoryNarInfoResolver, ReadAppState, ReadService,
        StaticUpstreamSelector, read_router,
    };

    fn sample_object_provider() -> InMemoryCacheObjectProvider {
        let path = hello_path();

        let mut provider = InMemoryCacheObjectProvider::new();
        provider.insert(
            path.url(),
            BlobMetadata::new("application/octet-stream", Some(9), None, None),
            Bytes::from_static(b"local-nar"),
        );
        provider
    }

    fn sample_state() -> ReadAppState {
        let path = hello_path();
        let hash = path.hash();
        let narinfo = path.narinfo();

        let mut resolver = InMemoryNarInfoResolver::new();
        resolver.insert_aggregate(hash.clone(), narinfo.clone());
        resolver.insert_project(example_project(), hash, narinfo);

        let read_service = ReadService::new(
            Arc::new(resolver),
            Arc::new(sample_object_provider()),
            Arc::new(InMemoryUpstreamCacheClient::new()),
            Arc::new(StaticUpstreamSelector::new()),
            NarInfoRenderer::new(StoreDir::default()),
            Some(test_signing_key()),
            None,
        );

        ReadAppState::new(Arc::new(read_service), 30)
    }

    fn upstream_fallback_state() -> ReadAppState {
        let path = hello_path();
        let hash = path.hash();

        let upstream = sample_upstream("https://cache.nixos.org");
        let mut upstream_client = InMemoryUpstreamCacheClient::new();
        upstream_client.insert_narinfo(
            upstream.id,
            hash.as_str(),
            path.narinfo_text(&["cache.nixos.org-1:upstreamsig"]),
        );
        upstream_client.insert_object(
            upstream.id,
            path.url(),
            BlobMetadata::new("application/octet-stream", Some(12), None, None),
            Bytes::from_static(b"upstream-nar"),
        );

        let mut upstream_selector = StaticUpstreamSelector::new();
        upstream_selector.set_aggregate_upstreams(vec![upstream]);

        let read_service = ReadService::new(
            Arc::new(InMemoryNarInfoResolver::new()),
            Arc::new(InMemoryCacheObjectProvider::new()),
            Arc::new(upstream_client),
            Arc::new(upstream_selector),
            NarInfoRenderer::new(StoreDir::default()),
            Some(test_signing_key()),
            None,
        );

        ReadAppState::new(Arc::new(read_service), 30)
    }

    fn project_cache_info_path() -> String {
        format!("/p/{EXAMPLE_PROJECT_SLUG}/nix-cache-info")
    }

    fn aggregate_narinfo_path(path: SamplePath) -> String {
        format!("/{}.narinfo", path.hash().as_str())
    }

    fn project_narinfo_path(path: SamplePath) -> String {
        format!("/p/{EXAMPLE_PROJECT_SLUG}/{}.narinfo", path.hash().as_str())
    }

    fn object_path(path: SamplePath) -> String {
        format!("/{}", path.url())
    }

    fn invalid_project_narinfo_path(path: SamplePath) -> String {
        format!("/p/INVALID!/{hash}.narinfo", hash = path.hash().as_str())
    }

    fn assert_contains_rendered_narinfo(body: &str, path: SamplePath) {
        for line in path.narinfo_text(&[]).lines() {
            assert!(
                body.contains(line),
                "missing narinfo line {line:?} in body {body:?}"
            );
        }
    }

    async fn request(app: axum::Router, method: Method, uri: &str) -> axum::response::Response {
        app.oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
    }

    async fn get(app: axum::Router, uri: &str) -> axum::response::Response {
        request(app, Method::GET, uri).await
    }

    async fn head(app: axum::Router, uri: &str) -> axum::response::Response {
        request(app, Method::HEAD, uri).await
    }

    async fn body_to_bytes(response: axum::response::Response) -> Bytes {
        response.into_body().collect().await.unwrap().to_bytes()
    }

    async fn body_to_string(response: axum::response::Response) -> String {
        String::from_utf8(body_to_bytes(response).await.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let response = get(read_router(sample_state()), "/health").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_to_string(response).await, "ok\n");
    }

    #[tokio::test]
    async fn aggregate_nix_cache_info_returns_expected_text() {
        let response = get(read_router(sample_state()), "/nix-cache-info").await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap(),
            "text/plain; charset=utf-8"
        );

        let body = body_to_string(response).await;
        assert!(body.contains("StoreDir: /nix/store\n"));
        assert!(body.contains("WantMassQuery: 1\n"));
        assert!(body.contains("Priority: 30\n"));
    }

    #[tokio::test]
    async fn project_nix_cache_info_returns_expected_text() {
        let response = get(read_router(sample_state()), &project_cache_info_path()).await;

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn aggregate_narinfo_route_returns_rendered_signed_narinfo() {
        let path = hello_path();
        let response = get(read_router(sample_state()), &aggregate_narinfo_path(path)).await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap(),
            "text/x-nix-narinfo"
        );

        let body = body_to_string(response).await;
        assert_contains_rendered_narinfo(&body, path);
        assert!(body.contains("\nSig: cache.example.com-1:"));
    }

    #[tokio::test]
    async fn project_narinfo_route_returns_rendered_signed_narinfo() {
        let response = get(
            read_router(sample_state()),
            &project_narinfo_path(hello_path()),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap(),
            "text/x-nix-narinfo"
        );
    }

    #[tokio::test]
    async fn local_nar_route_returns_blob_bytes() {
        let response = get(read_router(sample_state()), &object_path(hello_path())).await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap(),
            "application/octet-stream"
        );
        assert_eq!(
            body_to_bytes(response).await,
            Bytes::from_static(b"local-nar")
        );
    }

    #[tokio::test]
    async fn upstream_narinfo_fallback_returns_rendered_signed_narinfo() {
        let path = hello_path();
        let response = get(
            read_router(upstream_fallback_state()),
            &aggregate_narinfo_path(path),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);

        let body = body_to_string(response).await;
        assert_contains_rendered_narinfo(&body, path);
        assert!(body.contains("Sig: cache.nixos.org-1:upstreamsig\n"));
        assert!(body.contains("Sig: cache.example.com-1:"));
    }

    #[tokio::test]
    async fn upstream_nar_blob_fallback_returns_bytes() {
        let response = get(
            read_router(upstream_fallback_state()),
            &object_path(hello_path()),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            body_to_bytes(response).await,
            Bytes::from_static(b"upstream-nar")
        );
    }

    #[tokio::test]
    async fn missing_narinfo_returns_not_found() {
        let response = get(
            read_router(sample_state()),
            "/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.narinfo",
        )
        .await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn invalid_object_path_returns_not_found() {
        let response = get(read_router(sample_state()), "/foo/bar/baz").await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn invalid_project_slug_returns_not_found() {
        let response = get(
            read_router(sample_state()),
            &invalid_project_narinfo_path(hello_path()),
        )
        .await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn head_narinfo_is_supported() {
        let response = head(
            read_router(sample_state()),
            &aggregate_narinfo_path(hello_path()),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap(),
            "text/x-nix-narinfo"
        );
    }
}
