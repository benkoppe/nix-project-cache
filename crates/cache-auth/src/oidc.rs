use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use jsonwebtoken::jwk::{Jwk, JwkSet};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use serde_json::{Map, Value};
use tokio::sync::RwLock;

use cache_core::project::ProjectSlug;

use crate::oidc_claims::{get_string_claim, validate_bound_claims, validate_bound_subject};
use crate::oidc_config::{ConfiguredOidcProvider, OidcConfig, OidcProviderConfig};
use crate::oidc_http::{OidcHttpClient, OidcHttpError};
use crate::{AuthError, Authorizer, Principal};

#[derive(Debug, Clone, Deserialize)]
struct DiscoveryDocument {
    issuer: String,
    jwks_uri: String,
}

#[derive(Clone)]
struct CachedProviderState {
    discovery: DiscoveryDocument,
    jwks: Arc<JwkSet>,
}

#[derive(Clone)]
pub struct OidcAuthorizer {
    config: OidcConfig,
    http_client: Arc<dyn OidcHttpClient>,
    cache: Arc<RwLock<HashMap<String, CachedProviderState>>>,
}

impl OidcAuthorizer {
    pub fn new(config: OidcConfig, http_client: Arc<dyn OidcHttpClient>) -> Self {
        Self {
            config,
            http_client,
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    async fn cached_or_fetch_provider_state(
        &self,
        provider: ConfiguredOidcProvider<'_>,
    ) -> Result<CachedProviderState, AuthError> {
        if let Some(state) = self.cache.read().await.get(provider.name).cloned() {
            return Ok(state);
        }

        self.refresh_provider_state(provider).await
    }

    async fn refresh_provider_state(
        &self,
        provider: ConfiguredOidcProvider<'_>,
    ) -> Result<CachedProviderState, AuthError> {
        let discovery = self.fetch_discovery(provider.config).await?;
        let jwks = Arc::new(self.fetch_jwks(&discovery.jwks_uri).await?);

        let state = CachedProviderState { discovery, jwks };

        self.cache
            .write()
            .await
            .insert(provider.name.to_owned(), state.clone());

        Ok(state)
    }

    async fn fetch_discovery(
        &self,
        provider: &OidcProviderConfig,
    ) -> Result<DiscoveryDocument, AuthError> {
        let issuer = provider.issuer.trim_end_matches('/');
        let url = format!("{issuer}/.well-known/openid-configuration");
        let body = self
            .http_client
            .fetch_text(&url)
            .await
            .map_err(map_http_error)?;

        let discovery = serde_json::from_str::<DiscoveryDocument>(&body).map_err(|error| {
            AuthError::Unavailable(format!("invalid discovery document: {}", error))
        })?;

        if discovery.issuer != provider.issuer {
            return Err(AuthError::InvalidToken);
        }

        Ok(discovery)
    }

    async fn fetch_jwks(&self, jwks_uri: &str) -> Result<JwkSet, AuthError> {
        let body = self
            .http_client
            .fetch_text(jwks_uri)
            .await
            .map_err(map_http_error)?;

        serde_json::from_str::<JwkSet>(&body)
            .map_err(|error| AuthError::Unavailable(format!("invalid JWKS document: {}", error)))
    }
}

#[async_trait]
impl Authorizer for OidcAuthorizer {
    async fn authorize_bearer(&self, bearer_token: Option<&str>) -> Result<Principal, AuthError> {
        let token = bearer_token.ok_or(AuthError::MissingToken)?;
        let unverified_claims = decode_claims_unverified(token)?;
        let issuer = get_string_claim(&unverified_claims, "iss").ok_or(AuthError::InvalidToken)?;

        let provider = self
            .config
            .provider_for_issuer(&issuer)
            .ok_or(AuthError::InvalidToken)?;

        let header = decode_header(token).map_err(|_| AuthError::InvalidToken)?;
        let kid = header.kid.ok_or(AuthError::InvalidToken)?;

        let mut provider_state = self.cached_or_fetch_provider_state(provider).await?;
        let mut jwk = select_jwk(&provider_state.jwks, &kid);

        if jwk.is_none() {
            provider_state = self.refresh_provider_state(provider).await?;
            jwk = select_jwk(&provider_state.jwks, &kid);
        }

        let jwk = jwk.ok_or(AuthError::InvalidToken)?;
        let decoding_key = DecodingKey::from_jwk(jwk).map_err(|_| AuthError::InvalidToken)?;

        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(&[provider.config.audience.as_str()]);
        validation.set_issuer(&[provider_state.discovery.issuer.as_str()]);
        validation.validate_exp = true;
        validation.validate_nbf = true;

        let verified = decode::<Value>(token, &decoding_key, &validation)
            .map_err(|_| AuthError::InvalidToken)?;

        let claims = verified
            .claims
            .as_object()
            .cloned()
            .ok_or(AuthError::InvalidToken)?;

        validate_bound_subject(&claims, &provider.config.bound_subject)
            .map_err(|_| AuthError::InvalidToken)?;
        validate_bound_claims(&claims, &provider.config.bound_claims)
            .map_err(|_| AuthError::InvalidToken)?;

        let subject = get_string_claim(&claims, "sub").ok_or(AuthError::InvalidToken)?;
        let ref_name = get_string_claim(&claims, "ref");
        let project = get_string_claim(&claims, "project_slug")
            .and_then(|slug| ProjectSlug::parse(&slug).ok());

        Ok(Principal {
            subject,
            provider: Some(provider.name.to_owned()),
            project,
            ref_name,
        })
    }
}

fn select_jwk<'a>(jwks: &'a JwkSet, kid: &str) -> Option<&'a Jwk> {
    jwks.keys
        .iter()
        .find(|jwk| jwk.common.key_id.as_deref() == Some(kid))
}

