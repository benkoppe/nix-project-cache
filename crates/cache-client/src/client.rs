use std::fmt::Write as _;

use reqwest::{Method, StatusCode, Url};
use serde::de::DeserializeOwned;
use tokio::io::AsyncRead;
use tokio_util::io::ReaderStream;

use cache_api::{
    AccessTokenInfo, BeginBuildRequest, BeginBuildResponse, CreateAccessTokenRequest,
    CreateAccessTokenResponse, CreatePinRequest, DeleteProjectOidcIdentityRequest,
    FinalizeBuildResponse, GenerateProjectSigningKeyRequest, ImportProjectSigningKeyRequest,
    PinInfo, ProjectInfo, ProjectOidcIdentityInfo, ProjectRetentionPolicyInfo,
    ProjectSigningKeyInfo, ProjectSigningKeyResponse, RegisterPathsResponse, RunGcRequest,
    RunGcResponse, UpsertProjectOidcIdentityRequest, UpsertProjectRequest,
    UpsertProjectRetentionPolicyRequest, UpsertUpstreamRequest, UpstreamInfo,
};
use cache_core::narinfo::NarInfo;
use cache_core::nix::StorePathHash;
use cache_core::project::ProjectSlug;
use cache_core::storage::StorageId;

use crate::error::CacheClientError;
use crate::routes;

#[derive(Clone)]
pub struct CacheClient {
    base_url: Url,
    auth_token: String,
    http_client: reqwest::Client,
}

impl CacheClient {
    pub fn new(server_url: &str, auth_token: impl Into<String>) -> Result<Self, CacheClientError> {
        let mut base_url =
            Url::parse(server_url).map_err(|error| CacheClientError::InvalidServerUrl {
                url: server_url.to_owned(),
                message: error.to_string(),
            })?;

        if !base_url.path().ends_with('/') {
            let trimmed = base_url.path().trim_end_matches('/');
            let normalized = if trimmed.is_empty() {
                "/".to_owned()
            } else {
                format!("{trimmed}/")
            };
            base_url.set_path(&normalized);
        }

        Ok(Self {
            base_url,
            auth_token: auth_token.into(),
            http_client: reqwest::Client::new(),
        })
    }

    pub fn with_http_client(mut self, http_client: reqwest::Client) -> Self {
        self.http_client = http_client;
        self
    }

    pub async fn begin_build(
        &self,
        request: BeginBuildRequest,
    ) -> Result<BeginBuildResponse, CacheClientError> {
        let url = routes::begin_build(&self.base_url)?;
        let response = self
            .request(Method::POST, url)
            .json(&request)
            .send()
            .await?;

        self.expect_json(response, &[StatusCode::OK]).await
    }

    pub async fn register_paths(
        &self,
        build_id: &str,
        paths: Vec<NarInfo>,
    ) -> Result<RegisterPathsResponse, CacheClientError> {
        let url = routes::register_paths(&self.base_url, build_id)?;
        let payloads = paths.iter().map(cache_api::NarInfoPayload::from).collect();

        let response = self
            .request(Method::POST, url)
            .json(&cache_api::RegisterPathsRequest {
                build_id: build_id.to_owned(),
                paths: payloads,
            })
            .send()
            .await?;

        self.expect_json(response, &[StatusCode::OK]).await
    }

    pub async fn upload_object_reader<R>(
        &self,
        build_id: &str,
        store_path_hash: &StorePathHash,
        object_path: &str,
        reader: R,
    ) -> Result<(), CacheClientError>
    where
        R: AsyncRead + Send + Unpin + 'static,
    {
        let url = routes::upload_object(&self.base_url, build_id, store_path_hash, object_path)?;
        let body = reqwest::Body::wrap_stream(ReaderStream::new(reader));

        let response = self
            .request(Method::PUT, url)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(body)
            .send()
            .await?;

        self.expect_empty(response, &[StatusCode::NO_CONTENT]).await
    }

    pub async fn upload_object_bytes(
        &self,
        build_id: &str,
        store_path_hash: &StorePathHash,
        object_path: &str,
        bytes: bytes::Bytes,
    ) -> Result<(), CacheClientError> {
        let url = routes::upload_object(&self.base_url, build_id, store_path_hash, object_path)?;

        let response = self
            .request(Method::PUT, url)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(bytes)
            .send()
            .await?;

        self.expect_empty(response, &[StatusCode::NO_CONTENT]).await
    }

    pub async fn finalize_build(
        &self,
        build_id: &str,
    ) -> Result<FinalizeBuildResponse, CacheClientError> {
        let url = routes::finalize_build(&self.base_url, build_id)?;
        let response = self.request(Method::POST, url).send().await?;

        self.expect_json(response, &[StatusCode::OK]).await
    }

