use axum::Router;
use axum::routing::{post, put};

use crate::handlers;
use crate::state::WriteAppState;

pub fn write_router(state: WriteAppState) -> Router {
    Router::new()
        .route("/api/builds", post(handlers::begin_build))
        .route(
            "/api/builds/{build_id}/paths",
            post(handlers::register_paths),
        )
        .route(
            "/api/builds/{build_id}/objects/{*object_path}",
            put(handlers::upload_object),
        )
        .route(
            "/api/builds/{build_id}/finalize",
            post(handlers::finalize_build),
        )
        .with_state(state)
}
