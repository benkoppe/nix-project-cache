use std::collections::HashMap;

use anyhow::{Context as _, Result, anyhow};
use async_trait::async_trait;
use reqwest::StatusCode;
use uuid::Uuid;

use depot_core::nix::StorePathHash;

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

    async fn head_object(
        &self,
        upstream: &UpstreamCache,
        object_path: &str,
    ) -> Result<Option<BlobMetadata>>;

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
            .user_agent("repo-depot/0.1")
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

    async fn head_object(
        &self,
        upstream: &UpstreamCache,
        object_path: &str,
    ) -> Result<Option<BlobMetadata>> {
        let response = self
            .client
            .head(upstream.object_url(object_path))
            .send()
            .await
            .with_context(|| format!("heading object from upstream {}", upstream.name))?;

        match response.status() {
            StatusCode::OK => Ok(Some(blob_metadata_from_response(&response))),
            StatusCode::NOT_FOUND => Ok(None),
            StatusCode::METHOD_NOT_ALLOWED => {
                // Some upstreams may not support HEAD cleanly; fall back to GET.
                match self.get_object(upstream, object_path).await? {
                    Some((metadata, _)) => Ok(Some(metadata)),
                    None => Ok(None),
                }
            }
            status => Err(anyhow!(
                "upstream {} returned unexpected status {} for HEAD object {}",
                upstream.name,
                status,
                object_path
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

    async fn head_object(
        &self,
        upstream: &UpstreamCache,
        object_path: &str,
    ) -> Result<Option<BlobMetadata>> {
        Ok(self
            .objects
            .get(&(upstream.id, object_path.to_owned()))
            .map(|(metadata, _)| metadata.clone()))
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

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::http::Method;
    use axum::http::{HeaderValue, StatusCode, header};
    use axum::response::IntoResponse;
    use axum::routing::{any, get};
    use tokio::net::TcpListener;
    use uuid::Uuid;

    use super::*;

    fn sample_store_path_hash() -> StorePathHash {
        StorePathHash::parse_from_store_path(
            "/nix/store/26xbg1ndr7hbcncrlf9nhx5is2b25d13-hello-2.12.1",
        )
        .unwrap()
    }
    fn sample_upstream(base_url: String) -> UpstreamCache {
        UpstreamCache::new(Uuid::now_v7(), "test-upstream", base_url, 10)
    }
    fn sample_narinfo_text() -> &'static str {
        "\
StorePath: /nix/store/26xbg1ndr7hbcncrlf9nhx5is2b25d13-hello-2.12.1
URL: nar/test.nar
Compression: zstd
NarHash: sha256:020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz
NarSize: 226560
References: 
"
    }
    async fn spawn_server(app: Router) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{}", addr)
    }
    async fn narinfo_ok() -> impl IntoResponse {
        (
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/x-nix-narinfo"),
            )],
            sample_narinfo_text(),
        )
    }
    async fn object_ok() -> impl IntoResponse {
        (
            [
                (
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/octet-stream"),
                ),
                (header::ETAG, HeaderValue::from_static("\"abc123\"")),
            ],
            BlobBytes::from_static(b"payload"),
        )
    }
    async fn object_head_405_get_ok(method: Method) -> impl IntoResponse {
        if method == Method::HEAD {
            StatusCode::METHOD_NOT_ALLOWED.into_response()
        } else {
            (
                [
                    (
                        header::CONTENT_TYPE,
                        HeaderValue::from_static("application/octet-stream"),
                    ),
                    (header::ETAG, HeaderValue::from_static("\"fallback\"")),
                ],
                BlobBytes::from_static(b"payload"),
            )
                .into_response()
        }
    }
    #[tokio::test]
    async fn fetch_narinfo_text_returns_text_on_200() {
        let app = Router::new().route("/26xbg1ndr7hbcncrlf9nhx5is2b25d13.narinfo", get(narinfo_ok));
        let base_url = spawn_server(app).await;
        let upstream = sample_upstream(base_url);
        let client = ReqwestUpstreamCacheClient::default();
        let text = client
            .fetch_narinfo_text(&upstream, &sample_store_path_hash())
            .await
            .unwrap()
            .unwrap();
        assert!(
            text.contains("StorePath: /nix/store/26xbg1ndr7hbcncrlf9nhx5is2b25d13-hello-2.12.1")
        );
    }
    #[tokio::test]
    async fn fetch_narinfo_text_returns_none_on_404() {
        let app = Router::new();
        let base_url = spawn_server(app).await;
        let upstream = sample_upstream(base_url);
        let client = ReqwestUpstreamCacheClient::default();
        let text = client
            .fetch_narinfo_text(&upstream, &sample_store_path_hash())
            .await
            .unwrap();
        assert!(text.is_none());
    }
    #[tokio::test]
    async fn head_object_returns_metadata_on_200() {
        let app = Router::new().route("/nar/test.nar", get(object_ok));
        let base_url = spawn_server(app).await;
        let upstream = sample_upstream(base_url);
        let client = ReqwestUpstreamCacheClient::default();
        let metadata = client
            .head_object(&upstream, "nar/test.nar")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(metadata.content_type, "application/octet-stream");
        assert_eq!(metadata.etag.as_deref(), Some("\"abc123\""));
    }
    #[tokio::test]
    async fn head_object_returns_none_on_404() {
        let app = Router::new();
        let base_url = spawn_server(app).await;
        let upstream = sample_upstream(base_url);
        let client = ReqwestUpstreamCacheClient::default();
        let metadata = client.head_object(&upstream, "nar/test.nar").await.unwrap();
        assert!(metadata.is_none());
    }
    #[tokio::test]
    async fn head_object_falls_back_to_get_on_405() {
        let app = Router::new().route("/nar/test.nar", any(object_head_405_get_ok));
        let base_url = spawn_server(app).await;
        let upstream = sample_upstream(base_url);
        let client = ReqwestUpstreamCacheClient::default();
        let metadata = client
            .head_object(&upstream, "nar/test.nar")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(metadata.content_type, "application/octet-stream");
        assert_eq!(metadata.etag.as_deref(), Some("\"fallback\""));
    }
    #[tokio::test]
    async fn in_memory_upstream_head_object_returns_metadata() {
        let upstream = UpstreamCache::new(
            Uuid::now_v7(),
            "test-upstream",
            "https://example.invalid",
            10,
        );
        let mut client = InMemoryUpstreamCacheClient::new();
        let metadata = BlobMetadata::new("application/octet-stream", Some(7), None, None);
        client.insert_object(
            upstream.id,
            "nar/test.nar",
            metadata.clone(),
            BlobBytes::from_static(b"payload"),
        );
        let loaded = client.head_object(&upstream, "nar/test.nar").await.unwrap();
        assert_eq!(loaded, Some(metadata));
    }
}
