use std::io::Write;

use anyhow::{Context as _, Result, bail};

use cache_api::{
    DeleteProjectOidcIdentityRequest, ProjectOidcIdentityInfo, ProjectRetentionPolicyInfo,
    ProjectRetentionRuleInfo, UpsertProjectOidcIdentityRequest,
    UpsertProjectRetentionPolicyRequest,
};
use cache_client::CacheClient;
use cache_core::project::ProjectSlug;

use crate::cli::{
    AddProjectOidcCommand, CreateProjectCommand, GetProjectRetentionCommand,
    ListProjectOidcCommand, ProjectKeyImportCommand, ProjectKeyProjectCommand,
    ProjectKeyRotateCommand, ProjectKeysCommand, ProjectOidcCommand, ProjectRetentionCommand,
    ProjectsCommand, RemoveProjectOidcCommand, ResetProjectRetentionCommand, RetentionProfile,
    SetProjectRetentionCommand,
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
        ProjectsCommand::Retention(command) => {
            handle_retention(client, writer, json_output, command).await
        }
        ProjectsCommand::Keys(command) => handle_keys(client, writer, json_output, command).await,
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

async fn handle_retention(
    client: &CacheClient,
    writer: &mut impl Write,
    json_output: bool,
    command: ProjectRetentionCommand,
) -> Result<()> {
    match command {
        ProjectRetentionCommand::Get(command) => {
            get_retention_policy(client, writer, json_output, command).await
        }
        ProjectRetentionCommand::Set(command) => {
            set_retention_policy(client, writer, json_output, command).await
        }
        ProjectRetentionCommand::Reset(command) => {
            reset_retention_policy(client, writer, json_output, command).await
        }
    }
}

async fn get_retention_policy(
    client: &CacheClient,
    writer: &mut impl Write,
    json_output: bool,
    command: GetProjectRetentionCommand,
) -> Result<()> {
    let project = parse_project_slug(&command.project)?;

    let policy = client
        .get_project_retention_policy(&project)
        .await
        .with_context(|| format!("loading retention policy for project {}", project.as_str()))?;

    print_retention_policy(writer, json_output, policy)
}

async fn set_retention_policy(
    client: &CacheClient,
    writer: &mut impl Write,
    json_output: bool,
    command: SetProjectRetentionCommand,
) -> Result<()> {
    let project = parse_project_slug(&command.project)?;
    let (profile_name, request) = retention_request_from_command(&command)?;

    client
        .upsert_project_retention_policy(&project, request)
        .await
        .with_context(|| format!("setting retention policy for project {}", project.as_str()))?;

    if json_output {
        output::print_status_json(
            writer,
            "updated",
            [
                ("project", serde_json::json!(project.as_str())),
                ("profile", serde_json::json!(profile_name)),
            ],
        )?;
    } else {
        writeln!(writer, "updated retention policy for {}", project.as_str())?;
        writeln!(writer, "profile={profile_name}")?;
    }

    Ok(())
}

async fn reset_retention_policy(
    client: &CacheClient,
    writer: &mut impl Write,
    json_output: bool,
    command: ResetProjectRetentionCommand,
) -> Result<()> {
    let project = parse_project_slug(&command.project)?;

    let deleted = client
        .delete_project_retention_policy(&project)
        .await
        .with_context(|| {
            format!(
                "resetting retention policy for project {}",
                project.as_str()
            )
        })?;

    if !deleted && !command.ignore_missing {
        bail!(
            "custom retention policy does not exist for project {}",
            project.as_str()
        );
    }

    if json_output {
        output::print_status_json(
            writer,
            if deleted { "reset" } else { "missing" },
            [("project", serde_json::json!(project.as_str()))],
        )?;
    } else if deleted {
        writeln!(writer, "reset retention policy for {}", project.as_str())?;
    } else {
        writeln!(
            writer,
            "retention policy for {} was already default",
            project.as_str()
        )?;
    }

    Ok(())
}

fn retention_request_from_command(
    command: &SetProjectRetentionCommand,
) -> Result<(&'static str, UpsertProjectRetentionPolicyRequest)> {
    if command.profile.is_some() && !command.rules.is_empty() {
        bail!("--profile and --rule cannot be used together");
    }

    match command.profile {
        Some(profile) => retention_request_from_profile(command, profile),
        None => retention_request_from_custom_rules(command),
    }
}

fn retention_request_from_profile(
    command: &SetProjectRetentionCommand,
    profile: RetentionProfile,
) -> Result<(&'static str, UpsertProjectRetentionPolicyRequest)> {
    let mut request = match profile {
        RetentionProfile::Aggressive => UpsertProjectRetentionPolicyRequest {
            keep_latest_builds_per_ref: 1,
            object_delete_grace_seconds: 24 * 60 * 60,
            rules: vec![
                rule(10, "refs/heads/main", None, Some(1)),
                rule(20, "refs/heads/master", None, Some(1)),
                rule(30, "refs/heads/trunk", None, Some(1)),
                rule(40, "refs/tags/*", None, Some(1)),
                rule(50, "refs/heads/release/*", days(90), Some(1)),
                rule(60, "refs/pull/*", days(7), Some(1)),
                rule(70, "refs/merge-requests/*", days(7), Some(1)),
                rule(80, "refs/heads/*", days(14), Some(1)),
                rule(90, "*", days(14), Some(1)),
            ],
        },
        RetentionProfile::Balanced => UpsertProjectRetentionPolicyRequest {
            keep_latest_builds_per_ref: 2,
            object_delete_grace_seconds: 24 * 60 * 60,
            rules: vec![
                rule(10, "refs/heads/main", None, Some(2)),
                rule(20, "refs/heads/master", None, Some(2)),
                rule(30, "refs/heads/trunk", None, Some(2)),
                rule(40, "refs/tags/*", None, Some(1)),
                rule(50, "refs/heads/release/*", days(180), Some(2)),
                rule(60, "refs/pull/*", days(14), Some(1)),
                rule(70, "refs/merge-requests/*", days(14), Some(1)),
                rule(80, "refs/heads/*", days(60), Some(2)),
                rule(90, "*", days(30), Some(1)),
            ],
        },
        RetentionProfile::Conservative => UpsertProjectRetentionPolicyRequest {
            keep_latest_builds_per_ref: 5,
            object_delete_grace_seconds: 24 * 60 * 60,
            rules: vec![
                rule(10, "refs/heads/main", None, Some(5)),
                rule(20, "refs/heads/master", None, Some(5)),
                rule(30, "refs/heads/trunk", None, Some(5)),
                rule(40, "refs/tags/*", None, Some(2)),
                rule(50, "refs/heads/release/*", None, Some(3)),
                rule(60, "refs/pull/*", days(30), Some(2)),
                rule(70, "refs/merge-requests/*", days(30), Some(2)),
                rule(80, "refs/heads/*", days(180), Some(3)),
                rule(90, "*", days(90), Some(2)),
            ],
        },
    };

    if let Some(keep_builds) = command.keep_builds {
        if keep_builds == 0 {
            bail!("--keep-builds must be greater than zero");
        }
        request.keep_latest_builds_per_ref = keep_builds;
    }

    if let Some(grace) = command.object_delete_grace.as_deref() {
        request.object_delete_grace_seconds = parse_duration_seconds(grace)?
            .ok_or_else(|| anyhow::anyhow!("--object-delete-grace cannot be forever"))?;
    }

    let profile_name = match profile {
        RetentionProfile::Aggressive => "aggressive",
        RetentionProfile::Balanced => "balanced",
        RetentionProfile::Conservative => "conservative",
    };

    Ok((profile_name, request))
}

fn retention_request_from_custom_rules(
    command: &SetProjectRetentionCommand,
) -> Result<(&'static str, UpsertProjectRetentionPolicyRequest)> {
    if command.rules.is_empty() {
        bail!("retention set requires either --profile or at least one --rule");
    }

    let keep_latest_builds_per_ref = command
        .keep_builds
        .ok_or_else(|| anyhow::anyhow!("custom retention requires --keep-builds"))?;
    if keep_latest_builds_per_ref == 0 {
        bail!("--keep-builds must be greater than zero");
    }

    let object_delete_grace_seconds = command
        .object_delete_grace
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("custom retention requires --object-delete-grace"))
        .and_then(|value| {
            parse_duration_seconds(value)?
                .ok_or_else(|| anyhow::anyhow!("--object-delete-grace cannot be forever"))
        })?;

    let rules = command
        .rules
        .iter()
        .map(|value| parse_retention_rule(value))
        .collect::<Result<Vec<_>>>()?;

    Ok((
        "custom",
        UpsertProjectRetentionPolicyRequest {
            keep_latest_builds_per_ref,
            object_delete_grace_seconds,
            rules,
        },
    ))
}

