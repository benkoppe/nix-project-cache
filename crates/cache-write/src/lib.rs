pub mod authz;
pub mod handlers;
pub mod router;
pub mod state;

pub use authz::{AuthorizationService, AuthorizationServiceError, AuthorizedPrincipal};
pub use router::write_router;
pub use state::WriteAppState;

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode, header};
    use bytes::Bytes;
    use http_body_util::BodyExt as _;
    use serde_json::json;
    use tempfile::TempDir;
    use tower::util::ServiceExt as _;

    use cache_api::{
        BeginBuildResponse, CreateAccessTokenRequest, CreateAccessTokenResponse,
        DeleteProjectOidcIdentityRequest, PinInfo, ProjectInfo, ProjectOidcIdentityInfo,
        RunGcResponse, UpsertProjectOidcIdentityRequest,
    };
    use cache_auth::{
        AuthError, AuthenticatedIdentity, Authorizer, OidcAuthorizer, OidcConfig,
        OidcProviderConfig, StaticOidcHttpClient, StaticTokenAuthorizer,
    };
    use cache_core::nix::StoreDir;
    use cache_core::storage::LocalBackendName;
    use cache_db::SqliteDatabase;
    use cache_ingest::{GcService, IngestService};
    use cache_store::local::InMemoryLocalObjectStore;
    use cache_store::upstream::InMemoryUpstreamCacheClient;
    use cache_test_utils::{
        EXAMPLE_PROJECT_SLUG, TestDatabase, TestOidcIssuer, example_project,
        filesystem_backends_in, hello_path,
    };

    use super::*;

    const WRITE_TOKEN: &str = "secret-token";
    const TEST_OIDC_ISSUER: &str = "https://token.actions.githubusercontent.com";
    const TEST_OIDC_AUDIENCE: &str = "https://cache.example.com";
    const TEST_OIDC_KID: &str = "cache-write-test-key";

    struct FixedAuthorizer {
        result: Result<AuthenticatedIdentity, AuthError>,
    }

    #[async_trait]
    impl Authorizer for FixedAuthorizer {
        async fn authorize_bearer(
            &self,
            _bearer_token: Option<&str>,
        ) -> Result<AuthenticatedIdentity, AuthError> {
            self.result.clone()
        }
    }

    struct WriteTestApp {
        app: axum::Router,
        db: SqliteDatabase,
        _temp_dir: TempDir,
    }

    async fn build_test_app() -> WriteTestApp {
        build_test_app_with_authorizer(Arc::new(StaticTokenAuthorizer::new(Some(
            WRITE_TOKEN.to_owned(),
        ))))
        .await
    }

    async fn build_test_app_with_authorizer(authorizer: Arc<dyn Authorizer>) -> WriteTestApp {
        let fixture = TestDatabase::new().await.unwrap();
        fixture.insert_example_project().await.unwrap();

        let backends = filesystem_backends_in(&fixture.temp_dir);

        let ingest_service = IngestService::new(
            fixture.db.clone(),
            StoreDir::default(),
            Arc::new(InMemoryLocalObjectStore::new()),
            backends.clone(),
            Some(LocalBackendName::fs()),
            Arc::new(InMemoryUpstreamCacheClient::new()),
        );
        let gc_service = GcService::new(fixture.db.clone(), backends);

        let authorization_service = AuthorizationService::new(fixture.db.clone(), authorizer);

        let state = WriteAppState::new(
            fixture.db.clone(),
            Arc::new(ingest_service),
            Arc::new(gc_service),
            Arc::new(authorization_service),
        );

        WriteTestApp {
            app: write_router(state),
            db: fixture.db,
            _temp_dir: fixture.temp_dir,
        }
    }

    fn oidc_identity(ref_name: &str) -> AuthenticatedIdentity {
        AuthenticatedIdentity::oidc(
            "github",
            "repo:owner/repo:ref",
            "https://token.actions.githubusercontent.com",
            Some("owner/repo".to_owned()),
            Some(ref_name.to_owned()),
            serde_json::Map::new(),
        )
    }

    async fn insert_example_oidc_binding(test_app: &WriteTestApp, ref_patterns: &[&str]) {
        let patterns = ref_patterns
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        test_app
            .db
            .replace_project_oidc_identity(&example_project(), "github", "owner/repo", &patterns)
            .await
            .unwrap();
    }

    async fn create_example_access_token(test_app: &WriteTestApp, ref_patterns: &[&str]) -> String {
        let patterns = ref_patterns
            .iter()
            .map(|pattern| pattern.to_string())
            .collect::<Vec<_>>();

        test_app
            .db
            .create_access_token("ci", &example_project(), &patterns, None)
            .await
            .unwrap()
            .token
    }

    fn build_configured_claim_oidc_authorizer() -> (OidcAuthorizer, TestOidcIssuer) {
        let issuer =
            TestOidcIssuer::new(TEST_OIDC_ISSUER, TEST_OIDC_AUDIENCE, TEST_OIDC_KID).unwrap();

        let mut http = StaticOidcHttpClient::new();
        http.insert(
            issuer.discovery_url(),
            issuer.discovery_document().to_string(),
        );
        http.insert(issuer.jwks_url(), issuer.jwks_document().to_string());

        let authorizer = OidcAuthorizer::new(
            OidcConfig {
                providers: BTreeMap::from([(
                    "github".to_owned(),
                    OidcProviderConfig {
                        issuer: TEST_OIDC_ISSUER.to_owned(),
                        audience: TEST_OIDC_AUDIENCE.to_owned(),
                        repository_claim: Some("project_path".to_owned()),
                        ref_claim: Some("git_ref".to_owned()),
                        bound_claims: BTreeMap::from([(
                            "project_path".to_owned(),
                            vec!["owner/repo".to_owned()],
                        )]),
                        bound_subject: vec!["repo:owner/repo:*".to_owned()],
                    },
                )]),
                allow_insecure: false,
            },
            Arc::new(http),
        );

        (authorizer, issuer)
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
        bearer_json(WRITE_TOKEN, method, uri, body)
    }

    fn bearer_json(
        token: &str,
        method: Method,
        uri: impl AsRef<str>,
        body: serde_json::Value,
    ) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri.as_ref())
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
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

    #[tokio::test]
    async fn admin_can_manage_project_oidc_identities() {
        let test_app = build_test_app().await;

        let create_response = test_app
            .app
            .clone()
            .oneshot(authed_json(
                Method::POST,
                format!("/api/projects/{EXAMPLE_PROJECT_SLUG}/oidc-identities"),
                json!(UpsertProjectOidcIdentityRequest {
                    provider: "github".to_owned(),
                    repository: "owner/repo".to_owned(),
                    ref_patterns: vec!["refs/heads/main".to_owned(), "refs/tags/*".to_owned()],
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
                format!("/api/projects/{EXAMPLE_PROJECT_SLUG}/oidc-identities"),
            ))
            .await
            .unwrap();
        assert_eq!(list_response.status(), StatusCode::OK);

        let body: Vec<ProjectOidcIdentityInfo> = body_json(list_response).await;
        assert_eq!(body.len(), 1);
        assert_eq!(body[0].provider, "github");
        assert_eq!(body[0].repository, "owner/repo");

        let delete_response = test_app
            .app
            .clone()
            .oneshot(authed_json(
                Method::DELETE,
                format!("/api/projects/{EXAMPLE_PROJECT_SLUG}/oidc-identities"),
                json!(DeleteProjectOidcIdentityRequest {
                    provider: "github".to_owned(),
                    repository: "owner/repo".to_owned(),
                }),
            ))
            .await
            .unwrap();
        assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn project_identity_can_begin_build_for_matching_project_and_ref() {
        let test_app = build_test_app_with_authorizer(Arc::new(FixedAuthorizer {
            result: Ok(oidc_identity("refs/heads/main")),
        }))
        .await;
        insert_example_oidc_binding(&test_app, &["refs/heads/main"]).await;

        let response = test_app
            .app
            .clone()
            .oneshot(authed_json(
                Method::POST,
                "/api/builds",
                json!({
                    "project": EXAMPLE_PROJECT_SLUG,
                    "ref_name": "refs/heads/main",
                    "revision": "deadbeef",
                }),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn project_identity_cannot_begin_build_for_wrong_project() {
        let test_app = build_test_app_with_authorizer(Arc::new(FixedAuthorizer {
            result: Ok(oidc_identity("refs/heads/main")),
        }))
        .await;
        insert_example_oidc_binding(&test_app, &["refs/heads/main"]).await;

        let response = test_app
            .app
            .clone()
            .oneshot(authed_json(
                Method::POST,
                "/api/builds",
                json!({
                    "project": "other_repo",
                    "ref_name": "refs/heads/main",
                    "revision": "deadbeef",
                }),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn project_identity_cannot_begin_build_for_wrong_ref() {
        let test_app = build_test_app_with_authorizer(Arc::new(FixedAuthorizer {
            result: Ok(oidc_identity("refs/heads/main")),
        }))
        .await;
        insert_example_oidc_binding(&test_app, &["refs/heads/main"]).await;

        let response = test_app
            .app
            .clone()
            .oneshot(authed_json(
                Method::POST,
                "/api/builds",
                json!({
                    "project": EXAMPLE_PROJECT_SLUG,
                    "ref_name": "refs/heads/feature",
                    "revision": "deadbeef",
                }),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn project_identity_cannot_manage_projects() {
        let test_app = build_test_app_with_authorizer(Arc::new(FixedAuthorizer {
            result: Ok(oidc_identity("refs/heads/main")),
        }))
        .await;
        insert_example_oidc_binding(&test_app, &["refs/heads/main"]).await;

        let response = test_app
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

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn project_identity_can_manage_project_scoped_pins_for_same_project() {
        let test_app = build_test_app_with_authorizer(Arc::new(FixedAuthorizer {
            result: Ok(oidc_identity("refs/heads/main")),
        }))
        .await;
        insert_example_oidc_binding(&test_app, &["refs/heads/main"]).await;

        test_app
            .db
            .upsert_path_info(&hello_path().narinfo())
            .await
            .unwrap();

        let response = test_app
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

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn project_identity_cannot_create_global_pin() {
        let test_app = build_test_app_with_authorizer(Arc::new(FixedAuthorizer {
            result: Ok(oidc_identity("refs/heads/main")),
        }))
        .await;
        insert_example_oidc_binding(&test_app, &["refs/heads/main"]).await;

        test_app
            .db
            .upsert_path_info(&hello_path().narinfo())
            .await
            .unwrap();

        let response = test_app
            .app
            .clone()
            .oneshot(authed_json(
                Method::POST,
                "/api/pins/release",
                json!({
                    "store_path": hello_path().store_path(),
                }),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn project_identity_cannot_manage_project_oidc_identities() {
        let test_app = build_test_app_with_authorizer(Arc::new(FixedAuthorizer {
            result: Ok(oidc_identity("refs/heads/main")),
        }))
        .await;
        insert_example_oidc_binding(&test_app, &["refs/heads/main"]).await;

        let response = test_app
            .app
            .clone()
            .oneshot(authed_json(
                Method::POST,
                format!("/api/projects/{EXAMPLE_PROJECT_SLUG}/oidc-identities"),
                json!(UpsertProjectOidcIdentityRequest {
                    provider: "github".to_owned(),
                    repository: "owner/other".to_owned(),
                    ref_patterns: vec!["refs/heads/main".to_owned()],
                }),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn project_identity_cannot_run_gc() {
        let test_app = build_test_app_with_authorizer(Arc::new(FixedAuthorizer {
            result: Ok(oidc_identity("refs/heads/main")),
        }))
        .await;
        insert_example_oidc_binding(&test_app, &["refs/heads/main"]).await;

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

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn project_access_token_can_begin_build_for_matching_project_and_ref() {
        let test_app = build_test_app().await;
        let token = create_example_access_token(&test_app, &["refs/heads/main"]).await;

        let response = test_app
            .app
            .clone()
            .oneshot(bearer_json(
                &token,
                Method::POST,
                "/api/builds",
                json!({
                    "project": EXAMPLE_PROJECT_SLUG,
                    "ref_name": "refs/heads/main",
                    "revision": "deadbeef",
                }),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn project_access_token_can_begin_build_for_any_ref_when_unrestricted() {
        let test_app = build_test_app().await;
        let token = create_example_access_token(&test_app, &[]).await;

        let response = test_app
            .app
            .clone()
            .oneshot(bearer_json(
                &token,
                Method::POST,
                "/api/builds",
                json!({
                    "project": EXAMPLE_PROJECT_SLUG,
                    "ref_name": "refs/heads/feature",
                    "revision": "deadbeef",
                }),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn project_access_token_cannot_begin_build_for_wrong_project() {
        let test_app = build_test_app().await;
        let token = create_example_access_token(&test_app, &["refs/heads/main"]).await;

        let response = test_app
            .app
            .clone()
            .oneshot(bearer_json(
                &token,
                Method::POST,
                "/api/builds",
                json!({
                    "project": "other_repo",
                    "ref_name": "refs/heads/main",
                    "revision": "deadbeef",
                }),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn project_access_token_cannot_begin_build_for_wrong_ref() {
        let test_app = build_test_app().await;
        let token = create_example_access_token(&test_app, &["refs/heads/main"]).await;

        let response = test_app
            .app
            .clone()
            .oneshot(bearer_json(
                &token,
                Method::POST,
                "/api/builds",
                json!({
                    "project": EXAMPLE_PROJECT_SLUG,
                    "ref_name": "refs/heads/feature",
                    "revision": "deadbeef",
                }),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn revoked_project_access_token_is_rejected() {
        let test_app = build_test_app().await;
        let created = test_app
            .db
            .create_access_token(
                "ci",
                &example_project(),
                &["refs/heads/main".to_owned()],
                None,
            )
            .await
            .unwrap();
        let token_id = created.records[0].id.clone();

        assert!(test_app.db.revoke_access_token(&token_id).await.unwrap());

        let response = test_app
            .app
            .clone()
            .oneshot(bearer_json(
                &created.token,
                Method::POST,
                "/api/builds",
                json!({
                    "project": EXAMPLE_PROJECT_SLUG,
                    "ref_name": "refs/heads/main",
                    "revision": "deadbeef",
                }),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn expired_project_access_token_is_rejected() {
        let test_app = build_test_app().await;

        let create_response = test_app
            .app
            .clone()
            .oneshot(authed_json(
                Method::POST,
                "/api/access-tokens",
                json!(CreateAccessTokenRequest {
                    name: "ci".to_owned(),
                    project: EXAMPLE_PROJECT_SLUG.to_owned(),
                    ref_patterns: vec!["refs/heads/main".to_owned()],
                    expires_at: Some("2000-01-01T00:00:00Z".to_owned()),
                }),
            ))
            .await
            .unwrap();
        assert_eq!(create_response.status(), StatusCode::OK);

        let created: CreateAccessTokenResponse = body_json(create_response).await;

        let response = test_app
            .app
            .clone()
            .oneshot(bearer_json(
                &created.token,
                Method::POST,
                "/api/builds",
                json!({
                    "project": EXAMPLE_PROJECT_SLUG,
                    "ref_name": "refs/heads/main",
                    "revision": "deadbeef",
                }),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn project_access_token_cannot_manage_admin_only_access_tokens() {
        let test_app = build_test_app().await;
        let token = create_example_access_token(&test_app, &["refs/heads/main"]).await;

        let response = test_app
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/access-tokens")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn oidc_identity_with_configured_claim_names_can_begin_build_for_matching_project_and_ref()
     {
        let (authorizer, issuer) = build_configured_claim_oidc_authorizer();
        let test_app = build_test_app_with_authorizer(Arc::new(authorizer)).await;

        test_app
            .db
            .replace_project_oidc_identity(
                &example_project(),
                "github",
                "owner/repo",
                &["refs/heads/main".to_owned()],
            )
            .await
            .unwrap();

        let mut claims = issuer.github_actions_claims("owner/repo", "refs/heads/main");
        claims.set_string_claim("project_path", "owner/repo");
        claims.set_string_claim("git_ref", "refs/heads/main");

        let token = issuer.issue_token(&claims).unwrap();

        let response = test_app
            .app
            .clone()
            .oneshot(bearer_json(
                &token,
                Method::POST,
                "/api/builds",
                json!({
                    "project": EXAMPLE_PROJECT_SLUG,
                    "ref_name": "refs/heads/main",
                    "revision": "deadbeef",
                }),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn oidc_identity_with_configured_claim_names_rejects_wrong_ref() {
        let (authorizer, issuer) = build_configured_claim_oidc_authorizer();
        let test_app = build_test_app_with_authorizer(Arc::new(authorizer)).await;

        test_app
            .db
            .replace_project_oidc_identity(
                &example_project(),
                "github",
                "owner/repo",
                &["refs/heads/main".to_owned()],
            )
            .await
            .unwrap();

        let mut claims = issuer.github_actions_claims("owner/repo", "refs/heads/main");
        claims.set_string_claim("project_path", "owner/repo");
        claims.set_string_claim("git_ref", "refs/heads/main");

        let token = issuer.issue_token(&claims).unwrap();

        let response = test_app
            .app
            .clone()
            .oneshot(bearer_json(
                &token,
                Method::POST,
                "/api/builds",
                json!({
                    "project": EXAMPLE_PROJECT_SLUG,
                    "ref_name": "refs/heads/feature",
                    "revision": "deadbeef",
                }),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
