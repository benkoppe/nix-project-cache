use std::io;

use axum::Json;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use tokio_util::io::StreamReader;
use uuid::Uuid;

use cache_api::{BeginBuildRequest, FinalizeBuildRequest, RegisterPathsRequest};
use cache_auth::{AuthError, Authorizer};
use cache_core::nix::StorePathHash;

use crate::state::WriteAppState;

pub async fn begin_build(
    State(state): State<WriteAppState>,
    headers: HeaderMap,
    Json(request): Json<BeginBuildRequest>,
) -> Response {
    if let Err(error) = authorize(&*state.authorizer, &headers) {
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
    if let Err(error) = authorize(&*state.authorizer, &headers) {
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
    if let Err(error) = authorize(&*state.authorizer, request.headers()) {
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
    if let Err(error) = authorize(&*state.authorizer, &headers) {
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

fn authorize(authorizer: &dyn Authorizer, headers: &HeaderMap) -> Result<(), AuthorizeError> {
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim);

    match authorizer.authorize_bearer(bearer) {
        Ok(_) => Ok(()),
        Err(AuthError::Disabled) => Err(AuthorizeError::ServiceUnavailable),
        Err(AuthError::MissingToken) | Err(AuthError::InvalidToken) => {
            Err(AuthorizeError::Unauthorized)
        }
    }
}