    pub async fn list_pins(
        &self,
        project: Option<&ProjectSlug>,
    ) -> Result<Vec<PinInfo>, CacheClientError> {
        let url = routes::list_pins(&self.base_url, project)?;
        let response = self.request(Method::GET, url).send().await?;

        self.expect_json(response, &[StatusCode::OK]).await
    }

    pub async fn create_pin(
        &self,
        name: &str,
        project: Option<&ProjectSlug>,
        store_path: &str,
    ) -> Result<(), CacheClientError> {
        let url = routes::create_pin(&self.base_url, name)?;
        let response = self
            .request(Method::POST, url)
            .json(&CreatePinRequest {
                project: project.map(|slug| slug.as_str().to_owned()),
                store_path: store_path.to_owned(),
            })
            .send()
            .await?;

        self.expect_empty(response, &[StatusCode::NO_CONTENT]).await
    }

    pub async fn delete_pin(
        &self,
        name: &str,
        project: Option<&ProjectSlug>,
    ) -> Result<bool, CacheClientError> {
        let url = routes::delete_pin(&self.base_url, name, project)?;
        let response = self.request(Method::DELETE, url).send().await?;

        match response.status() {
            StatusCode::NO_CONTENT => Ok(true),
            StatusCode::NOT_FOUND => Ok(false),
            status => Err(self.unexpected_status(response, status).await),
        }
    }

    pub async fn run_gc(&self, request: RunGcRequest) -> Result<RunGcResponse, CacheClientError> {
        let url = routes::run_gc(&self.base_url)?;
        let response = self
            .request(Method::POST, url)
            .json(&request)
            .send()
            .await?;

        self.expect_json(response, &[StatusCode::OK]).await
    }

    pub async fn list_projects(&self) -> Result<Vec<ProjectInfo>, CacheClientError> {
        let url = routes::list_projects(&self.base_url)?;
        let response = self.request(Method::GET, url).send().await?;

        self.expect_json(response, &[StatusCode::OK]).await
    }

    pub async fn upsert_project(
        &self,
        project: &ProjectSlug,
        display_name: &str,
        public: bool,
    ) -> Result<(), CacheClientError> {
        self.upsert_project_with_storage(project, display_name, public, None)
            .await
    }

    pub async fn upsert_project_with_storage(
        &self,
        project: &ProjectSlug,
        display_name: &str,
        public: bool,
        storage_id: Option<&StorageId>,
    ) -> Result<(), CacheClientError> {
        let url = routes::upsert_project(&self.base_url)?;
        let response = self
            .request(Method::POST, url)
            .json(&UpsertProjectRequest {
                slug: project.as_str().to_owned(),
                display_name: display_name.to_owned(),
                public,
                storage_id: storage_id.map(|id| id.as_str().to_owned()),
            })
            .send()
            .await?;

        self.expect_empty(response, &[StatusCode::NO_CONTENT]).await
    }

    pub async fn list_project_oidc_identities(
        &self,
        project: &ProjectSlug,
    ) -> Result<Vec<ProjectOidcIdentityInfo>, CacheClientError> {
        let url = routes::project_oidc_identities(&self.base_url, project)?;
        let response = self.request(Method::GET, url).send().await?;

        self.expect_json(response, &[StatusCode::OK]).await
    }

    pub async fn upsert_project_oidc_identity(
        &self,
        project: &ProjectSlug,
        request: UpsertProjectOidcIdentityRequest,
    ) -> Result<(), CacheClientError> {
        let url = routes::project_oidc_identities(&self.base_url, project)?;
        let response = self
            .request(Method::POST, url)
            .json(&request)
            .send()
            .await?;

        self.expect_empty(response, &[StatusCode::NO_CONTENT]).await
    }

    pub async fn delete_project_oidc_identity(
        &self,
        project: &ProjectSlug,
        request: DeleteProjectOidcIdentityRequest,
    ) -> Result<bool, CacheClientError> {
        let url = routes::project_oidc_identities(&self.base_url, project)?;
        let response = self
            .request(Method::DELETE, url)
            .json(&request)
            .send()
            .await?;

        match response.status() {
            StatusCode::NO_CONTENT => Ok(true),
            StatusCode::NOT_FOUND => Ok(false),
            status => Err(self.unexpected_status(response, status).await),
        }
    }

