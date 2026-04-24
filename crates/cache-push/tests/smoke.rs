use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result, bail};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::routing::get;
use axum::{Json, Router};
use base64::Engine as _;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rsa::pkcs8::{EncodePrivateKey, LineEnding};
use rsa::traits::PublicKeyParts;
use rsa::{RsaPrivateKey, RsaPublicKey};
use serde::Serialize;
use tokio::fs;
use tokio::process::Command;

use cache_app::{build_app_with_authorizer, build_app_with_parts};
use cache_auth::{OidcAuthorizer, OidcConfig, OidcProviderConfig, ReqwestOidcHttpClient};
use cache_core::nix::parse_path_info_json;
use cache_store::upstream::ReqwestUpstreamCacheClient;
use cache_test_utils::{
    EXAMPLE_PROJECT_NAME, TestDatabase, TestServer, example_project, filesystem_backends_in,
    test_signing_keys,
};

const WRITE_TOKEN: &str = "secret-token";
const GITHUB_OIDC_REQUEST_TOKEN: &str = "github-actions-request-token";
const TEST_KID: &str = "cache-push-smoke-test-key";

#[derive(Debug, Clone)]
struct FakeOidcState {
    issuer: String,
    jwks_uri: String,
    jwks: serde_json::Value,
    token: String,
    token_requests: Arc<Mutex<Vec<FakeTokenRequest>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FakeTokenRequest {
    authorization: Option<String>,
    audience: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct FakeTokenQuery {
    audience: Option<String>,
}

#[derive(Debug, Serialize)]
struct TestOidcClaims {
    iss: String,
    aud: String,
    sub: String,
    exp: usize,
    nbf: usize,
    iat: usize,
    r#ref: String,
    repository: String,
}

struct LocalTestServer {
    base_url: String,
    handle: tokio::task::JoinHandle<()>,
}

impl LocalTestServer {
    async fn spawn_with_known_url<F>(build_app: F) -> Result<Self>
    where
        F: FnOnce(String) -> Router,
    {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let base_url = format!("http://{address}");
        let app = build_app(base_url.clone());

        let handle = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("local test server should stay alive");
        });

        Ok(Self { base_url, handle })
    }

    fn url(&self, path: impl AsRef<str>) -> String {
        format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            path.as_ref().trim_start_matches('/')
        )
    }
}

impl Drop for LocalTestServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

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

async fn discovery_handler(State(state): State<FakeOidcState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "issuer": state.issuer,
        "jwks_uri": state.jwks_uri,
    }))
}

async fn jwks_handler(State(state): State<FakeOidcState>) -> Json<serde_json::Value> {
    Json(state.jwks)
}

async fn token_handler(
    State(state): State<FakeOidcState>,
    headers: HeaderMap,
    Query(query): Query<FakeTokenQuery>,
) -> Json<serde_json::Value> {
    state.token_requests.lock().unwrap().push(FakeTokenRequest {
        authorization: headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
        audience: query.audience,
    });

    Json(serde_json::json!({
        "value": state.token,
    }))
}

fn build_test_oidc_token_and_jwks(
    issuer: &str,
    audience: &str,
) -> Result<(String, serde_json::Value)> {
    let mut rng = rsa::rand_core::OsRng;
    let private_key = RsaPrivateKey::new(&mut rng, 2048).context("generating test RSA key")?;
    let public_key = RsaPublicKey::from(&private_key);

    let private_pem = private_key
        .to_pkcs8_pem(LineEnding::LF)
        .context("encoding test RSA private key")?;

    let n = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(public_key.n().to_bytes_be());
    let e = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(public_key.e().to_bytes_be());

    let jwks = serde_json::json!({
        "keys": [
            {
                "kty": "RSA",
                "use": "sig",
                "kid": TEST_KID,
                "alg": "RS256",
                "n": n,
                "e": e
            }
        ]
    });

    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(TEST_KID.to_owned());

    let claims = TestOidcClaims {
        iss: issuer.to_owned(),
        aud: audience.to_owned(),
        sub: "repo:owner/repo:ref:refs/heads/main".to_owned(),
        exp: 4_102_444_800,
        nbf: 1_700_000_000,
        iat: 1_700_000_000,
        r#ref: "refs/heads/main".to_owned(),
        repository: "owner/repo".to_owned(),
    };

    let token = encode(
        &header,
        &claims,
        &EncodingKey::from_rsa_pem(private_pem.as_bytes()).context("building test encoding key")?,
    )
    .context("encoding test OIDC token")?;

    Ok((token, jwks))
}

async fn spawn_fake_github_oidc_issuer(
    audience: &str,
) -> Result<(LocalTestServer, Arc<Mutex<Vec<FakeTokenRequest>>>)> {
    let token_requests = Arc::new(Mutex::new(Vec::new()));
    let token_requests_for_app = token_requests.clone();
    let audience = audience.to_owned();

    let server = LocalTestServer::spawn_with_known_url(move |issuer| {
        let (token, jwks) = build_test_oidc_token_and_jwks(&issuer, &audience)
            .expect("building fake OIDC token and JWKS");

        let state = FakeOidcState {
            issuer: issuer.clone(),
            jwks_uri: format!("{issuer}/.well-known/jwks"),
            jwks,
            token,
            token_requests: token_requests_for_app,
        };

        Router::new()
            .route("/.well-known/openid-configuration", get(discovery_handler))
            .route("/.well-known/jwks", get(jwks_handler))
            .route("/token", get(token_handler))
            .with_state(state)
    })
    .await?;

    Ok((server, token_requests))
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
        fixture.db.clone(),
        cache_core::nix::StoreDir::default(),
        test_signing_keys(),
        filesystem_backends_in(&fixture.temp_dir),
        Some(cache_core::storage::LocalBackendName::fs()),
        Some(WRITE_TOKEN.to_owned()),
        Arc::new(ReqwestUpstreamCacheClient::default()),
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
    let (oidc_server, token_requests) = spawn_fake_github_oidc_issuer(app_audience).await?;

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
                    issuer: oidc_server.base_url.clone(),
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
        fixture.db.clone(),
        cache_core::nix::StoreDir::default(),
        test_signing_keys(),
        filesystem_backends_in(&fixture.temp_dir),
        Some(cache_core::storage::LocalBackendName::fs()),
        Arc::new(oidc_authorizer),
        Arc::new(ReqwestUpstreamCacheClient::default()),
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
        token_requests.lock().unwrap().as_slice(),
        &[FakeTokenRequest {
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
