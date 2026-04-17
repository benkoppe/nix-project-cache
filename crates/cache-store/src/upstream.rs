use std::collections::HashMap;

use anyhow::{Context as _, Result, anyhow};
use async_trait::async_trait;
use reqwest::StatusCode;
use uuid::Uuid;

use cache_core::nix::StorePathHash;

use crate::blob::{BlobBytes, BlobMetadata};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamCache {
    pub id: Uuid,
    pub name: String,
    pub base_url: String,
    pub priority: u32,
}

impl UpstreamCache {
    pub fn new(
        id: Uuid,
        name: impl Into<String>,
        base_url: impl Into<String>,
        priority: u32,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            priority,
        }
    }

    pub fn narinfo_url(&self, store_path_hash: &StorePathHash) -> String {
        format!("{}/{}.narinfo", self.base_url, store_path_hash.as_str())
    }

    pub fn object_url(&self, object_path: &str) -> String {
        format!("{}/{}", self.base_url, object_path)
    }
}

#[async_trait]
pub trait UpstreamCacheClient: Send + Sync + 'static {
    async fn fetch_narinfo_text(
        &self,
        upstream: &UpstreamCache,
        store_path_hash: &StorePathHash,
    ) -> Result<Option<String>>;

    async fn get_object(
        &self,
        upstream: &UpstreamCache,
        object_path: &str,
    ) -> Result<Option<(BlobMetadata, BlobBytes)>>;
}

#[derive(Debug, Clone)]
pub struct ReqwestUpstreamCacheClient {
    client: reqwest::Client,
}

impl ReqwestUpstreamCacheClient {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
}

impl Default for ReqwestUpstreamCacheClient {
    fn default() -> Self {
        let client = reqwest::Client::builder()
            .user_agent("nix-project-cache/0.1")
            .build()
            .expect("building reqwest client for upstream cache access");

        Self::new(client)
    }
}

#[async_trait]
impl UpstreamCacheClient for ReqwestUpstreamCacheClient {
    async fn fetch_narinfo_text(
        &self,
        upstream: &UpstreamCache,
        store_path_hash: &StorePathHash,
    ) -> Result<Option<String>> {
        let response = self
            .client
            .get(upstream.narinfo_url(store_path_hash))
            .send()
            .await
            .with_context(|| format!("fetching narinfo from upstream {}", upstream.name))?;

        match response.status() {
            StatusCode::OK => Ok(Some(response.text().await.context("reading narinfo body")?)),
            StatusCode::NOT_FOUND => Ok(None),
            status => Err(anyhow!(
                "upstream {} returned unexpected status {} for narinfo",
                upstream.name,
                status
            )),
        }
    }

    async fn get_object(
        &self,
        upstream: &UpstreamCache,
        object_path: &str,
    ) -> Result<Option<(BlobMetadata, BlobBytes)>> {
        let response = self
            .client
            .get(upstream.object_url(object_path))
            .send()
            .await
            .with_context(|| format!("fetching object from upstream {}", upstream.name))?;

        match response.status() {
            StatusCode::OK => {
                let metadata = blob_metadata_from_response(&response);
                let bytes = response.bytes().await.context("reading object body")?;
                Ok(Some((metadata, bytes)))
            }
            StatusCode::NOT_FOUND => Ok(None),
            status => Err(anyhow!(
                "upstream {} returned unexpected status {} for object {}",
                upstream.name,
                status,
                object_path
            )),
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct InMemoryUpstreamCacheClient {
    narinfos: HashMap<(Uuid, String), String>,
    objects: HashMap<(Uuid, String), (BlobMetadata, BlobBytes)>,
}

impl InMemoryUpstreamCacheClient {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert_narinfo(
        &mut self,
        upstream_id: Uuid,
        store_path_hash: impl Into<String>,
        narinfo_text: impl Into<String>,
    ) {
        self.narinfos
            .insert((upstream_id, store_path_hash.into()), narinfo_text.into());
    }

    pub fn insert_object(
        &mut self,
        upstream_id: Uuid,
        object_path: impl Into<String>,
        metadata: BlobMetadata,
        bytes: BlobBytes,
    ) {
        self.objects
            .insert((upstream_id, object_path.into()), (metadata, bytes));
    }
}

#[async_trait]
impl UpstreamCacheClient for InMemoryUpstreamCacheClient {
    async fn fetch_narinfo_text(
        &self,
        upstream: &UpstreamCache,
        store_path_hash: &StorePathHash,
    ) -> Result<Option<String>> {
        Ok(self
            .narinfos
            .get(&(upstream.id, store_path_hash.as_str().to_owned()))
            .cloned())
    }

    async fn get_object(
        &self,
        upstream: &UpstreamCache,
        object_path: &str,
    ) -> Result<Option<(BlobMetadata, BlobBytes)>> {
        Ok(self
            .objects
            .get(&(upstream.id, object_path.to_owned()))
            .cloned())
    }
}

fn blob_metadata_from_response(response: &reqwest::Response) -> BlobMetadata {
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_owned();

    let content_length = response.content_length();

    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);

    BlobMetadata::new(content_type, content_length, etag, None)
}
