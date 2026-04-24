use std::collections::BTreeMap;
use std::io;

use axum::Json;
use axum::extract::{Path, Query, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio_util::io::StreamReader;
use uuid::Uuid;

use cache_api::{
    AccessTokenInfo, BeginBuildRequest, CreateAccessTokenRequest, CreateAccessTokenResponse,
    CreatePinRequest, DeleteProjectOidcIdentityRequest, FinalizeBuildRequest, PinInfo, ProjectInfo,
    ProjectOidcIdentityInfo, ProjectRetentionPolicyInfo, ProjectRetentionRuleInfo,
    RegisterPathsRequest, RunGcRequest, UpsertProjectOidcIdentityRequest, UpsertProjectRequest,
    UpsertProjectRetentionPolicyRequest, UpsertUpstreamRequest, UpstreamInfo,
};
use cache_core::nix::StorePathHash;
use cache_core::project::ProjectSlug;

use crate::authz::{AuthorizationServiceError, AuthorizedPrincipal};
use crate::state::WriteAppState;

#[derive(Debug, Deserialize)]
pub struct PinQuery {
    pub project: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AccessTokenQuery {
    pub project: Option<String>,
}

pub async fn begin_build(
    State(state): State<WriteAppState>,
    headers: axum::http::HeaderMap,
    Json(request): Json<BeginBuildRequest>,
) -> Response {
    let principal = match state
        .authorization_service
        .authorize_headers(&headers)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return authorization_error_response(error),
    };

    let project = match ProjectSlug::parse(&request.project) {
        Ok(project) => project,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    if let Err(error) = principal.require_project_ref(&project, &request.ref_name) {
        return authorization_error_response(error);
    }

    match state.ingest_service.begin_build(request).await {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(error) => {
            tracing::error!(?error, "begin_build failed");
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}

pub async fn register_paths(
    Path(build_id): Path<String>,
    State(state): State<WriteAppState>,
    headers: axum::http::HeaderMap,
    Json(mut request): Json<RegisterPathsRequest>,
) -> Response {
    let principal = match state
        .authorization_service
        .authorize_headers(&headers)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return authorization_error_response(error),
    };

    let build_id_uuid = match Uuid::parse_str(&build_id) {
        Ok(build_id) => build_id,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    if let Err(response) = require_build_access(&state, &principal, build_id_uuid).await {
        return response;
    }

    request.build_id = build_id;

    match state.ingest_service.register_paths(request).await {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(error) => {
            tracing::error!(?error, "register_paths failed");
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}

pub async fn upload_object(
    Path((build_id, store_path_hash, object_path)): Path<(String, String, String)>,
    State(state): State<WriteAppState>,
    request: Request,
) -> Response {
    let principal = match state
        .authorization_service
        .authorize_headers(request.headers())
        .await
    {
        Ok(principal) => principal,
        Err(error) => return authorization_error_response(error),
    };

    let build_id = match Uuid::parse_str(&build_id) {
        Ok(build_id) => build_id,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    if let Err(response) = require_build_access(&state, &principal, build_id).await {
        return response;
    }

    let Ok(store_path_hash) = StorePathHash::from_hash(&store_path_hash) else {
        return StatusCode::BAD_REQUEST.into_response();
    };

    let body_stream = futures_util::TryStreamExt::map_err(
        request.into_body().into_data_stream(),
        io::Error::other,
    );
    let body_reader = Box::pin(StreamReader::new(body_stream));

    match state
        .ingest_service
        .upload_object(build_id, &store_path_hash, &object_path, body_reader)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => {
            tracing::error!(?error, "upload_object failed");
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}

pub async fn finalize_build(
    Path(build_id): Path<String>,
    State(state): State<WriteAppState>,
    headers: axum::http::HeaderMap,
) -> Response {
    let principal = match state
        .authorization_service
        .authorize_headers(&headers)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return authorization_error_response(error),
    };

    let build_id_uuid = match Uuid::parse_str(&build_id) {
        Ok(build_id) => build_id,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    if let Err(response) = require_build_access(&state, &principal, build_id_uuid).await {
        return response;
    }

    match state
        .ingest_service
        .finalize_build(FinalizeBuildRequest { build_id })
        .await
    {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(error) => {
            tracing::error!(?error, "finalize_build failed");
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}

pub async fn list_pins(
    State(state): State<WriteAppState>,
    headers: axum::http::HeaderMap,
    Query(query): Query<PinQuery>,
) -> Response {
    let principal = match state
        .authorization_service
        .authorize_headers(&headers)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return authorization_error_response(error),
    };

    let project = match parse_optional_project(query.project.as_deref()) {
        Ok(project) => project,
        Err(status) => return status.into_response(),
    };

    let permission = match &project {
        Some(project) => principal.require_project(project),
        None => principal.require_admin(),
    };
    if let Err(error) = permission {
        return authorization_error_response(error);
    }

    match state.db.list_pins(project.as_ref()).await {
        Ok(pins) => (
            StatusCode::OK,
            Json(
                pins.into_iter()
                    .map(|pin| PinInfo {
                        name: pin.name,
                        project: pin.project_slug.map(|slug| slug.as_str().to_owned()),
                        store_path: pin.store_path,
                        created_at: pin.created_at.to_string(),
                        updated_at: pin.updated_at.to_string(),
                    })
                    .collect::<Vec<_>>(),
            ),
        )
            .into_response(),
        Err(error) => {
            tracing::error!(?error, "list_pins failed");
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}

pub async fn create_pin(
    Path(name): Path<String>,
    State(state): State<WriteAppState>,
    headers: axum::http::HeaderMap,
    Json(request): Json<CreatePinRequest>,
) -> Response {
    let principal = match state
        .authorization_service
        .authorize_headers(&headers)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return authorization_error_response(error),
    };

    let project = match parse_optional_project(request.project.as_deref()) {
        Ok(project) => project,
        Err(status) => return status.into_response(),
    };

    let permission = match &project {
        Some(project) => principal.require_project(project),
        None => principal.require_admin(),
    };
    if let Err(error) = permission {
        return authorization_error_response(error);
    }

    let store_path_hash = match StorePathHash::parse_from_store_path(&request.store_path) {
        Ok(hash) => hash,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    match state
        .db
        .upsert_pin(
            &name,
            project.as_ref(),
            &store_path_hash,
            &request.store_path,
        )
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => {
            tracing::error!(?error, "create_pin failed");
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}

pub async fn delete_pin(
    Path(name): Path<String>,
    State(state): State<WriteAppState>,
    headers: axum::http::HeaderMap,
    Query(query): Query<PinQuery>,
) -> Response {
    let principal = match state
        .authorization_service
        .authorize_headers(&headers)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return authorization_error_response(error),
    };

    let project = match parse_optional_project(query.project.as_deref()) {
        Ok(project) => project,
        Err(status) => return status.into_response(),
    };

    let permission = match &project {
        Some(project) => principal.require_project(project),
        None => principal.require_admin(),
    };
    if let Err(error) = permission {
        return authorization_error_response(error);
    }

    match state.db.delete_pin(&name, project.as_ref()).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => {
            tracing::error!(?error, "delete_pin failed");
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}

pub async fn run_gc(
    State(state): State<WriteAppState>,
    headers: axum::http::HeaderMap,
    Json(request): Json<RunGcRequest>,
) -> Response {
    let principal = match state
        .authorization_service
        .authorize_headers(&headers)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return authorization_error_response(error),
    };

    if let Err(error) = principal.require_admin() {
        return authorization_error_response(error);
    }

    let gc_result = match request.grace_period_seconds {
        Some(grace_period_seconds) => match i64::try_from(grace_period_seconds) {
            Ok(grace_period_seconds) => {
                state
                    .gc_service
                    .run_local_gc_with_grace_period(request.dry_run, grace_period_seconds)
                    .await
            }
            Err(_) => return StatusCode::BAD_REQUEST.into_response(),
        },
        None => state.gc_service.run_local_gc(request.dry_run).await,
    };

    match gc_result {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(error) => {
            tracing::error!(?error, "run_gc failed");
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}

pub async fn list_projects(
    State(state): State<WriteAppState>,
    headers: axum::http::HeaderMap,
) -> Response {
    let principal = match state
        .authorization_service
        .authorize_headers(&headers)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return authorization_error_response(error),
    };

    if let Err(error) = principal.require_admin() {
        return authorization_error_response(error);
    }

    match state.db.list_projects().await {
        Ok(projects) => (
            StatusCode::OK,
            Json(
                projects
                    .into_iter()
                    .map(|project| ProjectInfo {
                        slug: project.slug.as_str().to_owned(),
                        display_name: project.display_name,
                        public: project.public,
                        created_at: project.created_at.to_string(),
                    })
                    .collect::<Vec<_>>(),
            ),
        )
            .into_response(),
        Err(error) => {
            tracing::error!(?error, "list_projects failed");
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}

pub async fn upsert_project(
    State(state): State<WriteAppState>,
    headers: axum::http::HeaderMap,
    Json(request): Json<UpsertProjectRequest>,
) -> Response {
    let principal = match state
        .authorization_service
        .authorize_headers(&headers)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return authorization_error_response(error),
    };

    if let Err(error) = principal.require_admin() {
        return authorization_error_response(error);
    }

    let project = match ProjectSlug::parse(&request.slug) {
        Ok(project) => project,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    match state
        .db
        .insert_project(&project, &request.display_name, request.public)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => {
            tracing::error!(?error, "upsert_project failed");
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}

pub async fn list_project_oidc_identities(
    Path(project): Path<String>,
    State(state): State<WriteAppState>,
    headers: axum::http::HeaderMap,
) -> Response {
    let principal = match state
        .authorization_service
        .authorize_headers(&headers)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return authorization_error_response(error),
    };

    if let Err(error) = principal.require_admin() {
        return authorization_error_response(error);
    }

    let project = match ProjectSlug::parse(&project) {
        Ok(project) => project,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    match state.db.list_project_oidc_identities(&project).await {
        Ok(rows) => {
            let mut grouped = BTreeMap::<(String, String), Vec<String>>::new();

            for row in rows {
                let key = (row.provider, row.repository);
                let entry = grouped.entry(key).or_default();
                if let Some(pattern) = row.ref_pattern {
                    entry.push(pattern);
                }
            }

            let items = grouped
                .into_iter()
                .map(|((provider, repository), mut ref_patterns)| {
                    ref_patterns.sort();
                    ref_patterns.dedup();
                    ProjectOidcIdentityInfo {
                        provider,
                        repository,
                        ref_patterns,
                    }
                })
                .collect::<Vec<_>>();

            (StatusCode::OK, Json(items)).into_response()
        }
        Err(error) => {
            tracing::error!(?error, "list_project_oidc_identities failed");
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}

pub async fn upsert_project_oidc_identity(
    Path(project): Path<String>,
    State(state): State<WriteAppState>,
    headers: axum::http::HeaderMap,
    Json(request): Json<UpsertProjectOidcIdentityRequest>,
) -> Response {
    let principal = match state
        .authorization_service
        .authorize_headers(&headers)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return authorization_error_response(error),
    };

    if let Err(error) = principal.require_admin() {
        return authorization_error_response(error);
    }

    let project = match ProjectSlug::parse(&project) {
        Ok(project) => project,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    match state
        .db
        .replace_project_oidc_identity(
            &project,
            &request.provider,
            &request.repository,
            &request.ref_patterns,
        )
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => {
            tracing::error!(?error, "upsert_project_oidc_identity failed");
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}

pub async fn delete_project_oidc_identity(
    Path(project): Path<String>,
    State(state): State<WriteAppState>,
    headers: axum::http::HeaderMap,
    Json(request): Json<DeleteProjectOidcIdentityRequest>,
) -> Response {
    let principal = match state
        .authorization_service
        .authorize_headers(&headers)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return authorization_error_response(error),
    };

    if let Err(error) = principal.require_admin() {
        return authorization_error_response(error);
    }

    let project = match ProjectSlug::parse(&project) {
        Ok(project) => project,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    match state
        .db
        .delete_project_oidc_identity(&project, &request.provider, &request.repository)
        .await
    {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => {
            tracing::error!(?error, "delete_project_oidc_identity failed");
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}

pub async fn get_project_retention_policy(
    Path(project): Path<String>,
    State(state): State<WriteAppState>,
    headers: axum::http::HeaderMap,
) -> Response {
    let principal = match state
        .authorization_service
        .authorize_headers(&headers)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return authorization_error_response(error),
    };

    if let Err(error) = principal.require_admin() {
        return authorization_error_response(error);
    }

    let project = match ProjectSlug::parse(&project) {
        Ok(project) => project,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    match state.db.get_project_retention_policy(&project).await {
        Ok(policy) => (StatusCode::OK, Json(retention_policy_info(policy))).into_response(),
        Err(error) => {
            tracing::error!(?error, "get_project_retention_policy failed");
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}

pub async fn upsert_project_retention_policy(
    Path(project): Path<String>,
    State(state): State<WriteAppState>,
    headers: axum::http::HeaderMap,
    Json(request): Json<UpsertProjectRetentionPolicyRequest>,
) -> Response {
    let principal = match state
        .authorization_service
        .authorize_headers(&headers)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return authorization_error_response(error),
    };

    if let Err(error) = principal.require_admin() {
        return authorization_error_response(error);
    }

    let project = match ProjectSlug::parse(&project) {
        Ok(project) => project,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    let rules = request
        .rules
        .into_iter()
        .map(|rule| cache_db::ProjectRetentionRuleRecord {
            priority: rule.priority,
            ref_pattern: rule.ref_pattern,
            ttl_seconds: rule.ttl_seconds,
            keep_builds: rule.keep_builds,
        })
        .collect::<Vec<_>>();

    match state
        .db
        .replace_project_retention_policy(
            &project,
            request.keep_latest_builds_per_ref,
            request.object_delete_grace_seconds,
            &rules,
        )
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => {
            tracing::error!(?error, "upsert_project_retention_policy failed");
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}

pub async fn delete_project_retention_policy(
    Path(project): Path<String>,
    State(state): State<WriteAppState>,
    headers: axum::http::HeaderMap,
) -> Response {
    let principal = match state
        .authorization_service
        .authorize_headers(&headers)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return authorization_error_response(error),
    };

    if let Err(error) = principal.require_admin() {
        return authorization_error_response(error);
    }

    let project = match ProjectSlug::parse(&project) {
        Ok(project) => project,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    match state.db.delete_project_retention_policy(&project).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => {
            tracing::error!(?error, "delete_project_retention_policy failed");
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}

fn retention_policy_info(
    policy: cache_db::ProjectRetentionPolicyRecord,
) -> ProjectRetentionPolicyInfo {
    ProjectRetentionPolicyInfo {
        project: policy.project_slug.as_str().to_owned(),
        inherited_default: policy.inherited_default,
        keep_latest_builds_per_ref: policy.keep_latest_builds_per_ref,
        object_delete_grace_seconds: policy.object_delete_grace_seconds,
        rules: policy
            .rules
            .into_iter()
            .map(|rule| ProjectRetentionRuleInfo {
                priority: rule.priority,
                ref_pattern: rule.ref_pattern,
                ttl_seconds: rule.ttl_seconds,
                keep_builds: rule.keep_builds,
            })
            .collect(),
    }
}

pub async fn list_upstreams(
    State(state): State<WriteAppState>,
    headers: axum::http::HeaderMap,
) -> Response {
    let principal = match state
        .authorization_service
        .authorize_headers(&headers)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return authorization_error_response(error),
    };

    if let Err(error) = principal.require_admin() {
        return authorization_error_response(error);
    }

    match state.db.list_upstream_caches().await {
        Ok(upstreams) => (StatusCode::OK, Json(upstream_infos(upstreams))).into_response(),
        Err(error) => {
            tracing::error!(?error, "list_upstreams failed");
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}

pub async fn upsert_upstream(
    State(state): State<WriteAppState>,
    headers: axum::http::HeaderMap,
    Json(request): Json<UpsertUpstreamRequest>,
) -> Response {
    let principal = match state
        .authorization_service
        .authorize_headers(&headers)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return authorization_error_response(error),
    };

    if let Err(error) = principal.require_admin() {
        return authorization_error_response(error);
    }

    if request.name.trim().is_empty() || request.base_url.trim().is_empty() {
        return StatusCode::BAD_REQUEST.into_response();
    }

    if reqwest::Url::parse(&request.base_url).is_err() {
        return StatusCode::BAD_REQUEST.into_response();
    }

    match state
        .db
        .upsert_upstream_cache_by_name(
            request.name.trim(),
            request.base_url.trim(),
            request.priority,
            request.enabled,
        )
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => {
            tracing::error!(?error, "upsert_upstream failed");
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}

pub async fn enable_upstream(
    Path(upstream): Path<String>,
    State(state): State<WriteAppState>,
    headers: axum::http::HeaderMap,
) -> Response {
    set_upstream_enabled(state, headers, upstream, true).await
}

pub async fn disable_upstream(
    Path(upstream): Path<String>,
    State(state): State<WriteAppState>,
    headers: axum::http::HeaderMap,
) -> Response {
    set_upstream_enabled(state, headers, upstream, false).await
}

async fn set_upstream_enabled(
    state: WriteAppState,
    headers: axum::http::HeaderMap,
    upstream: String,
    enabled: bool,
) -> Response {
    let principal = match state
        .authorization_service
        .authorize_headers(&headers)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return authorization_error_response(error),
    };

    if let Err(error) = principal.require_admin() {
        return authorization_error_response(error);
    }

    match state.db.set_upstream_enabled(&upstream, enabled).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => {
            tracing::error!(?error, "set_upstream_enabled failed");
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}

pub async fn list_project_upstreams(
    Path(project): Path<String>,
    State(state): State<WriteAppState>,
    headers: axum::http::HeaderMap,
) -> Response {
    let principal = match state
        .authorization_service
        .authorize_headers(&headers)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return authorization_error_response(error),
    };

    if let Err(error) = principal.require_admin() {
        return authorization_error_response(error);
    }

    let project = match ProjectSlug::parse(&project) {
        Ok(project) => project,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    match state.db.list_upstreams_for_project(&project).await {
        Ok(upstreams) => (StatusCode::OK, Json(upstream_infos(upstreams))).into_response(),
        Err(error) => {
            tracing::error!(?error, "list_project_upstreams failed");
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}

pub async fn link_project_upstream(
    Path((project, upstream)): Path<(String, String)>,
    State(state): State<WriteAppState>,
    headers: axum::http::HeaderMap,
) -> Response {
    let principal = match state
        .authorization_service
        .authorize_headers(&headers)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return authorization_error_response(error),
    };

    if let Err(error) = principal.require_admin() {
        return authorization_error_response(error);
    }

    let project = match ProjectSlug::parse(&project) {
        Ok(project) => project,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    match state
        .db
        .link_project_upstream_by_name(&project, &upstream)
        .await
    {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => {
            tracing::error!(?error, "link_project_upstream failed");
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}

pub async fn unlink_project_upstream(
    Path((project, upstream)): Path<(String, String)>,
    State(state): State<WriteAppState>,
    headers: axum::http::HeaderMap,
) -> Response {
    let principal = match state
        .authorization_service
        .authorize_headers(&headers)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return authorization_error_response(error),
    };

    if let Err(error) = principal.require_admin() {
        return authorization_error_response(error);
    }

    let project = match ProjectSlug::parse(&project) {
        Ok(project) => project,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    match state
        .db
        .unlink_project_upstream_by_name(&project, &upstream)
        .await
    {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => {
            tracing::error!(?error, "unlink_project_upstream failed");
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}

fn upstream_infos(upstreams: Vec<cache_db::UpstreamCacheRecord>) -> Vec<UpstreamInfo> {
    upstreams
        .into_iter()
        .map(|upstream| UpstreamInfo {
            name: upstream.name,
            base_url: upstream.base_url,
            priority: upstream.priority,
            enabled: upstream.enabled,
            created_at: upstream.created_at.to_string(),
        })
        .collect()
}

pub async fn create_access_token(
    State(state): State<WriteAppState>,
    headers: axum::http::HeaderMap,
    Json(request): Json<CreateAccessTokenRequest>,
) -> Response {
    let principal = match state
        .authorization_service
        .authorize_headers(&headers)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return authorization_error_response(error),
    };

    if let Err(error) = principal.require_admin() {
        return authorization_error_response(error);
    }

    let project = match ProjectSlug::parse(&request.project) {
        Ok(project) => project,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    let expires_at = match request.expires_at.as_deref() {
        Some(value) => match OffsetDateTime::parse(value, &Rfc3339) {
            Ok(value) => Some(value),
            Err(_) => return StatusCode::BAD_REQUEST.into_response(),
        },
        None => None,
    };

    match state
        .db
        .create_access_token(&request.name, &project, &request.ref_patterns, expires_at)
        .await
    {
        Ok(created) => {
            let mut infos = access_token_info_from_records(created.records);
            let Some(info) = infos.pop() else {
                tracing::error!("created access token returned no metadata");
                return StatusCode::BAD_REQUEST.into_response();
            };
            (
                StatusCode::OK,
                Json(CreateAccessTokenResponse {
                    token: created.token,
                    info,
                }),
            )
                .into_response()
        }
        Err(error) => {
            tracing::error!(?error, "create_access_token failed");
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}

pub async fn list_access_tokens(
    State(state): State<WriteAppState>,
    headers: axum::http::HeaderMap,
    Query(query): Query<AccessTokenQuery>,
) -> Response {
    let principal = match state
        .authorization_service
        .authorize_headers(&headers)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return authorization_error_response(error),
    };

    if let Err(error) = principal.require_admin() {
        return authorization_error_response(error);
    }

    let project = match parse_optional_project(query.project.as_deref()) {
        Ok(project) => project,
        Err(status) => return status.into_response(),
    };

    match state.db.list_access_tokens(project.as_ref()).await {
        Ok(records) => (
            StatusCode::OK,
            Json(access_token_info_from_records(records)),
        )
            .into_response(),
        Err(error) => {
            tracing::error!(?error, "list_access_tokens failed");
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}

pub async fn revoke_access_token(
    Path(token_id): Path<String>,
    State(state): State<WriteAppState>,
    headers: axum::http::HeaderMap,
) -> Response {
    let principal = match state
        .authorization_service
        .authorize_headers(&headers)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return authorization_error_response(error),
    };

    if let Err(error) = principal.require_admin() {
        return authorization_error_response(error);
    }

    match state.db.revoke_access_token(&token_id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => {
            tracing::error!(?error, "revoke_access_token failed");
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}

async fn require_build_access(
    state: &WriteAppState,
    principal: &AuthorizedPrincipal,
    build_id: Uuid,
) -> Result<(), Response> {
    let build_context = match state.db.get_build_context(build_id).await {
        Ok(Some(context)) => context,
        Ok(None) => return Err(StatusCode::BAD_REQUEST.into_response()),
        Err(error) => {
            tracing::error!(?error, build_id = %build_id, "failed to fetch build context");
            return Err(StatusCode::BAD_REQUEST.into_response());
        }
    };

    principal
        .require_project_ref(&build_context.project_slug, &build_context.ref_name)
        .map_err(authorization_error_response)
}

fn authorization_error_response(error: AuthorizationServiceError) -> Response {
    match error {
        AuthorizationServiceError::ServiceUnavailable => {
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
        AuthorizationServiceError::Unauthorized => StatusCode::UNAUTHORIZED.into_response(),
        AuthorizationServiceError::Forbidden => StatusCode::FORBIDDEN.into_response(),
    }
}

fn parse_optional_project(project: Option<&str>) -> Result<Option<ProjectSlug>, StatusCode> {
    match project {
        Some(project) => ProjectSlug::parse(project)
            .map(Some)
            .map_err(|_| StatusCode::BAD_REQUEST),
        None => Ok(None),
    }
}

fn access_token_info_from_records(
    records: Vec<cache_db::AccessTokenRecord>,
) -> Vec<AccessTokenInfo> {
    let mut grouped = BTreeMap::<
        (
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
        ),
        Vec<String>,
    >::new();

    for record in records {
        let key = (
            record.id,
            record.name,
            record.project_slug.as_str().to_owned(),
            record.created_at.to_string(),
            record.expires_at.map(|value| value.to_string()),
            record.revoked_at.map(|value| value.to_string()),
        );

        let entry = grouped.entry(key).or_default();
        if let Some(pattern) = record.ref_pattern {
            entry.push(pattern);
        }
    }

    grouped
        .into_iter()
        .map(
            |((id, name, project, created_at, expires_at, revoked_at), mut ref_patterns)| {
                ref_patterns.sort();
                ref_patterns.dedup();
                AccessTokenInfo {
                    id,
                    name,
                    project,
                    ref_patterns,
                    created_at,
                    expires_at,
                    revoked_at,
                }
            },
        )
        .collect()
}