fn parse_retention_rule(value: &str) -> Result<ProjectRetentionRuleInfo> {
    let mut parts = value.splitn(4, ':').collect::<Vec<_>>();
    if parts.len() != 4 {
        bail!("invalid retention rule {value:?}; expected priority:ref_pattern:ttl:keep_builds");
    }

    let priority = parts
        .remove(0)
        .parse::<u32>()
        .with_context(|| format!("invalid priority in retention rule {value:?}"))?;
    if priority == 0 {
        bail!("retention rule priority must be greater than zero");
    }

    let ref_pattern = parts.remove(0).trim().to_owned();
    if ref_pattern.is_empty() {
        bail!("retention rule ref_pattern must not be empty");
    }

    let ttl_seconds = parse_duration_seconds(parts.remove(0))?;
    let keep_builds = parse_keep_builds(parts.remove(0))?;

    Ok(ProjectRetentionRuleInfo {
        priority,
        ref_pattern,
        ttl_seconds,
        keep_builds,
    })
}

fn parse_keep_builds(value: &str) -> Result<Option<u32>> {
    let value = value.trim();

    if value == "default" {
        return Ok(None);
    }

    let keep_builds = value
        .parse::<u32>()
        .with_context(|| format!("invalid keep_builds value {value:?}"))?;

    if keep_builds == 0 {
        bail!("keep_builds must be greater than zero");
    }

    Ok(Some(keep_builds))
}

