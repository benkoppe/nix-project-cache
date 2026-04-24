use std::io::Write;

use anyhow::{Context as _, Result};

use cache_api::PinInfo;
use cache_client::CacheClient;
use cache_core::project::ProjectSlug;

use crate::cli::{CreatePinCommand, DeletePinCommand, ListPinsCommand, PinsCommand};
use crate::output;

pub async fn handle(
    client: &CacheClient,
    writer: &mut impl Write,
    json_output: bool,
    command: PinsCommand,
) -> Result<()> {
    match command {
        PinsCommand::List(command) => list_pins(client, writer, json_output, command).await,
        PinsCommand::Create(command) => create_pin(client, writer, json_output, command).await,
        PinsCommand::Delete(command) => delete_pin(client, writer, json_output, command).await,
    }
}

async fn list_pins(
    client: &CacheClient,
    writer: &mut impl Write,
    json_output: bool,
    command: ListPinsCommand,
) -> Result<()> {
    let project = command
        .project
        .as_deref()
        .map(parse_project_slug)
        .transpose()?;

    let pins = client
        .list_pins(project.as_ref())
        .await
        .context("listing pins")?;

    print_pins(writer, json_output, &pins)
}

async fn create_pin(
    client: &CacheClient,
    writer: &mut impl Write,
    json_output: bool,
    command: CreatePinCommand,
) -> Result<()> {
    let project = command
        .project
        .as_deref()
        .map(parse_project_slug)
        .transpose()?;

    client
        .create_pin(&command.name, project.as_ref(), &command.store_path)
        .await
        .with_context(|| format!("creating pin {}", command.name))?;

    if json_output {
        output::print_status_json(
            writer,
            "created",
            [
                ("name", serde_json::json!(command.name)),
                (
                    "project",
                    project
                        .as_ref()
                        .map(|project| serde_json::json!(project.as_str()))
                        .unwrap_or(serde_json::Value::Null),
                ),
                ("store_path", serde_json::json!(command.store_path)),
            ],
        )?;
    } else {
        writeln!(writer, "created pin {}", command.name)?;
    }

    Ok(())
}

async fn delete_pin(
    client: &CacheClient,
    writer: &mut impl Write,
    json_output: bool,
    command: DeletePinCommand,
) -> Result<()> {
    let project = command
        .project
        .as_deref()
        .map(parse_project_slug)
        .transpose()?;

    let deleted = client
        .delete_pin(&command.name, project.as_ref())
        .await
        .with_context(|| format!("deleting pin {}", command.name))?;

    if !deleted && !command.ignore_missing {
        anyhow::bail!("pin {} does not exist", command.name);
    }

    if json_output {
        output::print_status_json(
            writer,
            if deleted { "deleted" } else { "missing" },
            [
                ("name", serde_json::json!(command.name)),
                (
                    "project",
                    project
                        .as_ref()
                        .map(|project| serde_json::json!(project.as_str()))
                        .unwrap_or(serde_json::Value::Null),
                ),
            ],
        )?;
    } else if deleted {
        writeln!(writer, "deleted pin {}", command.name)?;
    } else {
        writeln!(writer, "pin {} was already absent", command.name)?;
    }

    Ok(())
}

fn parse_project_slug(slug: &str) -> Result<ProjectSlug> {
    ProjectSlug::parse(slug).map_err(|_| anyhow::anyhow!("invalid project slug {}", slug))
}