    pub async fn get_project_retention_policy(
        &self,
        project: &ProjectSlug,
    ) -> Result<ProjectRetentionPolicyInfo, CacheClientError> {
        let url = routes::project_retention(&self.base_url, project)?;
        let response = self.request(Method::GET, url).send().await?;

        self.expect_json(response, &[StatusCode::OK]).await
    }

    pub async fn upsert_project_retention_policy(
        &self,
        project: &ProjectSlug,
        request: UpsertProjectRetentionPolicyRequest,
    ) -> Result<(), CacheClientError> {
        let url = routes::project_retention(&self.base_url, project)?;
        let response = self.request(Method::PUT, url).json(&request).send().await?;

        self.expect_empty(response, &[StatusCode::NO_CONTENT]).await
    }

    pub async fn delete_project_retention_policy(
        &self,
        project: &ProjectSlug,
    ) -> Result<bool, CacheClientError> {
        let url = routes::project_retention(&self.base_url, project)?;
        let response = self.request(Method::DELETE, url).send().await?;

        match response.status() {
            StatusCode::NO_CONTENT => Ok(true),
            StatusCode::NOT_FOUND => Ok(false),
            status => Err(self.unexpected_status(response, status).await),
        }
    }

    pub async fn get_project_signing_key(
        &self,
        project: &ProjectSlug,
    ) -> Result<ProjectSigningKeyInfo, CacheClientError> {
        let url = routes::project_signing_key(&self.base_url, project)?;
        let response = self.request(Method::GET, url).send().await?;

        self.expect_json(response, &[StatusCode::OK]).await
    }

    pub async fn generate_project_signing_key(
        &self,
        project: &ProjectSlug,
        name: Option<String>,
    ) -> Result<ProjectSigningKeyResponse, CacheClientError> {
        let url = routes::generate_project_signing_key(&self.base_url, project)?;
        let response = self
            .request(Method::POST, url)
            .json(&GenerateProjectSigningKeyRequest { name })
            .send()
            .await?;

        self.expect_json(response, &[StatusCode::OK]).await
    }

    pub async fn import_project_signing_key(
        &self,
        project: &ProjectSlug,
        name: Option<String>,
        signing_key: String,
    ) -> Result<ProjectSigningKeyResponse, CacheClientError> {
        let url = routes::import_project_signing_key(&self.base_url, project)?;
        let response = self
            .request(Method::POST, url)
            .json(&ImportProjectSigningKeyRequest { name, signing_key })
            .send()
            .await?;

        self.expect_json(response, &[StatusCode::OK]).await
    }

    pub async fn list_upstreams(&self) -> Result<Vec<UpstreamInfo>, CacheClientError> {
        let url = routes::upstreams(&self.base_url)?;
        let response = self.request(Method::GET, url).send().await?;

        self.expect_json(response, &[StatusCode::OK]).await
    }

    pub async fn upsert_upstream(
        &self,
        request: UpsertUpstreamRequest,
    ) -> Result<(), CacheClientError> {
        let url = routes::upstreams(&self.base_url)?;
        let response = self
            .request(Method::POST, url)
            .json(&request)
            .send()
            .await?;

        self.expect_empty(response, &[StatusCode::NO_CONTENT]).await
    }

    pub async fn set_upstream_enabled(
        &self,
        upstream: &str,
        enabled: bool,
    ) -> Result<bool, CacheClientError> {
        let url = routes::upstream_enabled(&self.base_url, upstream, enabled)?;
        let response = self.request(Method::POST, url).send().await?;

        match response.status() {
            StatusCode::NO_CONTENT => Ok(true),
            StatusCode::NOT_FOUND => Ok(false),
            status => Err(self.unexpected_status(response, status).await),
        }
    }

    pub async fn list_project_upstreams(
        &self,
        project: &ProjectSlug,
    ) -> Result<Vec<UpstreamInfo>, CacheClientError> {
        let url = routes::project_upstreams(&self.base_url, project)?;
        let response = self.request(Method::GET, url).send().await?;

        self.expect_json(response, &[StatusCode::OK]).await
    }

    pub async fn link_project_upstream(
        &self,
        project: &ProjectSlug,
        upstream: &str,
    ) -> Result<bool, CacheClientError> {
        let url = routes::project_upstream(&self.base_url, project, upstream)?;
        let response = self.request(Method::POST, url).send().await?;

        match response.status() {
            StatusCode::NO_CONTENT => Ok(true),
            StatusCode::NOT_FOUND => Ok(false),
            status => Err(self.unexpected_status(response, status).await),
        }
    }