fn parse_duration_seconds(value: &str) -> Result<Option<u64>> {
    let value = value.trim();

    if value == "forever" {
        return Ok(None);
    }

    let Some((number, multiplier)) = value
        .strip_suffix('d')
        .map(|number| (number, 24 * 60 * 60))
        .or_else(|| value.strip_suffix('h').map(|number| (number, 60 * 60)))
        .or_else(|| value.strip_suffix('m').map(|number| (number, 60)))
        .or_else(|| value.strip_suffix('s').map(|number| (number, 1)))
    else {
        bail!("invalid duration {value:?}; expected forever, 14d, 24h, 30m, or 60s");
    };

    let amount = number
        .parse::<u64>()
        .with_context(|| format!("invalid duration amount in {value:?}"))?;

    Ok(Some(amount * multiplier))
}

fn days(days: u64) -> Option<u64> {
    Some(days * 24 * 60 * 60)
}

fn rule(
    priority: u32,
    ref_pattern: &str,
    ttl_seconds: Option<u64>,
    keep_builds: Option<u32>,
) -> ProjectRetentionRuleInfo {
    ProjectRetentionRuleInfo {
        priority,
        ref_pattern: ref_pattern.to_owned(),
        ttl_seconds,
        keep_builds,
    }
}

fn print_retention_policy(
    writer: &mut impl Write,
    json_output: bool,
    policy: ProjectRetentionPolicyInfo,
) -> Result<()> {
    if json_output {
        output::print_json(writer, &policy)?;
    } else {
        writeln!(
            writer,
            "{}\tinherited_default={}\tkeep_latest_builds_per_ref={}\tobject_delete_grace_seconds={}",
            policy.project,
            policy.inherited_default,
            policy.keep_latest_builds_per_ref,
            policy.object_delete_grace_seconds,
        )?;

        for rule in policy.rules {
            let ttl = rule
                .ttl_seconds
                .map(format_duration_seconds)
                .unwrap_or_else(|| "forever".to_owned());
            let keep = rule
                .keep_builds
                .map(|value| value.to_string())
                .unwrap_or_else(|| "default".to_owned());
            writeln!(
                writer,
                "rule\tpriority={}\tref={}\tttl={}\tkeep_builds={}",
                rule.priority, rule.ref_pattern, ttl, keep
            )?;
        }
    }

    Ok(())
}

fn format_duration_seconds(seconds: u64) -> String {
    const DAY: u64 = 24 * 60 * 60;
    const HOUR: u64 = 60 * 60;
    const MINUTE: u64 = 60;

    if seconds.is_multiple_of(DAY) {
        format!("{}d", seconds / DAY)
    } else if seconds.is_multiple_of(HOUR) {
        format!("{}h", seconds / HOUR)
    } else if seconds.is_multiple_of(MINUTE) {
        format!("{}m", seconds / MINUTE)
    } else {
        format!("{seconds}s")
    }
}

async fn handle_keys(
    client: &CacheClient,
    writer: &mut impl Write,
    json_output: bool,
    command: ProjectKeysCommand,
) -> Result<()> {
    match command {
        ProjectKeysCommand::Get(command) => {
            get_project_key(client, writer, json_output, command).await
        }
        ProjectKeysCommand::Rotate(command) => {
            rotate_project_key(client, writer, json_output, command).await
        }
        ProjectKeysCommand::Import(command) => {
            import_project_key(client, writer, json_output, command).await
        }
    }
}