fn print_pins(writer: &mut impl Write, json_output: bool, pins: &[PinInfo]) -> Result<()> {
    if json_output {
        output::print_json(writer, pins)?;
    } else {
        for pin in pins {
            let project = pin.project.as_deref().unwrap_or("<global>");
            writeln!(writer, "{}\t{}\t{}", pin.name, project, pin.store_path)?;
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

    use cache_api::CreatePinRequest;
    use cache_test_utils::{EXAMPLE_PROJECT_SLUG, TestServer};

    use super::*;

    const PIN_NAME: &str = "release";
    const STORE_PATH: &str = "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-release";

    type Shared<T> = Arc<Mutex<T>>;
    type DeletedPins = Vec<(String, Option<String>)>;

    #[derive(Default, Clone)]
    struct TestState {
        auth_headers: Shared<Vec<String>>,
        create_requests: Shared<Vec<CreatePinRequest>>,
        deleted: Shared<DeletedPins>,
    }

    #[derive(Debug, Deserialize)]
    struct PinQuery {
        project: Option<String>,
    }

    async fn list_pins_handler(
        State(state): State<TestState>,
        headers: HeaderMap,
        Query(query): Query<PinQuery>,
    ) -> (StatusCode, Json<Vec<PinInfo>>) {
        record_auth_header(&state, &headers);
        assert_eq!(query.project.as_deref(), Some(EXAMPLE_PROJECT_SLUG));
        (
            StatusCode::OK,
            Json(vec![PinInfo {
                name: PIN_NAME.to_owned(),
                project: Some(EXAMPLE_PROJECT_SLUG.to_owned()),
                store_path: STORE_PATH.to_owned(),
                created_at: "2026-04-20T00:00:00Z".to_owned(),
                updated_at: "2026-04-20T00:00:00Z".to_owned(),
            }]),
        )
    }

    async fn create_pin_handler(
        State(state): State<TestState>,
        headers: HeaderMap,
        Path(name): Path<String>,
        Json(request): Json<CreatePinRequest>,
    ) -> StatusCode {
        record_auth_header(&state, &headers);
        assert_eq!(name, PIN_NAME);
        state.create_requests.lock().unwrap().push(request);
        StatusCode::NO_CONTENT
    }

    async fn delete_pin_handler(
        State(state): State<TestState>,
        headers: HeaderMap,
        Path(name): Path<String>,
        Query(query): Query<PinQuery>,
    ) -> StatusCode {
        record_auth_header(&state, &headers);
        state.deleted.lock().unwrap().push((name, query.project));
        StatusCode::NO_CONTENT
    }

    async fn missing_delete_pin_handler() -> StatusCode {
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

    fn client_for(server: &TestServer) -> CacheClient {
        CacheClient::new(&server.base_url, "secret-token").unwrap()
    }

    #[tokio::test]
    async fn list_pins_prints_human_output() {
        let state = TestState::default();
        let app = Router::new()
            .route("/api/pins", get(list_pins_handler))
            .with_state(state);
        let server = TestServer::spawn(app).await.unwrap();
        let client = client_for(&server);

        let mut output = Vec::new();

        handle(
            &client,
            &mut output,
            false,
            PinsCommand::List(ListPinsCommand {
                project: Some(EXAMPLE_PROJECT_SLUG.to_owned()),
            }),
        )
        .await
        .unwrap();

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains(PIN_NAME));
        assert!(output.contains(EXAMPLE_PROJECT_SLUG));
        assert!(output.contains(STORE_PATH));
    }

    #[tokio::test]
    async fn create_pin_sends_request() {
        let state = TestState::default();
        let app = Router::new()
            .route("/api/pins/{name}", post(create_pin_handler))
            .with_state(state.clone());

        let server = TestServer::spawn(app).await.unwrap();
        let client = client_for(&server);

        let mut output = Vec::new();

        handle(
            &client,
            &mut output,
            false,
            PinsCommand::Create(CreatePinCommand {
                name: PIN_NAME.to_owned(),
                store_path: STORE_PATH.to_owned(),
                project: Some(EXAMPLE_PROJECT_SLUG.to_owned()),
            }),
        )
        .await
        .unwrap();

        let requests = state.create_requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].project.as_deref(), Some(EXAMPLE_PROJECT_SLUG));
        assert_eq!(requests[0].store_path, STORE_PATH);

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("created pin"));
    }

    #[tokio::test]
    async fn delete_pin_sends_request() {
        let state = TestState::default();
        let app = Router::new()
            .route("/api/pins/{name}", delete(delete_pin_handler))
            .with_state(state.clone());

        let server = TestServer::spawn(app).await.unwrap();
        let client = client_for(&server);

        let mut output = Vec::new();

        handle(
            &client,
            &mut output,
            false,
            PinsCommand::Delete(DeletePinCommand {
                name: PIN_NAME.to_owned(),
                project: Some(EXAMPLE_PROJECT_SLUG.to_owned()),
                ignore_missing: false,
            }),
        )
        .await
        .unwrap();

        assert_eq!(
            state.deleted.lock().unwrap().as_slice(),
            &[(PIN_NAME.to_owned(), Some(EXAMPLE_PROJECT_SLUG.to_owned()))]
        );

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("deleted pin"));
    }

    #[tokio::test]
    async fn delete_pin_ignore_missing_succeeds_for_not_found() {
        let app = Router::new().route("/api/pins/{name}", delete(missing_delete_pin_handler));
        let server = TestServer::spawn(app).await.unwrap();
        let client = client_for(&server);

        let mut output = Vec::new();

        handle(
            &client,
            &mut output,
            false,
            PinsCommand::Delete(DeletePinCommand {
                name: PIN_NAME.to_owned(),
                project: Some(EXAMPLE_PROJECT_SLUG.to_owned()),
                ignore_missing: true,
            }),
        )
        .await
        .unwrap();

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("already absent"));
    }
}