    pub async fn unlink_project_upstream(
        &self,
        project: &ProjectSlug,
        upstream: &str,
    ) -> Result<bool, CacheClientError> {
        let url = routes::project_upstream(&self.base_url, project, upstream)?;
        let response = self.request(Method::DELETE, url).send().await?;

        match response.status() {
            StatusCode::NO_CONTENT => Ok(true),
            StatusCode::NOT_FOUND => Ok(false),
            status => Err(self.unexpected_status(response, status).await),
        }
    }

    pub async fn create_access_token(
        &self,
        request: CreateAccessTokenRequest,
    ) -> Result<CreateAccessTokenResponse, CacheClientError> {
        let url = routes::access_tokens(&self.base_url, None)?;
        let response = self
            .request(Method::POST, url)
            .json(&request)
            .send()
            .await?;

        self.expect_json(response, &[StatusCode::OK]).await
    }

    pub async fn list_access_tokens(
        &self,
        project: Option<&ProjectSlug>,
    ) -> Result<Vec<AccessTokenInfo>, CacheClientError> {
        let url = routes::access_tokens(&self.base_url, project)?;
        let response = self.request(Method::GET, url).send().await?;

        self.expect_json(response, &[StatusCode::OK]).await
    }

    pub async fn revoke_access_token(&self, token_id: &str) -> Result<bool, CacheClientError> {
        let url = routes::revoke_access_token(&self.base_url, token_id)?;
        let response = self.request(Method::DELETE, url).send().await?;

        match response.status() {
            StatusCode::NO_CONTENT => Ok(true),
            StatusCode::NOT_FOUND => Ok(false),
            status => Err(self.unexpected_status(response, status).await),
        }
    }

    fn request(&self, method: Method, url: Url) -> reqwest::RequestBuilder {
        self.http_client
            .request(method, url)
            .bearer_auth(&self.auth_token)
    }

    async fn expect_empty(
        &self,
        response: reqwest::Response,
        ok_statuses: &[StatusCode],
    ) -> Result<(), CacheClientError> {
        let status = response.status();
        if ok_statuses.contains(&status) {
            Ok(())
        } else {
            Err(self.unexpected_status(response, status).await)
        }
    }

    async fn expect_json<T: DeserializeOwned>(
        &self,
        response: reqwest::Response,
        ok_statuses: &[StatusCode],
    ) -> Result<T, CacheClientError> {
        let status = response.status();
        if ok_statuses.contains(&status) {
            Ok(response.json().await?)
        } else {
            Err(self.unexpected_status(response, status).await)
        }
    }

