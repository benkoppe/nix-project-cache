use std::io::Write;

use anyhow::{Context as _, Result, bail};

use depot_api::{AccessTokenInfo, CreateAccessTokenRequest};
use depot_client::DepotClient;
use depot_core::project::ProjectSlug;

use crate::cli::{CreateTokenCommand, ListTokensCommand, RevokeTokenCommand, TokensCommand};
use crate::output;

pub async fn handle(
    client: &DepotClient,
    writer: &mut impl Write,
    json_output: bool,
    command: TokensCommand,
) -> Result<()> {
    match command {
        TokensCommand::List(command) => list_tokens(client, writer, json_output, command).await,
        TokensCommand::Create(command) => create_token(client, writer, json_output, command).await,
        TokensCommand::Revoke(command) => revoke_token(client, writer, json_output, command).await,
    }
}

async fn list_tokens(
    client: &DepotClient,
    writer: &mut impl Write,
    json_output: bool,
    command: ListTokensCommand,
) -> Result<()> {
    let project = command
        .project
        .as_deref()
        .map(parse_project_slug)
        .transpose()?;

    let tokens = client
        .list_access_tokens(project.as_ref())
        .await
        .context("listing access tokens")?;

    print_tokens(writer, json_output, &tokens)
}

async fn create_token(
    client: &DepotClient,
    writer: &mut impl Write,
    json_output: bool,
    command: CreateTokenCommand,
) -> Result<()> {
    if command.name.trim().is_empty() {
        bail!("token name must not be empty");
    }

    let project = parse_project_slug(&command.project)?;

    let response = client
        .create_access_token(CreateAccessTokenRequest {
            name: command.name,
            project: project.as_str().to_owned(),
            ref_patterns: command.ref_patterns,
            expires_at: command.expires_at,
        })
        .await
        .context("creating access token")?;

    if json_output {
        output::print_json(writer, &response)?;
    } else {
        writeln!(writer, "created token {}", response.info.id)?;
        writeln!(writer, "token: {}", response.token)?;
    }

    Ok(())
}

async fn revoke_token(
    client: &DepotClient,
    writer: &mut impl Write,
    json_output: bool,
    command: RevokeTokenCommand,
) -> Result<()> {
    let revoked = client
        .revoke_access_token(&command.token_id)
        .await
        .with_context(|| format!("revoking access token {}", command.token_id))?;

    if !revoked && !command.ignore_missing {
        bail!("access token {} does not exist", command.token_id);
    }

    if json_output {
        output::print_status_json(
            writer,
            if revoked { "revoked" } else { "missing" },
            [("token_id", serde_json::json!(command.token_id))],
        )?;
    } else if revoked {
        writeln!(writer, "revoked token {}", command.token_id)?;
    } else {
        writeln!(writer, "token {} was already absent", command.token_id)?;
    }

    Ok(())
}

fn parse_project_slug(slug: &str) -> Result<ProjectSlug> {
    ProjectSlug::parse(slug).map_err(|_| anyhow::anyhow!("invalid project slug {}", slug))
}

