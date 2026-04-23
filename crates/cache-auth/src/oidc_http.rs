use std::collections::HashMap;

use async_trait::async_trait;
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum OidcHttpError {
    #[error("request failed for {url}: {message}")]
    Request { url: String, message: String },
    #[error("unexpected status {status} for {url}: {body}")]
    Status {
        url: String,
        status: u16,
        body: String,
    },
}

#[async_trait]
pub trait OidcHttpClient: Send + Sync + 'static {
    async fn fetch_text(&self, url: &str) -> Result<String, OidcHttpError>;
}

#[derive(Debug, Clone)]
pub struct ReqwestOidcHttpClient {
    client: reqwest::Client,
}

impl ReqwestOidcHttpClient {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
}

impl Default for ReqwestOidcHttpClient {
    fn default() -> Self {
        let client = reqwest::Client::builder()
            .user_agent("nix-project-cache/0.1")
            .build()
            .expect("building reqwest OIDC client");
        Self::new(client)
    }
}

#[async_trait]
impl OidcHttpClient for ReqwestOidcHttpClient {
    async fn fetch_text(&self, url: &str) -> Result<String, OidcHttpError> {
        let response =
            self.client
                .get(url)
                .send()
                .await
                .map_err(|error| OidcHttpError::Request {
                    url: url.to_owned(),
                    message: error.to_string(),
                })?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| OidcHttpError::Request {
                url: url.to_owned(),
                message: error.to_string(),
            })?;

        if status.is_success() {
            Ok(body)
        } else {
            Err(OidcHttpError::Status {
                url: url.to_owned(),
                status: status.as_u16(),
                body,
            })
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct StaticOidcHttpClient {
    responses: HashMap<String, String>,
}

impl StaticOidcHttpClient {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, url: impl Into<String>, body: impl Into<String>) {
        self.responses.insert(url.into(), body.into());
    }
}

#[async_trait]
impl OidcHttpClient for StaticOidcHttpClient {
    async fn fetch_text(&self, url: &str) -> Result<String, OidcHttpError> {
        self.responses
            .get(url)
            .cloned()
            .ok_or_else(|| OidcHttpError::Status {
                url: url.to_owned(),
                status: 404,
                body: "not found".to_owned(),
            })
    }
}
