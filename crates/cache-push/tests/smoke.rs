use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use reqwest::StatusCode;
use tokio::fs;
use tokio::process::Command;

use cache_app::{AppMode, AppParts, build_app_with_authorizer, build_app_with_parts};
use cache_auth::{
    ChainAuthorizer, OidcAuthorizer, OidcConfig, OidcProviderConfig, ReqwestOidcHttpClient,
    StaticTokenAuthorizer,
};
use cache_core::nix::parse_path_info_json;
use cache_store::upstream::ReqwestUpstreamCacheClient;
use cache_test_utils::{
    EXAMPLE_PROJECT_NAME, RecordedOidcTokenRequest, TestDatabase, TestGitHubActionsOidcServer,
    TestServer, example_project, filesystem_storage_in, test_signing_key,
};

const WRITE_TOKEN: &str = "secret-token";
const GITHUB_OIDC_REQUEST_TOKEN: &str = "github-actions-request-token";

async fn command_available(command: &str) -> bool {
    match Command::new(command).arg("--version").output().await {
        Ok(output) => output.status.success(),
        Err(_) => false,
    }
}

async fn skip_unless_nix_available() -> Result<()> {
    if !command_available("nix").await || !command_available("nix-store").await {
        eprintln!("skipping cache-push smoke test because nix/nix-store is unavailable");
        return Err(anyhow::anyhow!("skip"));
    }

    Ok(())
}

