use std::io::Write;

use anyhow::{Context as _, Result, bail};

use cache_api::{
    DeleteProjectOidcIdentityRequest, ProjectOidcIdentityInfo, UpsertProjectOidcIdentityRequest,
};
use cache_client::CacheClient;
use cache_core::project::ProjectSlug;

use crate::cli::{
    AddProjectOidcCommand, CreateProjectCommand, ListProjectOidcCommand, ProjectOidcCommand,
    ProjectsCommand, RemoveProjectOidcCommand,
};
use crate::output;

pub async fn handle(
    client: &CacheClient,
    writer: &mut impl Write,
    json_output: bool,
    command: ProjectsCommand,
) -> Result<()> {
    match command {
        ProjectsCommand::List => list_projects(client, writer, json_output).await,
        ProjectsCommand::Create(command) => {
            create_project(client, writer, json_output, command).await
        }
        ProjectsCommand::Oidc(command) => handle_oidc(client, writer, json_output, command).await,
    }
}

async fn list_projects(
    client: &CacheClient,
    writer: &mut impl Write,
    json_output: bool,
) -> Result<()> {
    let projects = client.list_projects().await.context("listing projects")?;

    if json_output {
        output::print_json(writer, &projects)?;
    } else {
        for project in projects {
            writeln!(
                writer,
                "{}\t{}\tpublic={}",
                project.slug, project.display_name, project.public
            )?;
        }
    }

    Ok(())
}

async fn create_project(
    client: &CacheClient,
    writer: &mut impl Write,
    json_output: bool,
    command: CreateProjectCommand,
) -> Result<()> {
    let project = parse_project_slug(&command.slug)?;
    if command.if_not_exists && project_exists(client, &project).await? {
        if json_output {
            output::print_status_json(
                writer,
                "exists",
                [("project", serde_json::json!(project.as_str()))],
            )?;
        } else {
            writeln!(writer, "project {} already exists", project.as_str())?;
        }
        return Ok(());
    }
    let display_name = command
        .display_name
        .unwrap_or_else(|| project.as_str().to_owned());
    client
        .upsert_project(&project, &display_name, command.public)
        .await
        .with_context(|| format!("creating project {}", project.as_str()))?;
    if json_output {
        output::print_status_json(
            writer,
            "created",
            [("project", serde_json::json!(project.as_str()))],
        )?;
    } else {
        writeln!(writer, "created project {}", project.as_str())?;
    }
    Ok(())
}

async fn handle_oidc(
    client: &CacheClient,
    writer: &mut impl Write,
    json_output: bool,
    command: ProjectOidcCommand,
) -> Result<()> {
    match command {
        ProjectOidcCommand::List(command) => {
            list_oidc_identities(client, writer, json_output, command).await
        }
        ProjectOidcCommand::Add(command) => {
            add_oidc_identity(client, writer, json_output, command).await
        }
        ProjectOidcCommand::Remove(command) => {
            remove_oidc_identity(client, writer, json_output, command).await
        }
    }
}

async fn list_oidc_identities(
    client: &CacheClient,
    writer: &mut impl Write,
    json_output: bool,
    command: ListProjectOidcCommand,
) -> Result<()> {
    let project = parse_project_slug(&command.project)?;

    let identities = client
        .list_project_oidc_identities(&project)
        .await
        .with_context(|| format!("listing OIDC identities for project {}", project.as_str()))?;

    print_oidc_identities(writer, json_output, &identities)
}

async fn add_oidc_identity(
    client: &CacheClient,
    writer: &mut impl Write,
    json_output: bool,
    command: AddProjectOidcCommand,
) -> Result<()> {
    let project = parse_project_slug(&command.project)?;

    if command.provider.trim().is_empty() {
        bail!("--provider must not be empty");
    }

    if command.repository.trim().is_empty() {
        bail!("--repository must not be empty");
    }

    if command.if_not_exists {
        let identities = client
            .list_project_oidc_identities(&project)
            .await
            .with_context(|| format!("listing OIDC identities for project {}", project.as_str()))?;

        if identities.iter().any(|identity| {
            identity.provider == command.provider && identity.repository == command.repository
        }) {
            if json_output {
                output::print_status_json(
                    writer,
                    "exists",
                    [
                        ("project", serde_json::json!(project.as_str())),
                        ("provider", serde_json::json!(command.provider)),
                        ("repository", serde_json::json!(command.repository)),
                    ],
                )?;
            } else {
                writeln!(
                    writer,
                    "OIDC identity {}/{} already exists for project {}",
                    command.provider,
                    command.repository,
                    project.as_str()
                )?;
            }
            return Ok(());
        }
    }

    let request = UpsertProjectOidcIdentityRequest {
        provider: command.provider,
        repository: command.repository,
        ref_patterns: command.ref_patterns,
    };

    client
        .upsert_project_oidc_identity(&project, request.clone())
        .await
        .with_context(|| format!("adding OIDC identity for project {}", project.as_str()))?;

    if json_output {
        output::print_status_json(
            writer,
            "added",
            [
                ("project", serde_json::json!(project.as_str())),
                ("provider", serde_json::json!(request.provider)),
                ("repository", serde_json::json!(request.repository)),
                ("ref_patterns", serde_json::json!(request.ref_patterns)),
            ],
        )?;
    } else {
        writeln!(
            writer,
            "added OIDC identity {}/{} to project {}",
            request.provider,
            request.repository,
            project.as_str()
        )?;
    }

    Ok(())
}

