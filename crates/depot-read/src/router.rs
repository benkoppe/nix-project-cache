use axum::Router;
use axum::routing::get;

use crate::handlers;
use crate::state::ReadAppState;

pub fn read_router(state: ReadAppState) -> Router {
    Router::new()
        .route("/health", get(handlers::health))
        .route("/nix-cache-info", get(handlers::aggregate_cache_info))
        .route("/{*object}", get(handlers::aggregate_object))
        .route(
            "/p/{project}/nix-cache-info",
            get(handlers::project_cache_info),
        )
        .route("/p/{project}/{*object}", get(handlers::project_object))
        .with_state(state)
}