    async fn unexpected_status(
        &self,
        response: reqwest::Response,
        status: StatusCode,
    ) -> CacheClientError {
        let body = response.text().await.unwrap_or_else(|error| {
            let mut message = String::from("<failed to read error body: ");
            let _ = write!(&mut message, "{error}>");
            message
        });

        CacheClientError::UnexpectedStatus { status, body }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::Json;
    use axum::Router;
    use axum::body::Bytes;
    use axum::extract::{Path, Query, State};
    use axum::http::{HeaderMap, StatusCode, header};
    use axum::routing::{delete, get, post, put};
    use serde::Deserialize;
    use serde_json::json;

    use cache_api::{
        AccessTokenInfo, BeginBuildRequest, BeginBuildResponse, CreateAccessTokenRequest,
        CreateAccessTokenResponse, CreatePinRequest, DeleteProjectOidcIdentityRequest, PinInfo,
        ProjectInfo, ProjectOidcIdentityInfo, RegisterPathsResponse, RunGcRequest, RunGcResponse,
        UpsertProjectOidcIdentityRequest, UpsertProjectRequest,
    };
    use cache_core::project::ProjectSlug;
    use cache_test_utils::{
        EXAMPLE_PROJECT_NAME, EXAMPLE_PROJECT_SLUG, TestServer, duplex_reader, hello_path,
    };

    use super::*;

    #[derive(Default, Clone)]
    struct TestState {
        auth_headers: Arc<Mutex<Vec<String>>>,
        uploaded_paths: Arc<Mutex<Vec<(String, String, String)>>>,
        uploaded_bodies: Arc<Mutex<Vec<Vec<u8>>>>,
        pin_queries: Arc<Mutex<Vec<Option<String>>>>,
    }

    #[derive(Debug, Deserialize)]
    struct PinQuery {
        project: Option<String>,
    }

    const TOKEN_ID: &str = "token-123";
    const ACCESS_TOKEN: &str = "npc_test_token";
    const BUILD_ID: &str = "build-123";
    const RELEASE_PIN_NAME: &str = "release";
    const RELEASE_STORE_PATH: &str = "/nix/store/example-release";

    async fn begin_build_handler(
        State(state): State<TestState>,
        headers: HeaderMap,
        Json(request): Json<BeginBuildRequest>,
    ) -> (StatusCode, Json<BeginBuildResponse>) {
        state.auth_headers.lock().unwrap().push(
            headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned(),
        );

        assert_eq!(request.project, EXAMPLE_PROJECT_SLUG);
        assert_eq!(request.ref_name, "main");
        assert_eq!(request.revision.as_deref(), Some("deadbeef"));

        (
            StatusCode::OK,
            Json(BeginBuildResponse {
                build_id: "build-123".to_owned(),
            }),
        )
    }

    async fn register_paths_handler(
        Json(request): Json<cache_api::RegisterPathsRequest>,
    ) -> (StatusCode, Json<RegisterPathsResponse>) {
        let path = hello_path();

        assert_eq!(request.build_id, BUILD_ID);
        assert_eq!(request.paths.len(), 1);
        assert_eq!(request.paths[0].store_path, path.store_path());
        assert_eq!(request.paths[0].url, path.url());

        (
            StatusCode::OK,
            Json(RegisterPathsResponse {
                required_uploads: vec![cache_api::RequiredUpload {
                    store_path_hash: path.hash_str(),
                    object_path: path.url().to_owned(),
                    content_type: "application/octet-stream".to_owned(),
                }],
            }),
        )
    }

    async fn upload_handler(
        State(state): State<TestState>,
        headers: HeaderMap,
        Path((build_id, store_path_hash, object_path)): Path<(String, String, String)>,
        body: Bytes,
    ) -> StatusCode {
        state.auth_headers.lock().unwrap().push(
            headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned(),
        );
        state
            .uploaded_paths
            .lock()
            .unwrap()
            .push((build_id, store_path_hash, object_path));
        state.uploaded_bodies.lock().unwrap().push(body.to_vec());

        StatusCode::NO_CONTENT
    }

    async fn finalize_handler() -> (StatusCode, Json<cache_api::FinalizeBuildResponse>) {
        (StatusCode::OK, Json(cache_api::FinalizeBuildResponse {}))
    }

    async fn list_pins_handler(
        State(state): State<TestState>,
        Query(query): Query<PinQuery>,
    ) -> (StatusCode, Json<Vec<PinInfo>>) {
        state
            .pin_queries
            .lock()
            .unwrap()
            .push(query.project.clone());

        (
            StatusCode::OK,
            Json(vec![PinInfo {
                name: RELEASE_PIN_NAME.to_owned(),
                project: query.project,
                store_path: RELEASE_STORE_PATH.to_owned(),
                created_at: "2026-04-20T00:00:00Z".to_owned(),
                updated_at: "2026-04-20T00:00:00Z".to_owned(),
            }]),
        )
    }

    async fn create_pin_handler(
        Path(name): Path<String>,
        Json(request): Json<CreatePinRequest>,
    ) -> StatusCode {
        assert_eq!(name, RELEASE_PIN_NAME);
        assert_eq!(request.project.as_deref(), Some(EXAMPLE_PROJECT_SLUG));
        assert_eq!(request.store_path, RELEASE_STORE_PATH);
        StatusCode::NO_CONTENT
    }

    async fn delete_pin_handler(
        State(state): State<TestState>,
        Path(name): Path<String>,
        Query(query): Query<PinQuery>,
    ) -> StatusCode {
        assert_eq!(name, RELEASE_PIN_NAME);
        state.pin_queries.lock().unwrap().push(query.project);

        StatusCode::NO_CONTENT
    }

    async fn list_projects_handler() -> (StatusCode, Json<Vec<ProjectInfo>>) {
        (
            StatusCode::OK,
            Json(vec![ProjectInfo {
                slug: EXAMPLE_PROJECT_SLUG.to_owned(),
                storage_id: None,
                display_name: EXAMPLE_PROJECT_NAME.to_owned(),
                public: true,
                created_at: "2026-04-20T00:00:00Z".to_owned(),
            }]),
        )
    }

    async fn upsert_project_handler(Json(request): Json<UpsertProjectRequest>) -> StatusCode {
        assert_eq!(request.slug, EXAMPLE_PROJECT_SLUG);
        assert_eq!(request.storage_id, None);
        assert_eq!(request.display_name, EXAMPLE_PROJECT_NAME);
        assert!(request.public);
        StatusCode::NO_CONTENT
    }

    async fn run_gc_handler(
        Json(request): Json<RunGcRequest>,
    ) -> (StatusCode, Json<RunGcResponse>) {
        assert!(request.dry_run);

        (
            StatusCode::OK,
            Json(RunGcResponse {
                deleted_objects: vec!["nar/stale.nar.zst".to_owned()],
                deleted_count: 1,
            }),
        )
    }

    async fn list_project_oidc_identities_handler(
        State(state): State<TestState>,
        headers: HeaderMap,
        Path(project): Path<String>,
    ) -> (StatusCode, Json<Vec<ProjectOidcIdentityInfo>>) {
        state.auth_headers.lock().unwrap().push(
            headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned(),
        );

        assert_eq!(project, EXAMPLE_PROJECT_SLUG);

        (
            StatusCode::OK,
            Json(vec![ProjectOidcIdentityInfo {
                provider: "github".to_owned(),
                repository: "owner/repo".to_owned(),
                ref_patterns: vec!["refs/heads/main".to_owned()],
            }]),
        )
    }

    async fn upsert_project_oidc_identity_handler(
        State(state): State<TestState>,
        headers: HeaderMap,
        Path(project): Path<String>,
        Json(request): Json<UpsertProjectOidcIdentityRequest>,
    ) -> StatusCode {
        state.auth_headers.lock().unwrap().push(
            headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned(),
        );

        assert_eq!(project, EXAMPLE_PROJECT_SLUG);
        assert_eq!(request.provider, "github");
        assert_eq!(request.repository, "owner/repo");
        assert_eq!(request.ref_patterns, vec!["refs/heads/main".to_owned()]);

        StatusCode::NO_CONTENT
    }

    async fn delete_project_oidc_identity_handler(
        State(state): State<TestState>,
        headers: HeaderMap,
        Path(project): Path<String>,
        Json(request): Json<DeleteProjectOidcIdentityRequest>,
    ) -> StatusCode {
        state.auth_headers.lock().unwrap().push(
            headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned(),
        );

        assert_eq!(project, EXAMPLE_PROJECT_SLUG);
        assert_eq!(request.provider, "github");
        assert_eq!(request.repository, "owner/repo");

        StatusCode::NO_CONTENT
    }

    async fn create_access_token_handler(
        State(state): State<TestState>,
        headers: HeaderMap,
        Json(request): Json<CreateAccessTokenRequest>,
    ) -> (StatusCode, Json<CreateAccessTokenResponse>) {
        state.auth_headers.lock().unwrap().push(
            headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned(),
        );

        assert_eq!(request.name, "ci-main");
        assert_eq!(request.project, EXAMPLE_PROJECT_SLUG);
        assert_eq!(request.ref_patterns, vec!["refs/heads/main".to_owned()]);
        assert_eq!(request.expires_at.as_deref(), Some("2030-01-01T00:00:00Z"));

        (
            StatusCode::OK,
            Json(CreateAccessTokenResponse {
                token: ACCESS_TOKEN.to_owned(),
                info: AccessTokenInfo {
                    id: TOKEN_ID.to_owned(),
                    name: "ci-main".to_owned(),
                    project: EXAMPLE_PROJECT_SLUG.to_owned(),
                    ref_patterns: vec!["refs/heads/main".to_owned()],
                    created_at: "2026-04-20T00:00:00Z".to_owned(),
                    expires_at: Some("2030-01-01T00:00:00Z".to_owned()),
                    revoked_at: None,
                },
            }),
        )
    }

    async fn list_access_tokens_handler(
        State(state): State<TestState>,
        headers: HeaderMap,
        Query(query): Query<PinQuery>,
    ) -> (StatusCode, Json<Vec<AccessTokenInfo>>) {
        state.auth_headers.lock().unwrap().push(
            headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned(),
        );

        assert_eq!(query.project.as_deref(), Some(EXAMPLE_PROJECT_SLUG));

        (
            StatusCode::OK,
            Json(vec![AccessTokenInfo {
                id: TOKEN_ID.to_owned(),
                name: "ci-main".to_owned(),
                project: EXAMPLE_PROJECT_SLUG.to_owned(),
                ref_patterns: vec!["refs/heads/main".to_owned()],
                created_at: "2026-04-20T00:00:00Z".to_owned(),
                expires_at: None,
                revoked_at: None,
            }]),
        )
    }

    async fn revoke_access_token_handler(
        State(state): State<TestState>,
        headers: HeaderMap,
        Path(token_id): Path<String>,
    ) -> StatusCode {
        state.auth_headers.lock().unwrap().push(
            headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned(),
        );

        assert_eq!(token_id, TOKEN_ID);

        StatusCode::NO_CONTENT
    }

    #[tokio::test]
    async fn begin_build_sends_auth_and_parses_response() {
        let state = TestState::default();
        let app = Router::new()
            .route("/api/builds", post(begin_build_handler))
            .with_state(state.clone());

        let server = TestServer::spawn(app).await.unwrap();
        let client = CacheClient::new(&server.base_url, "secret-token").unwrap();

        let response = client
            .begin_build(BeginBuildRequest {
                project: EXAMPLE_PROJECT_SLUG.to_owned(),
                ref_name: "main".to_owned(),
                revision: Some("deadbeef".to_owned()),
            })
            .await
            .unwrap();

        assert_eq!(response.build_id, BUILD_ID);
        assert_eq!(
            state.auth_headers.lock().unwrap().as_slice(),
            &["Bearer secret-token".to_owned()]
        );
    }

    #[tokio::test]
    async fn build_routes_accept_typed_inputs() {
        let state = TestState::default();
        let app = Router::new()
            .route("/api/builds/{build_id}/paths", post(register_paths_handler))
            .route(
                "/api/builds/{build_id}/paths/{store_path_hash}/objects/{*object_path}",
                put(upload_handler),
            )
            .route("/api/builds/{build_id}/finalize", post(finalize_handler))
            .with_state(state.clone());

        let server = TestServer::spawn(app).await.unwrap();
        let client = CacheClient::new(&server.base_url, "secret-token").unwrap();
        let path = hello_path();
        let hash = path.hash();

        let register = client
            .register_paths(BUILD_ID, vec![path.narinfo()])
            .await
            .unwrap();
        assert_eq!(register.required_uploads.len(), 1);

        client
            .upload_object_reader(
                BUILD_ID,
                &hash,
                path.url(),
                duplex_reader(b"hello streamed world"),
            )
            .await
            .unwrap();

        client.finalize_build(BUILD_ID).await.unwrap();

        assert_eq!(
            state.uploaded_paths.lock().unwrap().as_slice(),
            &[(
                BUILD_ID.to_owned(),
                hash.as_str().to_owned(),
                path.url().to_owned(),
            )]
        );
        assert_eq!(
            state.uploaded_bodies.lock().unwrap().as_slice(),
            &[b"hello streamed world".to_vec()]
        );
    }

    #[tokio::test]
    async fn pin_gc_and_project_methods_hit_expected_endpoints() {
        let state = TestState::default();
        let app = Router::new()
            .route("/api/pins", get(list_pins_handler))
            .route("/api/pins/{name}", post(create_pin_handler))
            .route("/api/pins/{name}", delete(delete_pin_handler))
            .route("/api/projects", get(list_projects_handler))
            .route("/api/projects", post(upsert_project_handler))
            .route("/api/gc", post(run_gc_handler))
            .with_state(state.clone());

        let server = TestServer::spawn(app).await.unwrap();
        let client = CacheClient::new(&server.base_url, "secret-token").unwrap();
        let project = ProjectSlug::parse("example_repo").unwrap();

        let pins = client.list_pins(Some(&project)).await.unwrap();
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].name, RELEASE_PIN_NAME);