async fn remove_oidc_identity(
    client: &CacheClient,
    writer: &mut impl Write,
    json_output: bool,
    command: RemoveProjectOidcCommand,
) -> Result<()> {
    let project = parse_project_slug(&command.project)?;

    let request = DeleteProjectOidcIdentityRequest {
        provider: command.provider,
        repository: command.repository,
    };

    let deleted = client
        .delete_project_oidc_identity(&project, request.clone())
        .await
        .with_context(|| format!("removing OIDC identity from project {}", project.as_str()))?;

    if !deleted && !command.ignore_missing {
        bail!(
            "OIDC identity {}/{} does not exist for project {}",
            request.provider,
            request.repository,
            project.as_str()
        );
    }

    if json_output {
        output::print_status_json(
            writer,
            if deleted { "removed" } else { "missing" },
            [
                ("project", serde_json::json!(project.as_str())),
                ("provider", serde_json::json!(request.provider)),
                ("repository", serde_json::json!(request.repository)),
            ],
        )?;
    } else if deleted {
        writeln!(
            writer,
            "removed OIDC identity {}/{} from project {}",
            request.provider,
            request.repository,
            project.as_str()
        )?;
    } else {
        writeln!(
            writer,
            "OIDC identity {}/{} was already absent from project {}",
            request.provider,
            request.repository,
            project.as_str()
        )?;
    }

    Ok(())
}

async fn project_exists(client: &CacheClient, project: &ProjectSlug) -> Result<bool> {
    let projects = client.list_projects().await.context("listing projects")?;

    Ok(projects.iter().any(|item| item.slug == project.as_str()))
}

fn parse_project_slug(slug: &str) -> Result<ProjectSlug> {
    ProjectSlug::parse(slug).map_err(|_| anyhow::anyhow!("invalid project slug {}", slug))
}

