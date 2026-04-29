use std::io::Write;

use anyhow::{Context as _, Result};

use depot_api::{RunGcRequest, RunGcResponse};
use depot_client::CacheClient;

use crate::cli::{GcCommand, RunGcCommand};
use crate::output;

pub async fn handle(
    client: &CacheClient,
    writer: &mut impl Write,
    json_output: bool,
    command: GcCommand,
) -> Result<()> {
    match command {
        GcCommand::Run(command) => run_gc(client, writer, json_output, command).await,
    }
}

async fn run_gc(
    client: &CacheClient,
    writer: &mut impl Write,
    json_output: bool,
    command: RunGcCommand,
) -> Result<()> {
    let response = client
        .run_gc(RunGcRequest {
            dry_run: command.dry_run,
            grace_period_seconds: command.grace_period_seconds,
        })
        .await
        .context("running GC")?;

    print_gc_response(writer, json_output, command.dry_run, &response)
}

fn print_gc_response(
    writer: &mut impl Write,
    json_output: bool,
    dry_run: bool,
    response: &RunGcResponse,
) -> Result<()> {
    if json_output {
        output::print_json(writer, response)?;
    } else {
        let action = if dry_run { "would delete" } else { "deleted" };
        writeln!(writer, "{action} {} objects", response.deleted_count)?;

        for object_path in &response.deleted_objects {
            writeln!(writer, "{object_path}")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode, header};
    use axum::routing::post;
    use axum::{Json, Router};

    use depot_test_utils::TestServer;

    use super::*;

    #[derive(Default, Clone)]
    struct TestState {
        auth_headers: Arc<Mutex<Vec<String>>>,
        requests: Arc<Mutex<Vec<RunGcRequest>>>,
    }

    async fn run_gc_handler(
        State(state): State<TestState>,
        headers: HeaderMap,
        Json(request): Json<RunGcRequest>,
    ) -> (StatusCode, Json<RunGcResponse>) {
        state.auth_headers.lock().unwrap().push(
            headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned(),
        );
        state.requests.lock().unwrap().push(request);

        (
            StatusCode::OK,
            Json(RunGcResponse {
                deleted_count: 2,
                deleted_objects: vec![
                    "nar/stale-one.nar.zst".to_owned(),
                    "nar/stale-two.nar.zst".to_owned(),
                ],
            }),
        )
    }

    fn client_for(server: &TestServer) -> CacheClient {
        CacheClient::new(&server.base_url, "secret-token").unwrap()
    }

    #[tokio::test]
    async fn gc_run_sends_request_and_prints_human_output() {
        let state = TestState::default();
        let app = Router::new()
            .route("/api/gc", post(run_gc_handler))
            .with_state(state.clone());

        let server = TestServer::spawn(app).await.unwrap();
        let client = client_for(&server);

        let mut output = Vec::new();

        handle(
            &client,
            &mut output,
            false,
            GcCommand::Run(RunGcCommand {
                dry_run: true,
                grace_period_seconds: Some(0),
            }),
        )
        .await
        .unwrap();

        assert_eq!(
            state.auth_headers.lock().unwrap().as_slice(),
            &["Bearer secret-token".to_owned()]
        );

        let requests = state.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].dry_run);
        assert_eq!(requests[0].grace_period_seconds, Some(0));

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("would delete 2 objects"));
        assert!(output.contains("nar/stale-one.nar.zst"));
        assert!(output.contains("nar/stale-two.nar.zst"));
    }

    #[tokio::test]
    async fn gc_run_prints_json_output() {
        let state = TestState::default();
        let app = Router::new()
            .route("/api/gc", post(run_gc_handler))
            .with_state(state);

        let server = TestServer::spawn(app).await.unwrap();
        let client = client_for(&server);

        let mut output = Vec::new();

        handle(
            &client,
            &mut output,
            true,
            GcCommand::Run(RunGcCommand {
                dry_run: false,
                grace_period_seconds: None,
            }),
        )
        .await
        .unwrap();

        let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(value["deleted_count"], 2);
        assert_eq!(value["deleted_objects"][0], "nar/stale-one.nar.zst");
    }
}