        client
            .create_pin(RELEASE_PIN_NAME, Some(&project), RELEASE_STORE_PATH)
            .await
            .unwrap();

        assert!(
            client
                .delete_pin(RELEASE_PIN_NAME, Some(&project))
                .await
                .unwrap()
        );

        let projects = client.list_projects().await.unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].slug, EXAMPLE_PROJECT_SLUG);

        client
            .upsert_project(&project, EXAMPLE_PROJECT_NAME, true)
            .await
            .unwrap();

        let gc = client
            .run_gc(RunGcRequest {
                dry_run: true,
                grace_period_seconds: None,
            })
            .await
            .unwrap();
        assert_eq!(gc.deleted_count, 1);
        assert_eq!(gc.deleted_objects, vec!["nar/stale.nar.zst".to_owned()]);

        assert_eq!(
            state.pin_queries.lock().unwrap().as_slice(),
            &[
                Some(EXAMPLE_PROJECT_SLUG.to_owned()),
                Some(EXAMPLE_PROJECT_SLUG.to_owned())
            ]
        );
    }

    #[tokio::test]
    async fn project_oidc_identity_methods_hit_expected_endpoints() {
        let state = TestState::default();
        let app = Router::new()
            .route(
                "/api/projects/{project}/oidc-identities",
                get(list_project_oidc_identities_handler),
            )
            .route(
                "/api/projects/{project}/oidc-identities",
                post(upsert_project_oidc_identity_handler),
            )
            .route(
                "/api/projects/{project}/oidc-identities",
                delete(delete_project_oidc_identity_handler),
            )
            .with_state(state.clone());

        let server = TestServer::spawn(app).await.unwrap();
        let client = CacheClient::new(&server.base_url, "secret-token").unwrap();
        let project = ProjectSlug::parse(EXAMPLE_PROJECT_SLUG).unwrap();

        let identities = client.list_project_oidc_identities(&project).await.unwrap();
        assert_eq!(identities.len(), 1);
        assert_eq!(identities[0].provider, "github");
        assert_eq!(identities[0].repository, "owner/repo");
        assert_eq!(
            identities[0].ref_patterns,
            vec!["refs/heads/main".to_owned()]
        );

        client
            .upsert_project_oidc_identity(
                &project,
                UpsertProjectOidcIdentityRequest {
                    provider: "github".to_owned(),
                    repository: "owner/repo".to_owned(),
                    ref_patterns: vec!["refs/heads/main".to_owned()],
                },
            )
            .await
            .unwrap();

        assert!(
            client
                .delete_project_oidc_identity(
                    &project,
                    DeleteProjectOidcIdentityRequest {
                        provider: "github".to_owned(),
                        repository: "owner/repo".to_owned(),
                    },
                )
                .await
                .unwrap()
        );

        assert_eq!(
            state.auth_headers.lock().unwrap().as_slice(),
            &[
                "Bearer secret-token".to_owned(),
                "Bearer secret-token".to_owned(),
                "Bearer secret-token".to_owned(),
            ]
        );
    }

    #[tokio::test]
    async fn unexpected_status_returns_response_body() {
        let app = Router::new().route(
            "/api/projects",
            post(|| async {
                (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": "bad request"})),
                )
            }),
        );

        let server = TestServer::spawn(app).await.unwrap();
        let client = CacheClient::new(&server.base_url, "secret-token").unwrap();
        let project = ProjectSlug::parse(EXAMPLE_PROJECT_SLUG).unwrap();

        let error = client
            .upsert_project(&project, EXAMPLE_PROJECT_NAME, true)
            .await
            .unwrap_err();

        match error {
            CacheClientError::UnexpectedStatus { status, body } => {
                assert_eq!(status, StatusCode::BAD_REQUEST);
                assert!(body.contains("bad request"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn upload_object_bytes_works_for_small_payloads() {
        let state = TestState::default();
        let app = Router::new()
            .route(
                "/api/builds/{build_id}/paths/{store_path_hash}/objects/{*object_path}",
                put(upload_handler),
            )
            .with_state(state.clone());

        let server = TestServer::spawn(app).await.unwrap();
        let client = CacheClient::new(&server.base_url, "secret-token").unwrap();
        let path = hello_path();
        let hash = path.hash();

        client
            .upload_object_bytes(
                BUILD_ID,
                &hash,
                path.url(),
                bytes::Bytes::from_static(b"small"),
            )
            .await
            .unwrap();

        assert_eq!(
            state.uploaded_bodies.lock().unwrap().as_slice(),
            &[b"small".to_vec()]
        );
    }

    #[tokio::test]
    async fn access_token_methods_hit_expected_endpoints() {
        let state = TestState::default();
        let app = Router::new()
            .route("/api/access-tokens", post(create_access_token_handler))
            .route("/api/access-tokens", get(list_access_tokens_handler))
            .route(
                "/api/access-tokens/{token_id}",
                delete(revoke_access_token_handler),
            )
            .with_state(state.clone());

        let server = TestServer::spawn(app).await.unwrap();
        let client = CacheClient::new(&server.base_url, "secret-token").unwrap();
        let project = ProjectSlug::parse(EXAMPLE_PROJECT_SLUG).unwrap();

        let created = client
            .create_access_token(CreateAccessTokenRequest {
                name: "ci-main".to_owned(),
                project: EXAMPLE_PROJECT_SLUG.to_owned(),
                ref_patterns: vec!["refs/heads/main".to_owned()],
                expires_at: Some("2030-01-01T00:00:00Z".to_owned()),
            })
            .await
            .unwrap();

        assert_eq!(created.token, ACCESS_TOKEN);
        assert_eq!(created.info.id, TOKEN_ID);

        let tokens = client.list_access_tokens(Some(&project)).await.unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].name, "ci-main");

        assert!(client.revoke_access_token(TOKEN_ID).await.unwrap());

        assert_eq!(
            state.auth_headers.lock().unwrap().as_slice(),
            &[
                "Bearer secret-token".to_owned(),
                "Bearer secret-token".to_owned(),
                "Bearer secret-token".to_owned(),
            ]
        );
    }
}
