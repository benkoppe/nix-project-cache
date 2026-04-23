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