fn decode_claims_unverified(token: &str) -> Result<Map<String, Value>, AuthError> {
    let mut parts = token.split('.');
    let _header = parts.next().ok_or(AuthError::InvalidToken)?;
    let claims = parts.next().ok_or(AuthError::InvalidToken)?;
    let _signature = parts.next().ok_or(AuthError::InvalidToken)?;

    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(claims)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(claims))
        .map_err(|_| AuthError::InvalidToken)?;

    let value = serde_json::from_slice::<Value>(&decoded).map_err(|_| AuthError::InvalidToken)?;
    value.as_object().cloned().ok_or(AuthError::InvalidToken)
}

fn map_http_error(error: OidcHttpError) -> AuthError {
    AuthError::Unavailable(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use base64::Engine as _;
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    use rsa::pkcs8::{EncodePrivateKey, LineEnding};
    use rsa::traits::PublicKeyParts;
    use rsa::{RsaPrivateKey, RsaPublicKey};
    use serde::Serialize;
    use serde_json::json;

    use crate::{OidcConfig, OidcProviderConfig, StaticOidcHttpClient};

    use super::*;

    const ISSUER: &str = "https://token.actions.githubusercontent.com";
    const AUDIENCE: &str = "https://cache.example.com";
    const DISCOVERY_URL: &str =
        "https://token.actions.githubusercontent.com/.well-known/openid-configuration";
    const JWKS_URL: &str = "https://token.actions.githubusercontent.com/.well-known/jwks";
    const TEST_KID: &str = "test-kid-1";

    #[derive(Debug, Serialize)]
    struct TestClaims {
        iss: String,
        aud: String,
        sub: String,
        exp: usize,
        nbf: usize,
        iat: usize,
        r#ref: String,
        repository: String,
        project_slug: String,
    }

    fn oidc_config() -> OidcConfig {
        OidcConfig {
            providers: BTreeMap::from([(
                "github".to_owned(),
                OidcProviderConfig {
                    issuer: ISSUER.to_owned(),
                    audience: AUDIENCE.to_owned(),
                    bound_claims: BTreeMap::from([(
                        "repository".to_owned(),
                        vec!["owner/repo".to_owned()],
                    )]),
                    bound_subject: vec!["repo:owner/repo:*".to_owned()],
                },
            )]),
            allow_insecure: false,
        }
    }

    fn build_test_http_client() -> (StaticOidcHttpClient, EncodingKey) {
        let mut rng = rsa::rand_core::OsRng;
        let private_key = RsaPrivateKey::new(&mut rng, 2048).unwrap();
        let public_key = RsaPublicKey::from(&private_key);

        let private_pem = private_key
            .to_pkcs8_pem(LineEnding::LF)
            .unwrap()
            .to_string();

        let n =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(public_key.n().to_bytes_be());
        let e =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(public_key.e().to_bytes_be());

        let discovery = json!({
            "issuer": ISSUER,
            "jwks_uri": JWKS_URL,
        });

        let jwks = json!({
            "keys": [
                {
                    "kty": "RSA",
                    "kid": TEST_KID,
                    "use": "sig",
                    "alg": "RS256",
                    "n": n,
                    "e": e
                }
            ]
        });

        let mut http = StaticOidcHttpClient::new();
        http.insert(DISCOVERY_URL, discovery.to_string());
        http.insert(JWKS_URL, jwks.to_string());

        (
            http,
            EncodingKey::from_rsa_pem(private_pem.as_bytes()).unwrap(),
        )
    }

    fn issue_token(encoding_key: &EncodingKey, claims: TestClaims) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(TEST_KID.to_owned());

        encode(&header, &claims, encoding_key).unwrap()
    }

    fn valid_claims() -> TestClaims {
        TestClaims {
            iss: ISSUER.to_owned(),
            aud: AUDIENCE.to_owned(),
            sub: "repo:owner/repo:ref:refs/heads/main".to_owned(),
            exp: 4_102_444_800,
            nbf: 1_700_000_000,
            iat: 1_700_000_000,
            r#ref: "refs/heads/main".to_owned(),
            repository: "owner/repo".to_owned(),
            project_slug: "example_repo".to_owned(),
        }
    }

    #[tokio::test]
    async fn oidc_authorizer_accepts_valid_token() {
        let (http, encoding_key) = build_test_http_client();
        let authorizer = OidcAuthorizer::new(oidc_config(), Arc::new(http));
        let token = issue_token(&encoding_key, valid_claims());

        let principal = authorizer.authorize_bearer(Some(&token)).await.unwrap();

        assert_eq!(principal.subject, "repo:owner/repo:ref:refs/heads/main");
        assert_eq!(principal.provider.as_deref(), Some("github"));
        assert_eq!(principal.ref_name.as_deref(), Some("refs/heads/main"));
        assert_eq!(
            principal.project.as_ref().map(|project| project.as_str()),
            Some("example_repo")
        );
    }

    #[tokio::test]
    async fn oidc_authorizer_rejects_wrong_audience() {
        let (http, encoding_key) = build_test_http_client();
        let authorizer = OidcAuthorizer::new(oidc_config(), Arc::new(http));

        let mut claims = valid_claims();
        claims.aud = "https://wrong.example.com".to_owned();

        let token = issue_token(&encoding_key, claims);
        let error = authorizer.authorize_bearer(Some(&token)).await.unwrap_err();

        assert_eq!(error, AuthError::InvalidToken);
    }

    #[tokio::test]
    async fn oidc_authorizer_rejects_subject_that_does_not_match_bound_subject() {
        let (http, encoding_key) = build_test_http_client();
        let authorizer = OidcAuthorizer::new(oidc_config(), Arc::new(http));

        let mut claims = valid_claims();
        claims.sub = "repo:someone-else/repo:ref:refs/heads/main".to_owned();

        let token = issue_token(&encoding_key, claims);
        let error = authorizer.authorize_bearer(Some(&token)).await.unwrap_err();

        assert_eq!(error, AuthError::InvalidToken);
    }

    #[tokio::test]
    async fn oidc_authorizer_rejects_claim_that_does_not_match_bound_claims() {
        let (http, encoding_key) = build_test_http_client();
        let authorizer = OidcAuthorizer::new(oidc_config(), Arc::new(http));

        let mut claims = valid_claims();
        claims.repository = "owner/other-repo".to_owned();

        let token = issue_token(&encoding_key, claims);
        let error = authorizer.authorize_bearer(Some(&token)).await.unwrap_err();

        assert_eq!(error, AuthError::InvalidToken);
    }

    #[tokio::test]
    async fn oidc_authorizer_rejects_malformed_token_before_fetch() {
        let authorizer = OidcAuthorizer::new(oidc_config(), Arc::new(StaticOidcHttpClient::new()));
        let error = authorizer
            .authorize_bearer(Some("not-a-real-token"))
            .await
            .unwrap_err();

        assert_eq!(error, AuthError::InvalidToken);
    }

    #[tokio::test]
    async fn oidc_authorizer_returns_unavailable_when_discovery_fetch_fails() {
        let (_http, encoding_key) = build_test_http_client();
        let authorizer = OidcAuthorizer::new(oidc_config(), Arc::new(StaticOidcHttpClient::new()));

        let token = issue_token(&encoding_key, valid_claims());
        let error = authorizer.authorize_bearer(Some(&token)).await.unwrap_err();

        match error {
            AuthError::Unavailable(message) => {
                assert!(message.contains(DISCOVERY_URL));
            }
            other => panic!("expected AuthError::Unavailable, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn oidc_authorizer_returns_missing_token_when_no_bearer_is_provided() {
        let (http, _encoding_key) = build_test_http_client();
        let authorizer = OidcAuthorizer::new(oidc_config(), Arc::new(http));

        let error = authorizer.authorize_bearer(None).await.unwrap_err();

        assert_eq!(error, AuthError::MissingToken);
    }
}
