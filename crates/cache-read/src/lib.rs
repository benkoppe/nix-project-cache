pub mod handlers;
pub mod resolver;
pub mod router;
pub mod service;
pub mod state;

pub use resolver::{InMemoryNarInfoResolver, NarInfoResolver};
pub use router::router;
pub use service::ReadService;
pub use state::AppState;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode, header};
    use bytes::Bytes;
    use http_body_util::BodyExt as _;
    use tower::util::ServiceExt as _;
    use uuid::Uuid;

    use cache_core::narinfo::{NarInfo, NarInfoRenderer};
    use cache_core::nix::{NixHash, StoreDir, StorePathHash};
    use cache_core::project::ProjectSlug;
    use cache_core::signing::{NamedSigningKey, NarInfoSigner};

    use cache_store::blob::BlobMetadata;
    use cache_store::local::InMemoryLocalObjectStore;
    use cache_store::upstream::{InMemoryUpstreamCacheClient, UpstreamCache};

    use crate::{AppState, InMemoryNarInfoResolver, ReadService, router};

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

    fn sample_store_path_hash() -> StorePathHash {
        StorePathHash::parse_from_store_path(
            "/nix/store/26xbg1ndr7hbcncrlf9nhx5is2b25d13-hello-2.12.1",
        )
        .unwrap()
    }

    fn sample_local_object_store() -> InMemoryLocalObjectStore {
        let mut store = InMemoryLocalObjectStore::new();
        store.insert(
            "nar/020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz.nar.zst",
            BlobMetadata::new("application/octet-stream", Some(9), None, None),
            Bytes::from_static(b"local-nar"),
        );
        store
    }

    fn sample_state() -> AppState {
        let store_dir = StoreDir::default();
        let renderer = NarInfoRenderer::new(store_dir.clone());
        let signer = NarInfoSigner::new(
            store_dir,
            vec![
                NamedSigningKey::parse(
                    "cache.example.com-1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
                )
                .unwrap(),
            ],
        );

        let mut resolver = InMemoryNarInfoResolver::new();
        let hash = sample_store_path_hash();
        let narinfo = sample_narinfo();

        resolver.insert_aggregate(hash.clone(), narinfo.clone());
        resolver.insert_project(ProjectSlug::parse("example_repo").unwrap(), hash, narinfo);

        let read_service = ReadService::new(
            Arc::new(resolver),
            Arc::new(sample_local_object_store()),
            Arc::new(InMemoryUpstreamCacheClient::new()),
            Vec::new(),
            renderer,
            signer,
        );

        AppState::new(Arc::new(read_service), 30)
    }

    fn upstream_fallback_state() -> AppState {
        let store_dir = StoreDir::default();
        let renderer = NarInfoRenderer::new(store_dir.clone());
        let signer = NarInfoSigner::new(
            store_dir,
            vec![
                NamedSigningKey::parse(
                    "cache.example.com-1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
                )
                .unwrap(),
            ],
        );

        let resolver = InMemoryNarInfoResolver::new();
        let mut upstream_client = InMemoryUpstreamCacheClient::new();
        let upstream = UpstreamCache::new(
            Uuid::now_v7(),
            "cache.nixos.org",
            "https://cache.nixos.org",
            10,
        );

        upstream_client.insert_narinfo(
            upstream.id,
            "26xbg1ndr7hbcncrlf9nhx5is2b25d13",
            "\
StorePath: /nix/store/26xbg1ndr7hbcncrlf9nhx5is2b25d13-hello-2.12.1
URL: nar/020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz.nar.zst
Compression: zstd
NarHash: sha256:020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz
NarSize: 226560
References: 
Sig: cache.nixos.org-1:upstreamsig
",
        );

        upstream_client.insert_object(
            upstream.id,
            "nar/020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz.nar.zst",
            BlobMetadata::new("application/octet-stream", Some(12), None, None),
            Bytes::from_static(b"upstream-nar"),
        );

        let read_service = ReadService::new(
            Arc::new(resolver),
            Arc::new(InMemoryLocalObjectStore::new()),
            Arc::new(upstream_client),
            vec![upstream],
            renderer,
            signer,
        );

        AppState::new(Arc::new(read_service), 30)
    }

    async fn body_to_bytes(response: axum::response::Response) -> Bytes {
        response.into_body().collect().await.unwrap().to_bytes()
    }

    async fn body_to_string(response: axum::response::Response) -> String {
        String::from_utf8(body_to_bytes(response).await.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let app = router(sample_state());

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_to_string(response).await, "ok\n");
    }

    #[tokio::test]
    async fn aggregate_nix_cache_info_returns_expected_text() {
        let app = router(sample_state());

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/nix-cache-info")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

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
        let app = router(sample_state());

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/p/example_repo/nix-cache-info")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn aggregate_narinfo_route_returns_rendered_signed_narinfo() {
        let app = router(sample_state());

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/26xbg1ndr7hbcncrlf9nhx5is2b25d13.narinfo")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

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
        assert!(
            body.contains("StorePath: /nix/store/26xbg1ndr7hbcncrlf9nhx5is2b25d13-hello-2.12.1\n")
        );
        assert!(
            body.contains(
                "URL: nar/020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz.nar.zst\n"
            )
        );
        assert!(
            body.contains("NarHash: sha256:020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz\n")
        );
        assert!(body.contains("\nSig: cache.example.com-1:"));
    }

    #[tokio::test]
    async fn project_narinfo_route_returns_rendered_signed_narinfo() {
        let app = router(sample_state());

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/p/example_repo/26xbg1ndr7hbcncrlf9nhx5is2b25d13.narinfo")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

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
        let app = router(sample_state());

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/nar/020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz.nar.zst")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

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
        let app = router(upstream_fallback_state());

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/26xbg1ndr7hbcncrlf9nhx5is2b25d13.narinfo")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = body_to_string(response).await;
        assert!(
            body.contains("StorePath: /nix/store/26xbg1ndr7hbcncrlf9nhx5is2b25d13-hello-2.12.1\n")
        );
        assert!(body.contains("Sig: cache.nixos.org-1:upstreamsig\n"));
        assert!(body.contains("Sig: cache.example.com-1:"));
    }

    #[tokio::test]
    async fn upstream_nar_blob_fallback_returns_bytes() {
        let app = router(upstream_fallback_state());

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/nar/020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz.nar.zst")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            body_to_bytes(response).await,
            Bytes::from_static(b"upstream-nar")
        );
    }

    #[tokio::test]
    async fn missing_narinfo_returns_not_found() {
        let app = router(sample_state());

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.narinfo")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn invalid_object_path_returns_not_found() {
        let app = router(sample_state());

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/foo/bar/baz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn invalid_project_slug_returns_not_found() {
        let app = router(sample_state());

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/p/INVALID!/26xbg1ndr7hbcncrlf9nhx5is2b25d13.narinfo")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn head_narinfo_is_supported() {
        let app = router(sample_state());

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::HEAD)
                    .uri("/26xbg1ndr7hbcncrlf9nhx5is2b25d13.narinfo")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

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
