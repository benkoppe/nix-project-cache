pub mod handlers;
pub mod resolver;
pub mod router;
pub mod state;

pub use resolver::{InMemoryNarInfoResolver, NarInfoResolver};
pub use router::router;
pub use state::AppState;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode, header};
    use http_body_util::BodyExt as _;
    use tower::util::ServiceExt as _;

    use cache_core::narinfo::{NarInfo, NarInfoRenderer};
    use cache_core::nix::{NixHash, StoreDir, StorePathHash};
    use cache_core::project::ProjectSlug;
    use cache_core::signing::{NamedSigningKey, NarInfoSigner};

    use crate::{AppState, InMemoryNarInfoResolver, router};

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

        AppState::new(Arc::new(resolver), renderer, signer, 30)
    }

    async fn body_to_string(response: axum::response::Response) -> String {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
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
