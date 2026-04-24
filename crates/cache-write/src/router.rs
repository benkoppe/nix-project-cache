use axum::Router;
use axum::routing::{delete, get, post, put};

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
            "/api/builds/{build_id}/paths/{store_path_hash}/objects/{*object_path}",
            put(handlers::upload_object),
        )
        .route(
            "/api/builds/{build_id}/finalize",
            post(handlers::finalize_build),
        )
        .route("/api/pins", get(handlers::list_pins))
        .route("/api/pins/{name}", post(handlers::create_pin))
        .route("/api/pins/{name}", delete(handlers::delete_pin))
        .route("/api/gc", post(handlers::run_gc))
        .route("/api/projects", get(handlers::list_projects))
        .route("/api/projects", post(handlers::upsert_project))
        .route(
            "/api/projects/{project}/oidc-identities",
            get(handlers::list_project_oidc_identities),
        )
        .route(
            "/api/projects/{project}/oidc-identities",
            post(handlers::upsert_project_oidc_identity),
        )
        .route(
            "/api/projects/{project}/oidc-identities",
            delete(handlers::delete_project_oidc_identity),
        )
        .route(
            "/api/projects/{project}/retention",
            get(handlers::get_project_retention_policy),
        )
        .route(
            "/api/projects/{project}/retention",
            put(handlers::upsert_project_retention_policy),
        )
        .route(
            "/api/projects/{project}/retention",
            delete(handlers::delete_project_retention_policy),
        )
        .route(
            "/api/projects/{project}/upstreams",
            get(handlers::list_project_upstreams),
        )
        .route(
            "/api/projects/{project}/upstreams/{upstream}",
            post(handlers::link_project_upstream),
        )
        .route(
            "/api/projects/{project}/upstreams/{upstream}",
            delete(handlers::unlink_project_upstream),
        )
        .route("/api/upstreams", get(handlers::list_upstreams))
        .route("/api/upstreams", post(handlers::upsert_upstream))
        .route(
            "/api/upstreams/{upstream}/enable",
            post(handlers::enable_upstream),
        )
        .route(
            "/api/upstreams/{upstream}/disable",
            post(handlers::disable_upstream),
        )
        .route("/api/access-tokens", get(handlers::list_access_tokens))
        .route("/api/access-tokens", post(handlers::create_access_token))
        .route(
            "/api/access-tokens/{token_id}",
            delete(handlers::revoke_access_token),
        )
        .with_state(state)
}
