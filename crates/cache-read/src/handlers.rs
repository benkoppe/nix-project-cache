use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

use cache_core::cache_path::{CacheObjectPath, parse_cache_object_path};
use cache_core::project::ProjectSlug;
use cache_core::view::CacheView;

use cache_store::blob::{BlobBytes, BlobMetadata};

use crate::state::ReadAppState;

pub async fn health() -> impl IntoResponse {
    (StatusCode::OK, "ok\n")
}

pub async fn aggregate_cache_info(State(state): State<ReadAppState>) -> Response {
    cache_info_response(&state, &CacheView::Aggregate).await
}

pub async fn project_cache_info(
    Path(project): Path<String>,
    State(state): State<ReadAppState>,
) -> Response {
    let project = match ProjectSlug::parse(&project) {
        Ok(project) => project,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };

    cache_info_response(&state, &CacheView::Project(project)).await
}

pub async fn aggregate_object(
    Path(object): Path<String>,
    State(state): State<ReadAppState>,
) -> Response {
    serve_object(CacheView::Aggregate, &object, &state).await
}

pub async fn project_object(
    Path((project, object)): Path<(String, String)>,
    State(state): State<ReadAppState>,
) -> Response {
    let project = match ProjectSlug::parse(&project) {
        Ok(project) => project,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };

    serve_object(CacheView::Project(project), &object, &state).await
}

async fn cache_info_response(state: &ReadAppState, view: &CacheView) -> Response {
    match state.nix_cache_info_text(view).await {
        Ok(text) => (
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            )],
            text,
        )
            .into_response(),
        Err(error) => {
            tracing::error!(?error, "failed to render nix-cache-info");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn serve_object(view: CacheView, object: &str, state: &ReadAppState) -> Response {
    match parse_cache_object_path(object) {
        Some(CacheObjectPath::NarInfo { store_path_hash }) => {
            match state
                .read_service
                .render_narinfo(&view, &store_path_hash)
                .await
            {
                Ok(Some(rendered)) => (
                    [(
                        header::CONTENT_TYPE,
                        HeaderValue::from_static("text/x-nix-narinfo"),
                    )],
                    rendered,
                )
                    .into_response(),
                Ok(None) => StatusCode::NOT_FOUND.into_response(),
                Err(error) => {
                    tracing::error!(?error, "failed to render narinfo");
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Some(CacheObjectPath::Nar { .. }) => {
            match state.read_service.get_object(&view, object).await {
                Ok(Some((metadata, bytes))) => blob_response(metadata, bytes),
                Ok(None) => StatusCode::NOT_FOUND.into_response(),
                Err(error) => {
                    tracing::error!(?error, "failed to fetch object");
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

fn blob_response(metadata: BlobMetadata, bytes: BlobBytes) -> Response {
    let mut response = bytes.into_response();

    let content_type = HeaderValue::from_str(&metadata.content_type)
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, content_type);

    if let Some(content_length) = metadata.content_length
        && let Ok(header_value) = HeaderValue::from_str(&content_length.to_string())
    {
        response
            .headers_mut()
            .insert(header::CONTENT_LENGTH, header_value);
    }

    if let Some(etag) = metadata.etag
        && let Ok(header_value) = HeaderValue::from_str(&etag)
    {
        response.headers_mut().insert(header::ETAG, header_value);
    }

    response
}
