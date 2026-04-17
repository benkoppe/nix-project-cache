use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

use cache_core::cache_path::{CacheObjectPath, parse_cache_object_path};
use cache_core::project::ProjectSlug;
use cache_core::view::CacheView;

use crate::state::AppState;

pub async fn health() -> impl IntoResponse {
    (StatusCode::OK, "ok\n")
}

pub async fn aggregate_cache_info(State(state): State<AppState>) -> Response {
    cache_info_response(&state)
}

pub async fn project_cache_info(
    Path(project): Path<String>,
    State(state): State<AppState>,
) -> Response {
    if ProjectSlug::parse(&project).is_err() {
        return StatusCode::NOT_FOUND.into_response();
    }

    cache_info_response(&state)
}

pub async fn aggregate_object(
    Path(object): Path<String>,
    State(state): State<AppState>,
) -> Response {
    serve_object(CacheView::Aggregate, &object, &state).await
}

pub async fn project_object(
    Path((project, object)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Response {
    let project = match ProjectSlug::parse(&project) {
        Ok(project) => project,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };

    serve_object(CacheView::Project(project), &object, &state).await
}

fn cache_info_response(state: &AppState) -> Response {
    (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; charset=utf-8"),
        )],
        state.nix_cache_info_text(),
    )
        .into_response()
}

async fn serve_object(view: CacheView, object: &str, state: &AppState) -> Response {
    let store_path_hash = match parse_cache_object_path(object) {
        Some(CacheObjectPath::NarInfo { store_path_hash }) => store_path_hash,
        _ => return StatusCode::NOT_FOUND.into_response(),
    };

    let narinfo = match state
        .resolver
        .resolve_narinfo(&view, &store_path_hash)
        .await
    {
        Ok(Some(narinfo)) => narinfo,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(error) => {
            tracing::error!(?error, "failed to resolve narinfo");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let signatures = match state.signer.sign(&narinfo) {
        Ok(signatures) => signatures,
        Err(error) => {
            tracing::error!(?error, "failed to sign narinfo");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let rendered = match state.renderer.render_with_signatures(&narinfo, &signatures) {
        Ok(rendered) => rendered,
        Err(error) => {
            tracing::error!(?error, "failed to render narinfo");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/x-nix-narinfo"),
        )],
        rendered,
    )
        .into_response()
}