fn print_tokens(
    writer: &mut impl Write,
    json_output: bool,
    tokens: &[AccessTokenInfo],
) -> Result<()> {
    if json_output {
        output::print_json(writer, tokens)?;
    } else {
        for token in tokens {
            let refs = if token.ref_patterns.is_empty() {
                "*".to_owned()
            } else {
                token.ref_patterns.join(",")
            };

            writeln!(
                writer,
                "{}\t{}\t{}\trefs={}\trevoked={}",
                token.id,
                token.name,
                token.project,
                refs,
                token.revoked_at.is_some()
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::extract::{Path, Query, State};
    use axum::http::{HeaderMap, StatusCode, header};
    use axum::routing::{delete, get, post};
    use axum::{Json, Router};
    use serde::Deserialize;

    use depot_api::{CreateAccessTokenRequest, CreateAccessTokenResponse};
    use depot_test_utils::{EXAMPLE_PROJECT_SLUG, TestServer};

    use super::*;

    const TOKEN_ID: &str = "token-123";
    const ACCESS_TOKEN: &str = "npc_test_token";

    #[derive(Default, Clone)]
    struct TestState {
        auth_headers: Arc<Mutex<Vec<String>>>,
        create_requests: Arc<Mutex<Vec<CreateAccessTokenRequest>>>,
        revoked_tokens: Arc<Mutex<Vec<String>>>,
    }

    #[derive(Debug, Deserialize)]
    struct TokenQuery {
        project: Option<String>,
    }

    async fn create_token_handler(
        State(state): State<TestState>,
        headers: HeaderMap,
        Json(request): Json<CreateAccessTokenRequest>,
    ) -> (StatusCode, Json<CreateAccessTokenResponse>) {
        record_auth_header(&state, &headers);
        state.create_requests.lock().unwrap().push(request);

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
                    expires_at: None,
                    revoked_at: None,
                },
            }),
        )
    }

    async fn list_tokens_handler(
        State(state): State<TestState>,
        headers: HeaderMap,
        Query(query): Query<TokenQuery>,
    ) -> (StatusCode, Json<Vec<AccessTokenInfo>>) {
        record_auth_header(&state, &headers);
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

    async fn revoke_token_handler(
        State(state): State<TestState>,
        headers: HeaderMap,
        Path(token_id): Path<String>,
    ) -> StatusCode {
        record_auth_header(&state, &headers);
        state.revoked_tokens.lock().unwrap().push(token_id);
        StatusCode::NO_CONTENT
    }

    async fn missing_revoke_token_handler(Path(_token_id): Path<String>) -> StatusCode {
        StatusCode::NOT_FOUND
    }

    fn record_auth_header(state: &TestState, headers: &HeaderMap) {
        state.auth_headers.lock().unwrap().push(
            headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned(),
        );
    }

    fn client_for(server: &TestServer) -> DepotClient {
        DepotClient::new(&server.base_url, "secret-token").unwrap()
    }

    #[tokio::test]
    async fn create_token_prints_token_and_sends_request() {
        let state = TestState::default();
        let app = Router::new()
            .route("/api/access-tokens", post(create_token_handler))
            .with_state(state.clone());

        let server = TestServer::spawn(app).await.unwrap();
        let client = client_for(&server);

        let mut output = Vec::new();

        handle(
            &client,
            &mut output,
            false,
            TokensCommand::Create(CreateTokenCommand {
                name: "ci-main".to_owned(),
                project: EXAMPLE_PROJECT_SLUG.to_owned(),
                ref_patterns: vec!["refs/heads/main".to_owned()],
                expires_at: None,
            }),
        )
        .await
        .unwrap();

        let requests = state.create_requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].name, "ci-main");
        assert_eq!(requests[0].project, EXAMPLE_PROJECT_SLUG);
        assert_eq!(requests[0].ref_patterns, vec!["refs/heads/main".to_owned()]);

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("created token"));
        assert!(output.contains(ACCESS_TOKEN));
    }

    #[tokio::test]
    async fn create_token_prints_json() {
        let state = TestState::default();
        let app = Router::new()
            .route("/api/access-tokens", post(create_token_handler))
            .with_state(state);

        let server = TestServer::spawn(app).await.unwrap();
        let client = client_for(&server);

        let mut output = Vec::new();

        handle(
            &client,
            &mut output,
            true,
            TokensCommand::Create(CreateTokenCommand {
                name: "ci-main".to_owned(),
                project: EXAMPLE_PROJECT_SLUG.to_owned(),
                ref_patterns: vec!["refs/heads/main".to_owned()],
                expires_at: None,
            }),
        )
        .await
        .unwrap();

        let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(value["token"], ACCESS_TOKEN);
        assert_eq!(value["info"]["id"], TOKEN_ID);
    }

    #[tokio::test]
    async fn list_tokens_prints_human_output() {
        let state = TestState::default();
        let app = Router::new()
            .route("/api/access-tokens", get(list_tokens_handler))
            .with_state(state.clone());

        let server = TestServer::spawn(app).await.unwrap();
        let client = client_for(&server);

        let mut output = Vec::new();

        handle(
            &client,
            &mut output,
            false,
            TokensCommand::List(ListTokensCommand {
                project: Some(EXAMPLE_PROJECT_SLUG.to_owned()),
            }),
        )
        .await
        .unwrap();

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains(TOKEN_ID));
        assert!(output.contains("ci-main"));
        assert!(output.contains(EXAMPLE_PROJECT_SLUG));
        assert!(output.contains("refs=refs/heads/main"));
    }

    #[tokio::test]
    async fn revoke_token_sends_request() {
        let state = TestState::default();
        let app = Router::new()
            .route(
                "/api/access-tokens/{token_id}",
                delete(revoke_token_handler),
            )
            .with_state(state.clone());

        let server = TestServer::spawn(app).await.unwrap();
        let client = client_for(&server);

        let mut output = Vec::new();

        handle(
            &client,
            &mut output,
            false,
            TokensCommand::Revoke(RevokeTokenCommand {
                token_id: TOKEN_ID.to_owned(),
                ignore_missing: false,
            }),
        )
        .await
        .unwrap();

        assert_eq!(
            state.revoked_tokens.lock().unwrap().as_slice(),
            &[TOKEN_ID.to_owned()]
        );

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("revoked token"));
    }

    #[tokio::test]
    async fn revoke_token_ignore_missing_succeeds_for_not_found() {
        let app = Router::new().route(
            "/api/access-tokens/{token_id}",
            delete(missing_revoke_token_handler),
        );

        let server = TestServer::spawn(app).await.unwrap();
        let client = client_for(&server);

        let mut output = Vec::new();

        handle(
            &client,
            &mut output,
            false,
            TokensCommand::Revoke(RevokeTokenCommand {
                token_id: TOKEN_ID.to_owned(),
                ignore_missing: true,
            }),
        )
        .await
        .unwrap();

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("already absent"));
    }
}
