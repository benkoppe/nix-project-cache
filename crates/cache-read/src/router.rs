use axum::Router;
use axum::routing::get;

use crate::handlers;
use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(handlers::health))
        .route("/nix-cache-info", get(handlers::aggregate_cache_info))
        .route("/{object}", get(handlers::aggregate_object))
        .route(
            "/p/{project}/nix-cache-info",
            get(handlers::project_cache_info),
        )
        .route("/p/{project}/{object}", get(handlers::project_object))
        .with_state(state)
}