async fn add_fixed_store_path(path: &std::path::Path) -> Result<String> {
    let output = Command::new("nix-store")
        .args(["--add-fixed", "--recursive", "sha256"])
        .arg(path)
        .output()
        .await
        .context("running nix-store --add-fixed")?;

    if !output.status.success() {
        bail!(
            "nix-store --add-fixed failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

async fn path_info_for(store_path: &str) -> Result<cache_core::nix::PathInfo> {
    let output = Command::new("nix")
        .args([
            "--extra-experimental-features",
            "nix-command",
            "path-info",
            "--json",
            "--",
        ])
        .arg(store_path)
        .output()
        .await
        .with_context(|| format!("running nix path-info for {}", store_path))?;

    if !output.status.success() {
        bail!(
            "nix path-info failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let mut infos =
        parse_path_info_json(&output.stdout).context("parsing nix path-info json in smoke test")?;

    infos
        .remove(store_path)
        .ok_or_else(|| anyhow::anyhow!("missing path info for {}", store_path))
}

async fn expected_nar_dump_bytes(store_path: &str) -> Result<Vec<u8>> {
    let output = Command::new("nix-store")
        .args(["--dump", store_path])
        .output()
        .await
        .with_context(|| format!("running nix-store --dump for {}", store_path))?;

    if !output.status.success() {
        bail!(
            "nix-store --dump failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(output.stdout)
}

fn cache_ctl_command() -> Command {
    let mut command = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned()));
    command.args(["run", "-p", "cache-ctl", "--quiet", "--"]);
    command
}

async fn run_cache_ctl(server_url: &str, args: &[&str]) -> Result<std::process::Output> {
    let output = cache_ctl_command()
        .args(["--server", server_url, "--auth-token", WRITE_TOKEN])
        .args(args)
        .output()
        .await
        .context("running cache-ctl")?;

    if !output.status.success() {
        bail!(
            "cache-ctl failed\nargs: {:?}\nstdout:\n{}\n\nstderr:\n{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(output)
}

fn admin_and_oidc_authorizer(
    oidc_server: &TestGitHubActionsOidcServer,
    audience: &str,
) -> ChainAuthorizer {
    let mut authorizer = ChainAuthorizer::new();
    authorizer.push(Arc::new(StaticTokenAuthorizer::new(Some(
        WRITE_TOKEN.to_owned(),
    ))));
    authorizer.push(Arc::new(OidcAuthorizer::new(
        OidcConfig {
            providers: BTreeMap::from([(
                "github".to_owned(),
                OidcProviderConfig {
                    issuer: oidc_server.base_url().to_owned(),
                    audience: audience.to_owned(),
                    repository_claim: None,
                    ref_claim: None,
                    bound_claims: BTreeMap::from([(
                        "repository".to_owned(),
                        vec!["owner/repo".to_owned()],
                    )]),
                    bound_subject: vec!["repo:owner/repo:*".to_owned()],
                },
            )]),
            allow_insecure: true,
        },
        Arc::new(ReqwestOidcHttpClient::default()),
    )));

    authorizer
}

#[tokio::test]
async fn cache_push_can_publish_and_read_back_path() -> Result<()> {
    if skip_unless_nix_available().await.is_err() {
        return Ok(());
    }

    let fixture = TestDatabase::new().await?;
    let project = example_project();
    fixture.insert_example_project().await?;

    let app = build_app_with_parts(
        AppParts {
            db: fixture.db.clone(),
            store_dir: cache_core::nix::StoreDir::default(),
            aggregate_signing_key: Some(test_signing_key()),
            key_encryption_key: None,
            mode: AppMode::ReadWrite,
            storage_catalog: filesystem_storage_in(&fixture.temp_dir),
            upstream_client: Arc::new(ReqwestUpstreamCacheClient::default()),
        },
        Some(WRITE_TOKEN.to_owned()),
    );
    let server = TestServer::spawn(app).await?;

    let input_path = fixture.temp_dir.path().join("hello.txt");
    fs::write(&input_path, b"hello from cache-push smoke test")
        .await
        .context("writing smoke-test input file")?;

    let store_path = add_fixed_store_path(&input_path).await?;
    let path_info = path_info_for(&store_path).await?;
    let store_path_hash = path_info
        .store_path_hash()
        .context("deriving store path hash in smoke test")?;
    let object_path = format!("nar/{}.nar.zst", path_info.nar_hash.normalize()?.digest());
    let expected_nar_dump = expected_nar_dump_bytes(&store_path).await?;

    let binary = env!("CARGO_BIN_EXE_cache-push");
    let output = Command::new(binary)
        .args([
            "--server",
            &server.base_url,
            "--auth-token",
            WRITE_TOKEN,
            "--project",
            project.as_str(),
            "--ref",
            "refs/heads/main",
            "--revision",
            "deadbeef",
            "--max-concurrent-uploads",
            "1",
            &store_path,
        ])
        .output()
        .await
        .context("running cache-push smoke test binary")?;

    if !output.status.success() {
        bail!(
            "cache-push failed\nstdout:\n{}\n\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let read_client = reqwest::Client::new();

    let narinfo_response = read_client
        .get(server.url(format!("{}.narinfo", store_path_hash.as_str())))
        .send()
        .await
        .context("fetching aggregate narinfo after cache-push")?;

    assert_eq!(narinfo_response.status(), StatusCode::OK);
    assert_eq!(
        narinfo_response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .unwrap(),
        "text/x-nix-narinfo"
    );

    let narinfo_body = narinfo_response.text().await?;
    assert!(narinfo_body.contains(&format!("StorePath: {}", store_path)));
    assert!(narinfo_body.contains(&format!("URL: {}", object_path)));
    assert!(narinfo_body.contains("Sig: cache.example.com-1:"));

    let object_response = read_client
        .get(server.url(&object_path))
        .send()
        .await
        .context("fetching aggregate nar object after cache-push")?;

    assert_eq!(object_response.status(), StatusCode::OK);
    assert_eq!(
        object_response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .unwrap(),
        "application/octet-stream"
    );

    let object_bytes = object_response.bytes().await?;
    let decoded_object_bytes =
        zstd::stream::decode_all(object_bytes.as_ref()).context("decoding uploaded nar object")?;
    assert_eq!(decoded_object_bytes, expected_nar_dump);

    let project_narinfo_response = read_client
        .get(server.url(format!(
            "p/{}/{}.narinfo",
            project.as_str(),
            store_path_hash.as_str()
        )))
        .send()
        .await
        .context("fetching project narinfo after cache-push")?;

    assert_eq!(project_narinfo_response.status(), StatusCode::OK);

    let project_object_response = read_client
        .get(server.url(format!("p/{}/{}", project.as_str(), object_path)))
        .send()
        .await
        .context("fetching project nar object after cache-push")?;

    assert_eq!(project_object_response.status(), StatusCode::OK);

    let _ = EXAMPLE_PROJECT_NAME;

    Ok(())
}

#[tokio::test]
async fn cache_push_can_publish_with_github_oidc_token() -> Result<()> {
    if skip_unless_nix_available().await.is_err() {
        return Ok(());
    }

    let fixture = TestDatabase::new().await?;
    let project = example_project();
    fixture.insert_example_project().await?;

    let app_audience = "cache-push-oidc-smoke-test";
    let oidc_server =
        TestGitHubActionsOidcServer::spawn(app_audience, "owner/repo", "refs/heads/main").await?;

    fixture
        .db
        .replace_project_oidc_identity(
            &project,
            "github",
            "owner/repo",
            &["refs/heads/main".to_owned()],
        )
        .await
        .context("inserting project OIDC binding")?;

    let oidc_authorizer = OidcAuthorizer::new(
        OidcConfig {
            providers: BTreeMap::from([(
                "github".to_owned(),
                OidcProviderConfig {
                    issuer: oidc_server.base_url().to_owned(),
                    audience: app_audience.to_owned(),
                    repository_claim: None,
                    ref_claim: None,
                    bound_claims: BTreeMap::from([(
                        "repository".to_owned(),
                        vec!["owner/repo".to_owned()],
                    )]),
                    bound_subject: vec!["repo:owner/repo:*".to_owned()],
                },
            )]),
            allow_insecure: true,
        },
        Arc::new(ReqwestOidcHttpClient::default()),
    );

    let app = build_app_with_authorizer(
        AppParts {
            db: fixture.db.clone(),
            store_dir: cache_core::nix::StoreDir::default(),
            aggregate_signing_key: Some(test_signing_key()),
            key_encryption_key: None,
            mode: AppMode::ReadWrite,
            storage_catalog: filesystem_storage_in(&fixture.temp_dir),
            upstream_client: Arc::new(ReqwestUpstreamCacheClient::default()),
        },
        Arc::new(oidc_authorizer),
    );
    let server = TestServer::spawn(app).await?;

    let input_path = fixture.temp_dir.path().join("hello-oidc.txt");
    fs::write(&input_path, b"hello from OIDC cache-push smoke test")
        .await
        .context("writing OIDC smoke-test input file")?;

    let store_path = add_fixed_store_path(&input_path).await?;
    let path_info = path_info_for(&store_path).await?;
    let store_path_hash = path_info
        .store_path_hash()
        .context("deriving store path hash in OIDC smoke test")?;
    let object_path = format!("nar/{}.nar.zst", path_info.nar_hash.normalize()?.digest());
    let expected_nar_dump = expected_nar_dump_bytes(&store_path).await?;

    let binary = env!("CARGO_BIN_EXE_cache-push");
    let output = Command::new(binary)
        .args([
            "--server",
            &server.base_url,
            "--oidc-audience",
            app_audience,
            "--project",
            project.as_str(),
            "--ref",
            "refs/heads/main",
            "--revision",
            "deadbeef",
            "--max-concurrent-uploads",
            "1",
            &store_path,
        ])
        .env_remove("CACHE_WRITE_TOKEN")
        .env("ACTIONS_ID_TOKEN_REQUEST_URL", oidc_server.url("token"))
        .env("ACTIONS_ID_TOKEN_REQUEST_TOKEN", GITHUB_OIDC_REQUEST_TOKEN)
        .output()
        .await
        .context("running cache-push OIDC smoke test binary")?;

    if !output.status.success() {
        bail!(
            "cache-push OIDC failed\nstdout:\n{}\n\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    assert_eq!(
        oidc_server.token_requests().as_slice(),
        &[RecordedOidcTokenRequest {
            authorization: Some(format!("Bearer {GITHUB_OIDC_REQUEST_TOKEN}")),
            audience: Some(app_audience.to_owned()),
        }]
    );

    let read_client = reqwest::Client::new();

    let narinfo_response = read_client
        .get(server.url(format!("{}.narinfo", store_path_hash.as_str())))
        .send()
        .await
        .context("fetching aggregate narinfo after OIDC cache-push")?;

    assert_eq!(narinfo_response.status(), StatusCode::OK);

    let narinfo_body = narinfo_response.text().await?;
    assert!(narinfo_body.contains(&format!("StorePath: {}", store_path)));
    assert!(narinfo_body.contains(&format!("URL: {}", object_path)));

    let object_response = read_client
        .get(server.url(&object_path))
        .send()
        .await
        .context("fetching aggregate nar object after OIDC cache-push")?;

    assert_eq!(object_response.status(), StatusCode::OK);

    let object_bytes = object_response.bytes().await?;
    let decoded_object_bytes =
        zstd::stream::decode_all(object_bytes.as_ref()).context("decoding uploaded nar object")?;
    assert_eq!(decoded_object_bytes, expected_nar_dump);

    let project_narinfo_response = read_client
        .get(server.url(format!(
            "p/{}/{}.narinfo",
            project.as_str(),
            store_path_hash.as_str()
        )))
        .send()
        .await
        .context("fetching project narinfo after OIDC cache-push")?;

    assert_eq!(project_narinfo_response.status(), StatusCode::OK);

    let project_object_response = read_client
        .get(server.url(format!("p/{}/{}", project.as_str(), object_path)))
        .send()
        .await
        .context("fetching project nar object after OIDC cache-push")?;

    assert_eq!(project_object_response.status(), StatusCode::OK);

    Ok(())
}

#[tokio::test]
async fn cache_ctl_setup_then_cache_push_with_github_oidc_works() -> Result<()> {
    if skip_unless_nix_available().await.is_err() {
        return Ok(());
    }

    let fixture = TestDatabase::new().await?;
    let project = example_project();
    let audience = "cache-push-cache-ctl-oidc-smoke-test";

    let oidc_server =
        TestGitHubActionsOidcServer::spawn(audience, "owner/repo", "refs/heads/main").await?;

    let app = build_app_with_authorizer(
        AppParts {
            db: fixture.db.clone(),
            store_dir: cache_core::nix::StoreDir::default(),
            aggregate_signing_key: Some(test_signing_key()),
            key_encryption_key: None,
            mode: AppMode::ReadWrite,
            storage_catalog: filesystem_storage_in(&fixture.temp_dir),
            upstream_client: Arc::new(ReqwestUpstreamCacheClient::default()),
        },
        Arc::new(admin_and_oidc_authorizer(&oidc_server, audience)),
    );
    let server = TestServer::spawn(app).await?;

    run_cache_ctl(
        &server.base_url,
        &[
            "projects",
            "create",
            project.as_str(),
            "--display-name",
            "owner/repo",
            "--public",
            "--if-not-exists",
        ],
    )
    .await?;

    run_cache_ctl(
        &server.base_url,
        &[
            "projects",
            "oidc",
            "add",
            project.as_str(),
            "--provider",
            "github",
            "--repository",
            "owner/repo",
            "--ref",
            "refs/heads/main",
            "--if-not-exists",
        ],
    )
    .await?;

    let input_path = fixture.temp_dir.path().join("hello-cache-ctl-oidc.txt");
    fs::write(&input_path, b"hello from cache-ctl OIDC setup smoke test")
        .await
        .context("writing cache-ctl OIDC smoke-test input file")?;

    let store_path = add_fixed_store_path(&input_path).await?;
    let path_info = path_info_for(&store_path).await?;
    let store_path_hash = path_info
        .store_path_hash()
        .context("deriving store path hash in cache-ctl OIDC smoke test")?;
    let object_path = format!("nar/{}.nar.zst", path_info.nar_hash.normalize()?.digest());
    let expected_nar_dump = expected_nar_dump_bytes(&store_path).await?;

    let binary = env!("CARGO_BIN_EXE_cache-push");
    let output = Command::new(binary)
        .args([
            "--server",
            &server.base_url,
            "--oidc-audience",
            audience,
            "--project",
            project.as_str(),
            "--ref",
            "refs/heads/main",
            "--revision",
            "deadbeef",
            "--max-concurrent-uploads",
            "1",
            &store_path,
        ])
        .env_remove("CACHE_WRITE_TOKEN")
        .env("ACTIONS_ID_TOKEN_REQUEST_URL", oidc_server.url("token"))
        .env("ACTIONS_ID_TOKEN_REQUEST_TOKEN", GITHUB_OIDC_REQUEST_TOKEN)
        .output()
        .await
        .context("running cache-push after cache-ctl OIDC setup")?;

    if !output.status.success() {
        bail!(
            "cache-push after cache-ctl OIDC setup failed\nstdout:\n{}\n\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let read_client = reqwest::Client::new();

    let narinfo_response = read_client
        .get(server.url(format!("{}.narinfo", store_path_hash.as_str())))
        .send()
        .await
        .context("fetching aggregate narinfo after cache-ctl OIDC setup")?;
    assert_eq!(narinfo_response.status(), StatusCode::OK);

    let object_response = read_client
        .get(server.url(&object_path))
        .send()
        .await
        .context("fetching aggregate object after cache-ctl OIDC setup")?;
    assert_eq!(object_response.status(), StatusCode::OK);

    let object_bytes = object_response.bytes().await?;
    let decoded_object_bytes =
        zstd::stream::decode_all(object_bytes.as_ref()).context("decoding uploaded nar object")?;
    assert_eq!(decoded_object_bytes, expected_nar_dump);

    let project_narinfo_response = read_client
        .get(server.url(format!(
            "p/{}/{}.narinfo",
            project.as_str(),
            store_path_hash.as_str()
        )))
        .send()
        .await
        .context("fetching project narinfo after cache-ctl OIDC setup")?;
    assert_eq!(project_narinfo_response.status(), StatusCode::OK);

    let project_object_response = read_client
        .get(server.url(format!("p/{}/{}", project.as_str(), object_path)))
        .send()
        .await
        .context("fetching project object after cache-ctl OIDC setup")?;
    assert_eq!(project_object_response.status(), StatusCode::OK);

    Ok(())
}

#[tokio::test]
async fn cache_ctl_token_create_then_cache_push_with_project_token_enforces_ref_scope() -> Result<()>
{
    if skip_unless_nix_available().await.is_err() {
        return Ok(());
    }

    let fixture = TestDatabase::new().await?;
    let project = example_project();

    let app = build_app_with_parts(
        AppParts {
            db: fixture.db.clone(),
            store_dir: cache_core::nix::StoreDir::default(),
            aggregate_signing_key: Some(test_signing_key()),
            key_encryption_key: None,
            mode: AppMode::ReadWrite,
            storage_catalog: filesystem_storage_in(&fixture.temp_dir),
            upstream_client: Arc::new(ReqwestUpstreamCacheClient::default()),
        },
        Some(WRITE_TOKEN.to_owned()),
    );
    let server = TestServer::spawn(app).await?;

    run_cache_ctl(
        &server.base_url,
        &[
            "projects",
            "create",
            project.as_str(),
            "--display-name",
            "owner/repo",
            "--public",
            "--if-not-exists",
        ],
    )
    .await?;

    let token_output = run_cache_ctl(
        &server.base_url,
        &[
            "--json",
            "tokens",
            "create",
            "ci-main",
            "--project",
            project.as_str(),
            "--ref",
            "refs/heads/main",
        ],
    )
    .await?;

    let token_response: serde_json::Value =
        serde_json::from_slice(&token_output.stdout).context("parsing cache-ctl token JSON")?;
    let token = token_response["token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("cache-ctl token response did not contain token"))?
        .to_owned();

    let input_path = fixture.temp_dir.path().join("hello-project-token.txt");
    fs::write(&input_path, b"hello from project-token smoke test")
        .await
        .context("writing project-token smoke-test input file")?;

    let store_path = add_fixed_store_path(&input_path).await?;
    let path_info = path_info_for(&store_path).await?;
    let store_path_hash = path_info
        .store_path_hash()
        .context("deriving store path hash in project-token smoke test")?;

    let binary = env!("CARGO_BIN_EXE_cache-push");

    let ok_output = Command::new(binary)
        .args([
            "--server",
            &server.base_url,
            "--project",
            project.as_str(),
            "--ref",
            "refs/heads/main",
            "--revision",
            "deadbeef",
            "--max-concurrent-uploads",
            "1",
            &store_path,
        ])
        .env("CACHE_WRITE_TOKEN", &token)
        .output()
        .await
        .context("running cache-push with project access token")?;

    if !ok_output.status.success() {
        bail!(
            "cache-push with project access token failed\nstdout:\n{}\n\nstderr:\n{}",
            String::from_utf8_lossy(&ok_output.stdout),
            String::from_utf8_lossy(&ok_output.stderr)
        );
    }

    let read_client = reqwest::Client::new();
    let project_narinfo_response = read_client
        .get(server.url(format!(
            "p/{}/{}.narinfo",
            project.as_str(),
            store_path_hash.as_str()
        )))
        .send()
        .await
        .context("fetching project narinfo after project-token push")?;
    assert_eq!(project_narinfo_response.status(), StatusCode::OK);

    let wrong_ref_output = Command::new(binary)
        .args([
            "--server",
            &server.base_url,
            "--project",
            project.as_str(),
            "--ref",
            "refs/heads/feature",
            "--revision",
            "deadbeef",
            "--max-concurrent-uploads",
            "1",
            &store_path,
        ])
        .env("CACHE_WRITE_TOKEN", &token)
        .output()
        .await
        .context("running cache-push with project access token on wrong ref")?;

    assert!(
        !wrong_ref_output.status.success(),
        "cache-push unexpectedly succeeded on wrong ref\nstdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&wrong_ref_output.stdout),
        String::from_utf8_lossy(&wrong_ref_output.stderr)
    );

    Ok(())
}
