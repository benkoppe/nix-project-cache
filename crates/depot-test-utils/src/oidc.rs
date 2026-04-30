use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, header};
use axum::routing::get;
use axum::{Json, Router};
use base64::Engine as _;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rsa::pkcs8::{EncodePrivateKey, LineEnding};
use rsa::traits::PublicKeyParts;
use rsa::{RsaPrivateKey, RsaPublicKey};
use serde::Serialize;
use serde_json::Value;

use crate::TestServer;

#[derive(Debug, Clone, Serialize)]
pub struct TestOidcClaims {
    pub iss: String,
    pub aud: String,
    pub sub: String,
    pub exp: usize,
    pub nbf: usize,
    pub iat: usize,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl TestOidcClaims {
    pub fn new(issuer: &str, audience: &str, subject: &str) -> Self {
        Self {
            iss: issuer.to_owned(),
            aud: audience.to_owned(),
            sub: subject.to_owned(),
            exp: 4_102_444_800,
            nbf: 1_700_000_000,
            iat: 1_700_000_000,
            extra: BTreeMap::new(),
        }
    }

    pub fn github_actions(issuer: &str, audience: &str, repository: &str, ref_name: &str) -> Self {
        let mut claims = Self::new(
            issuer,
            audience,
            &format!("repo:{repository}:ref:{ref_name}"),
        );
        claims.set_string_claim("repository", repository);
        claims.set_string_claim("ref", ref_name);
        claims
    }

    pub fn set_string_claim(&mut self, name: &str, value: &str) {
        self.extra
            .insert(name.to_owned(), Value::String(value.to_owned()));
    }

    pub fn with_string_claim(mut self, name: &str, value: &str) -> Self {
        self.set_string_claim(name, value);
        self
    }
}

pub struct TestOidcIssuer {
    issuer: String,
    audience: String,
    kid: String,
    encoding_key: EncodingKey,
    jwks: Value,
}

impl TestOidcIssuer {
    pub fn new(issuer: &str, audience: &str, kid: &str) -> Result<Self> {
        let mut rng = rsa::rand_core::OsRng;
        let private_key = RsaPrivateKey::new(&mut rng, 2048).context("generating test RSA key")?;
        let public_key = RsaPublicKey::from(&private_key);

        let private_pem = private_key
            .to_pkcs8_pem(LineEnding::LF)
            .context("encoding test RSA private key")?;

        let n =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(public_key.n().to_bytes_be());
        let e =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(public_key.e().to_bytes_be());

        let jwks = serde_json::json!({
            "keys": [
                {
                    "kty": "RSA",
                    "use": "sig",
                    "kid": kid,
                    "alg": "RS256",
                    "n": n,
                    "e": e
                }
            ]
        });

        Ok(Self {
            issuer: issuer.to_owned(),
            audience: audience.to_owned(),
            kid: kid.to_owned(),
            encoding_key: EncodingKey::from_rsa_pem(private_pem.as_bytes())
                .context("building test OIDC encoding key")?,
            jwks,
        })
    }

    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    pub fn audience(&self) -> &str {
        &self.audience
    }

    pub fn discovery_url(&self) -> String {
        format!(
            "{}/.well-known/openid-configuration",
            self.issuer.trim_end_matches('/')
        )
    }

    pub fn jwks_url(&self) -> String {
        format!("{}/.well-known/jwks", self.issuer.trim_end_matches('/'))
    }

    pub fn discovery_document(&self) -> Value {
        serde_json::json!({
            "issuer": self.issuer,
            "jwks_uri": self.jwks_url(),
        })
    }

    pub fn jwks_document(&self) -> Value {
        self.jwks.clone()
    }

    pub fn issue_token(&self, claims: &TestOidcClaims) -> Result<String> {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(self.kid.clone());

        encode(&header, claims, &self.encoding_key).context("encoding test OIDC token")
    }

    pub fn github_actions_claims(&self, repository: &str, ref_name: &str) -> TestOidcClaims {
        TestOidcClaims::github_actions(self.issuer(), self.audience(), repository, ref_name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedOidcTokenRequest {
    pub authorization: Option<String>,
    pub audience: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct TokenQuery {
    audience: Option<String>,
}

#[derive(Debug, Clone)]
struct FakeOidcState {
    issuer: String,
    jwks_uri: String,
    jwks: Value,
    token: String,
    token_requests: Arc<Mutex<Vec<RecordedOidcTokenRequest>>>,
}

pub struct TestGitHubActionsOidcServer {
    pub server: TestServer,
    token_requests: Arc<Mutex<Vec<RecordedOidcTokenRequest>>>,
}

impl TestGitHubActionsOidcServer {
    pub async fn spawn(audience: &str, repository: &str, ref_name: &str) -> Result<Self> {
        let token_requests = Arc::new(Mutex::new(Vec::new()));
        let token_requests_for_app = token_requests.clone();
        let audience = audience.to_owned();
        let repository = repository.to_owned();
        let ref_name = ref_name.to_owned();

        let server = TestServer::spawn_with_known_url(move |issuer_url| {
            let issuer = TestOidcIssuer::new(&issuer_url, &audience, "test-oidc-key")
                .expect("building test OIDC issuer");
            let token = issuer
                .issue_token(&issuer.github_actions_claims(&repository, &ref_name))
                .expect("issuing test OIDC token");

            let state = FakeOidcState {
                issuer: issuer_url,
                jwks_uri: issuer.jwks_url(),
                jwks: issuer.jwks_document(),
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

        Ok(Self {
            server,
            token_requests,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.server.base_url
    }

    pub fn url(&self, path: impl AsRef<str>) -> String {
        self.server.url(path)
    }

    pub fn token_requests(&self) -> Vec<RecordedOidcTokenRequest> {
        self.token_requests.lock().unwrap().clone()
    }
}

async fn discovery_handler(State(state): State<FakeOidcState>) -> Json<Value> {
    Json(serde_json::json!({
        "issuer": state.issuer,
        "jwks_uri": state.jwks_uri,
    }))
}

async fn jwks_handler(State(state): State<FakeOidcState>) -> Json<Value> {
    Json(state.jwks)
}

async fn token_handler(
    State(state): State<FakeOidcState>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
) -> Json<Value> {
    state
        .token_requests
        .lock()
        .unwrap()
        .push(RecordedOidcTokenRequest {
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