fn print_oidc_identities(
    writer: &mut impl Write,
    json_output: bool,
    identities: &[ProjectOidcIdentityInfo],
) -> Result<()> {
    if json_output {
        output::print_json(writer, identities)?;
    } else {
        for identity in identities {
            let refs = if identity.ref_patterns.is_empty() {
                "*".to_owned()
            } else {
                identity.ref_patterns.join(",")
            };

            writeln!(
                writer,
                "{}\t{}\trefs={}",
                identity.provider, identity.repository, refs
            )?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::extract::{Path, State};
    use axum::http::{HeaderMap, StatusCode, header};
    use axum::routing::{delete, get, post};
    use axum::{Json, Router};

    use cache_api::{
        ProjectInfo, ProjectOidcIdentityInfo, UpsertProjectOidcIdentityRequest,
        UpsertProjectRequest,
    };
    use cache_test_utils::{EXAMPLE_PROJECT_NAME, EXAMPLE_PROJECT_SLUG, TestServer};

    use super::*;

    #[derive(Default, Clone)]
    struct TestState {
        auth_headers: Arc<Mutex<Vec<String>>>,
        created_projects: Arc<Mutex<Vec<UpsertProjectRequest>>>,
        oidc_requests: Arc<Mutex<Vec<UpsertProjectOidcIdentityRequest>>>,
        deleted_oidc_requests: Arc<Mutex<Vec<DeleteProjectOidcIdentityRequest>>>,
    }

    async fn list_projects_handler() -> Json<Vec<ProjectInfo>> {
        Json(vec![ProjectInfo {
            slug: EXAMPLE_PROJECT_SLUG.to_owned(),
            display_name: EXAMPLE_PROJECT_NAME.to_owned(),
            public: true,
            created_at: "2026-04-20T00:00:00Z".to_owned(),
        }])
    }

    async fn create_project_handler(
        State(state): State<TestState>,
        headers: HeaderMap,
        Json(request): Json<UpsertProjectRequest>,
    ) -> StatusCode {
        record_auth_header(&state, &headers);
        state.created_projects.lock().unwrap().push(request);
        StatusCode::NO_CONTENT
    }

    async fn list_oidc_handler(Path(project): Path<String>) -> Json<Vec<ProjectOidcIdentityInfo>> {
        assert_eq!(project, EXAMPLE_PROJECT_SLUG);

        Json(vec![ProjectOidcIdentityInfo {
            provider: "github".to_owned(),
            repository: "owner/repo".to_owned(),
            ref_patterns: vec!["refs/heads/main".to_owned()],
        }])
    }

    async fn add_oidc_handler(
        State(state): State<TestState>,
        headers: HeaderMap,
        Path(project): Path<String>,
        Json(request): Json<UpsertProjectOidcIdentityRequest>,
    ) -> StatusCode {
        assert_eq!(project, EXAMPLE_PROJECT_SLUG);
        record_auth_header(&state, &headers);
        state.oidc_requests.lock().unwrap().push(request);
        StatusCode::NO_CONTENT
    }

    async fn delete_oidc_handler(
        State(state): State<TestState>,
        headers: HeaderMap,
        Path(project): Path<String>,
        Json(request): Json<DeleteProjectOidcIdentityRequest>,
    ) -> StatusCode {
        assert_eq!(project, EXAMPLE_PROJECT_SLUG);
        record_auth_header(&state, &headers);
        state.deleted_oidc_requests.lock().unwrap().push(request);
        StatusCode::NO_CONTENT
    }

    async fn missing_delete_oidc_handler(
        Json(_request): Json<DeleteProjectOidcIdentityRequest>,
    ) -> StatusCode {
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
    async fn list_projects_prints_human_output() {
        let app = Router::new().route("/api/projects", get(list_projects_handler));
        let server = TestServer::spawn(app).await.unwrap();
        let client = client_for(&server);

        let mut output = Vec::new();

        handle(&client, &mut output, false, ProjectsCommand::List)
            .await
            .unwrap();

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains(EXAMPLE_PROJECT_SLUG));
        assert!(output.contains(EXAMPLE_PROJECT_NAME));
        assert!(output.contains("public=true"));
    }

    #[tokio::test]
    async fn list_projects_prints_json_output() {
        let app = Router::new().route("/api/projects", get(list_projects_handler));
        let server = TestServer::spawn(app).await.unwrap();
        let client = client_for(&server);

        let mut output = Vec::new();

        handle(&client, &mut output, true, ProjectsCommand::List)
            .await
            .unwrap();

        let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(value[0]["slug"], EXAMPLE_PROJECT_SLUG);
        assert_eq!(value[0]["display_name"], EXAMPLE_PROJECT_NAME);
        assert_eq!(value[0]["public"], true);
    }

    #[tokio::test]
    async fn create_project_sends_request_and_auth_header() {
        let state = TestState::default();
        let app = Router::new()
            .route("/api/projects", post(create_project_handler))
            .with_state(state.clone());

        let server = TestServer::spawn(app).await.unwrap();
        let client = client_for(&server);

        let mut output = Vec::new();

        handle(
            &client,
            &mut output,
            false,
            ProjectsCommand::Create(CreateProjectCommand {
                slug: EXAMPLE_PROJECT_SLUG.to_owned(),
                display_name: Some(EXAMPLE_PROJECT_NAME.to_owned()),
                public: true,
                if_not_exists: false,
            }),
        )
        .await
        .unwrap();

        assert_eq!(
            state.auth_headers.lock().unwrap().as_slice(),
            &["Bearer secret-token".to_owned()]
        );

        let created = state.created_projects.lock().unwrap();
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].slug, EXAMPLE_PROJECT_SLUG);
        assert_eq!(created[0].display_name, EXAMPLE_PROJECT_NAME);
        assert!(created[0].public);

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("created project"));
    }

    #[tokio::test]
    async fn create_project_if_not_exists_skips_existing_project() {
        let state = TestState::default();
        let app = Router::new()
            .route("/api/projects", get(list_projects_handler))
            .route("/api/projects", post(create_project_handler))
            .with_state(state.clone());

        let server = TestServer::spawn(app).await.unwrap();
        let client = client_for(&server);

        let mut output = Vec::new();

        handle(
            &client,
            &mut output,
            false,
            ProjectsCommand::Create(CreateProjectCommand {
                slug: EXAMPLE_PROJECT_SLUG.to_owned(),
                display_name: None,
                public: true,
                if_not_exists: true,
            }),
        )
        .await
        .unwrap();

        assert!(state.created_projects.lock().unwrap().is_empty());

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("already exists"));
    }
    #[tokio::test]
    async fn add_oidc_identity_sends_request() {
        let state = TestState::default();
        let app = Router::new()
            .route(
                "/api/projects/{project}/oidc-identities",
                post(add_oidc_handler),
            )
            .with_state(state.clone());

        let server = TestServer::spawn(app).await.unwrap();
        let client = client_for(&server);

        let mut output = Vec::new();

        handle(
            &client,
            &mut output,
            false,
            ProjectsCommand::Oidc(ProjectOidcCommand::Add(AddProjectOidcCommand {
                project: EXAMPLE_PROJECT_SLUG.to_owned(),
                provider: "github".to_owned(),
                repository: "owner/repo".to_owned(),
                ref_patterns: vec!["refs/heads/main".to_owned()],
                if_not_exists: false,
            })),
        )
        .await
        .unwrap();

        assert_eq!(
            state.auth_headers.lock().unwrap().as_slice(),
            &["Bearer secret-token".to_owned()]
        );

        let requests = state.oidc_requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].provider, "github");
        assert_eq!(requests[0].repository, "owner/repo");
        assert_eq!(requests[0].ref_patterns, vec!["refs/heads/main".to_owned()]);

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("added OIDC identity"));
    }

    #[tokio::test]
    async fn add_oidc_identity_if_not_exists_skips_existing_identity() {
        let state = TestState::default();
        let app = Router::new()
            .route(
                "/api/projects/{project}/oidc-identities",
                get(list_oidc_handler),
            )
            .route(
                "/api/projects/{project}/oidc-identities",
                post(add_oidc_handler),
            )
            .with_state(state.clone());

        let server = TestServer::spawn(app).await.unwrap();
        let client = client_for(&server);

        let mut output = Vec::new();

        handle(
            &client,
            &mut output,
            false,
            ProjectsCommand::Oidc(ProjectOidcCommand::Add(AddProjectOidcCommand {
                project: EXAMPLE_PROJECT_SLUG.to_owned(),
                provider: "github".to_owned(),
                repository: "owner/repo".to_owned(),
                ref_patterns: vec!["refs/heads/main".to_owned()],
                if_not_exists: true,
            })),
        )
        .await
        .unwrap();

        assert!(state.oidc_requests.lock().unwrap().is_empty());

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("already exists"));
    }

    #[tokio::test]
    async fn list_oidc_identities_prints_human_output() {
        let app = Router::new().route(
            "/api/projects/{project}/oidc-identities",
            get(list_oidc_handler),
        );

        let server = TestServer::spawn(app).await.unwrap();
        let client = client_for(&server);

        let mut output = Vec::new();

        handle(
            &client,
            &mut output,
            false,
            ProjectsCommand::Oidc(ProjectOidcCommand::List(ListProjectOidcCommand {
                project: EXAMPLE_PROJECT_SLUG.to_owned(),
            })),
        )
        .await
        .unwrap();

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("github"));
        assert!(output.contains("owner/repo"));
        assert!(output.contains("refs=refs/heads/main"));
    }

    #[tokio::test]
    async fn remove_oidc_identity_sends_request() {
        let state = TestState::default();
        let app = Router::new()
            .route(
                "/api/projects/{project}/oidc-identities",
                delete(delete_oidc_handler),
            )
            .with_state(state.clone());

        let server = TestServer::spawn(app).await.unwrap();
        let client = client_for(&server);

        let mut output = Vec::new();

        handle(
            &client,
            &mut output,
            false,
            ProjectsCommand::Oidc(ProjectOidcCommand::Remove(RemoveProjectOidcCommand {
                project: EXAMPLE_PROJECT_SLUG.to_owned(),
                provider: "github".to_owned(),
                repository: "owner/repo".to_owned(),
                ignore_missing: false,
            })),
        )
        .await
        .unwrap();

        assert_eq!(
            state.auth_headers.lock().unwrap().as_slice(),
            &["Bearer secret-token".to_owned()]
        );

        let requests = state.deleted_oidc_requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].provider, "github");
        assert_eq!(requests[0].repository, "owner/repo");

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("removed OIDC identity"));
    }

    #[tokio::test]
    async fn remove_oidc_identity_ignore_missing_succeeds_for_not_found() {
        let app = Router::new().route(
            "/api/projects/{project}/oidc-identities",
            delete(missing_delete_oidc_handler),
        );

        let server = TestServer::spawn(app).await.unwrap();
        let client = client_for(&server);

        let mut output = Vec::new();

        handle(
            &client,
            &mut output,
            false,
            ProjectsCommand::Oidc(ProjectOidcCommand::Remove(RemoveProjectOidcCommand {
                project: EXAMPLE_PROJECT_SLUG.to_owned(),
                provider: "github".to_owned(),
                repository: "owner/repo".to_owned(),
                ignore_missing: true,
            })),
        )
        .await
        .unwrap();

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("already absent"));
    }
}