async fn get_project_key(
    client: &CacheClient,
    writer: &mut impl Write,
    json_output: bool,
    command: ProjectKeyProjectCommand,
) -> Result<()> {
    let project = parse_project_slug(&command.project)?;

    let response = client
        .get_project_signing_key(&project)
        .await
        .with_context(|| format!("getting signing key for project {}", project.as_str()))?;

    if json_output {
        output::print_json(writer, &response)?;
    } else if let Some(public_key) = response.public_key {
        writeln!(writer, "{public_key}")?;
    } else {
        writeln!(writer, "project {} has no signing key", project.as_str())?;
    }

    Ok(())
}

async fn rotate_project_key(
    client: &CacheClient,
    writer: &mut impl Write,
    json_output: bool,
    command: ProjectKeyRotateCommand,
) -> Result<()> {
    let project = parse_project_slug(&command.project)?;

    let response = client
        .generate_project_signing_key(&project, command.name)
        .await
        .with_context(|| format!("rotating signing key for project {}", project.as_str()))?;

    if json_output {
        output::print_json(writer, &response)?;
    } else {
        writeln!(
            writer,
            "rotated signing key for project {}",
            project.as_str()
        )?;
        writeln!(writer, "public_key={}", response.public_key)?;
    }

    Ok(())
}

async fn import_project_key(
    client: &CacheClient,
    writer: &mut impl Write,
    json_output: bool,
    command: ProjectKeyImportCommand,
) -> Result<()> {
    let project = parse_project_slug(&command.project)?;
    let signing_key = std::fs::read_to_string(&command.file)
        .with_context(|| format!("reading {}", command.file.display()))?;

    let response = client
        .import_project_signing_key(&project, command.name, signing_key)
        .await
        .with_context(|| format!("importing signing key for project {}", project.as_str()))?;

    if json_output {
        output::print_json(writer, &response)?;
    } else {
        writeln!(
            writer,
            "imported signing key for project {}",
            project.as_str()
        )?;
        writeln!(writer, "public_key={}", response.public_key)?;
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
            storage_id: "main".to_owned(),
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

    #[test]
    fn parse_duration_accepts_forever_and_units() {
        assert_eq!(parse_duration_seconds("forever").unwrap(), None);
        assert_eq!(
            parse_duration_seconds("14d").unwrap(),
            Some(14 * 24 * 60 * 60)
        );
        assert_eq!(parse_duration_seconds("24h").unwrap(), Some(24 * 60 * 60));
        assert_eq!(parse_duration_seconds("30m").unwrap(), Some(30 * 60));
        assert_eq!(parse_duration_seconds("60s").unwrap(), Some(60));
    }

    #[test]
    fn parse_retention_rule_accepts_default_keep_builds() {
        let rule = parse_retention_rule("40:refs/heads/*:60d:default").unwrap();

        assert_eq!(rule.priority, 40);
        assert_eq!(rule.ref_pattern, "refs/heads/*");
        assert_eq!(rule.ttl_seconds, Some(60 * 24 * 60 * 60));
        assert_eq!(rule.keep_builds, None);
    }

    #[test]
    fn balanced_profile_matches_expected_defaults() {
        let command = SetProjectRetentionCommand {
            project: EXAMPLE_PROJECT_SLUG.to_owned(),
            profile: Some(RetentionProfile::Balanced),
            keep_builds: None,
            object_delete_grace: None,
            rules: Vec::new(),
        };

        let (profile, request) = retention_request_from_command(&command).unwrap();

        assert_eq!(profile, "balanced");
        assert_eq!(request.keep_latest_builds_per_ref, 2);
        assert_eq!(request.object_delete_grace_seconds, 24 * 60 * 60);
        assert!(
            request
                .rules
                .iter()
                .any(|rule| rule.ref_pattern == "refs/pull/*"
                    && rule.ttl_seconds == Some(14 * 24 * 60 * 60)
                    && rule.keep_builds == Some(1))
        );
    }

    #[test]
    fn custom_rules_require_keep_builds_and_grace() {
        let command = SetProjectRetentionCommand {
            project: EXAMPLE_PROJECT_SLUG.to_owned(),
            profile: None,
            keep_builds: None,
            object_delete_grace: Some("24h".to_owned()),
            rules: vec!["10:*:30d:1".to_owned()],
        };

        assert!(retention_request_from_command(&command).is_err());
    }

    #[test]
    fn profile_and_rules_cannot_be_mixed() {
        let command = SetProjectRetentionCommand {
            project: EXAMPLE_PROJECT_SLUG.to_owned(),
            profile: Some(RetentionProfile::Balanced),
            keep_builds: None,
            object_delete_grace: None,
            rules: vec!["10:*:30d:1".to_owned()],
        };

        assert!(retention_request_from_command(&command).is_err());
    }
}
