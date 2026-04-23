use std::io;

use axum::Json;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use tokio_util::io::StreamReader;
use uuid::Uuid;

use cache_api::{
    BeginBuildRequest, CreatePinRequest, FinalizeBuildRequest, PinInfo, ProjectInfo,
    RegisterPathsRequest, RunGcRequest, UpsertProjectRequest,
};
use cache_auth::{AuthError, Authorizer};
use cache_core::nix::StorePathHash;
use cache_core::project::ProjectSlug;

use crate::state::WriteAppState;

#[derive(Debug, Deserialize)]
pub struct PinQuery {
    pub project: Option<String>,
}

pub async fn begin_build(
    State(state): State<WriteAppState>,
    headers: HeaderMap,
    Json(request): Json<BeginBuildRequest>,
) -> Response {
    if let Err(error) = authorize(&*state.authorizer, &headers).await {
        return error.into_response();
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
    headers: HeaderMap,
    Json(mut request): Json<RegisterPathsRequest>,
) -> Response {
    if let Err(error) = authorize(&*state.authorizer, &headers).await {
        return error.into_response();
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
    if let Err(error) = authorize(&*state.authorizer, request.headers()).await {
        return error.into_response();
    }

    let build_id = match Uuid::parse_str(&build_id) {
        Ok(build_id) => build_id,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

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
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authorize(&*state.authorizer, &headers).await {
        return error.into_response();
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
    headers: HeaderMap,
    Query(query): Query<PinQuery>,
) -> Response {
    if let Err(error) = authorize(&*state.authorizer, &headers).await {
        return error.into_response();
    }

    let project = match parse_optional_project(query.project.as_deref()) {
        Ok(project) => project,
        Err(status) => return status.into_response(),
    };

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
    headers: HeaderMap,
    Json(request): Json<CreatePinRequest>,
) -> Response {
    if let Err(error) = authorize(&*state.authorizer, &headers).await {
        return error.into_response();
    }

    let project = match parse_optional_project(request.project.as_deref()) {
        Ok(project) => project,
        Err(status) => return status.into_response(),
    };

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
    headers: HeaderMap,
    Query(query): Query<PinQuery>,
) -> Response {
    if let Err(error) = authorize(&*state.authorizer, &headers).await {
        return error.into_response();
    }

    let project = match parse_optional_project(query.project.as_deref()) {
        Ok(project) => project,
        Err(status) => return status.into_response(),
    };

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
    headers: HeaderMap,
    Json(request): Json<RunGcRequest>,
) -> Response {
    if let Err(error) = authorize(&*state.authorizer, &headers).await {
        return error.into_response();
    }

    match state.gc_service.run_local_gc(request.dry_run).await {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(error) => {
            tracing::error!(?error, "run_gc failed");
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}

pub async fn list_projects(State(state): State<WriteAppState>, headers: HeaderMap) -> Response {
    if let Err(error) = authorize(&*state.authorizer, &headers).await {
        return error.into_response();
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
    headers: HeaderMap,
    Json(request): Json<UpsertProjectRequest>,
) -> Response {
    if let Err(error) = authorize(&*state.authorizer, &headers).await {
        return error.into_response();
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

enum AuthorizeError {
    ServiceUnavailable,
    Unauthorized,
}

impl IntoResponse for AuthorizeError {
    fn into_response(self) -> Response {
        match self {
            Self::ServiceUnavailable => StatusCode::SERVICE_UNAVAILABLE.into_response(),
            Self::Unauthorized => StatusCode::UNAUTHORIZED.into_response(),
        }
    }
}

async fn authorize(authorizer: &dyn Authorizer, headers: &HeaderMap) -> Result<(), AuthorizeError> {
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim);

    match authorizer.authorize_bearer(bearer).await {
        Ok(_) => Ok(()),
        Err(AuthError::Disabled) | Err(AuthError::Unavailable(_)) => {
            Err(AuthorizeError::ServiceUnavailable)
        }
        Err(AuthError::MissingToken) | Err(AuthError::InvalidToken) => {
            Err(AuthorizeError::Unauthorized)
        }
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
