use anyhow::{Context as _, Result, anyhow};
use async_trait::async_trait;
use aws_credential_types::Credentials;
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{
    Builder as S3ConfigBuilder, Region, RequestChecksumCalculation, ResponseChecksumValidation,
};
use aws_sdk_s3::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_s3::primitives::ByteStream;
use tokio::io::AsyncWriteExt as _;

use crate::blob::BlobBytes;
use crate::local::{CacheStorage, UploadReader};

#[derive(Debug, Clone)]
pub struct S3StorageConfig {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub force_path_style: bool,
    pub prefix: Option<String>,
}

#[derive(Clone)]
pub struct S3Storage {
    client: Client,
    bucket: String,
    prefix: Option<String>,
}

impl S3Storage {
    pub fn new(config: S3StorageConfig) -> Result<Self> {
        let credentials = Credentials::new(
            config.access_key_id,
            config.secret_access_key,
            None,
            None,
            "cache-store-s3",
        );

        let s3_config = S3ConfigBuilder::new()
            .behavior_version_latest()
            .region(Region::new(config.region))
            .credentials_provider(credentials)
            .endpoint_url(config.endpoint)
            .force_path_style(config.force_path_style)
            .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
            .response_checksum_validation(ResponseChecksumValidation::WhenRequired)
            .build();

        Ok(Self {
            client: Client::from_conf(s3_config),
            bucket: config.bucket,
            prefix: normalize_prefix(config.prefix)?,
        })
    }

    pub fn from_client(
        client: Client,
        bucket: impl Into<String>,
        prefix: Option<String>,
    ) -> Result<Self> {
        Ok(Self {
            client,
            bucket: bucket.into(),
            prefix: normalize_prefix(prefix)?,
        })
    }

    fn object_key(&self, object_path: &str) -> Result<String> {
        validate_object_path(object_path)?;

        Ok(match &self.prefix {
            Some(prefix) => format!("{prefix}/{object_path}"),
            None => object_path.to_owned(),
        })
    }
}

fn normalize_prefix(prefix: Option<String>) -> Result<Option<String>> {
    let Some(prefix) = prefix else {
        return Ok(None);
    };

    let prefix = prefix.trim_matches('/').to_owned();
    if prefix.is_empty() {
        return Ok(None);
    }

    validate_object_path(&prefix)?;
    Ok(Some(prefix))
}

fn validate_object_path(object_path: &str) -> Result<()> {
    if object_path.is_empty() {
        return Err(anyhow!("invalid empty object path"));
    }

    if object_path.starts_with('/') {
        return Err(anyhow!("invalid object path {object_path}"));
    }

    for segment in object_path.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(anyhow!("invalid object path {object_path}"));
        }
    }

    Ok(())
}

fn is_not_found<E>(error: &SdkError<E>) -> bool
where
    E: ProvideErrorMetadata,
{
    matches!(error.code(), Some("NoSuchKey" | "NotFound" | "404"))
}

#[async_trait]
impl CacheStorage for S3Storage {
    async fn contains(&self, object_path: &str) -> Result<bool> {
        let key = self.object_key(object_path)?;

        match self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
        {
            Ok(_) => Ok(true),
            Err(error) if is_not_found(&error) => Ok(false),
            Err(error) => Err(error)
                .with_context(|| format!("checking s3 object s3://{}/{}", self.bucket, key)),
        }
    }

    async fn get_bytes(&self, object_path: &str) -> Result<Option<BlobBytes>> {
        let key = self.object_key(object_path)?;

        let object = match self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
        {
            Ok(object) => object,
            Err(error) if is_not_found(&error) => return Ok(None),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("reading S3 object s3://{}/{}", self.bucket, key));
            }
        };

        let bytes = object
            .body
            .collect()
            .await
            .with_context(|| format!("collecting S3 object s3://{}/{}", self.bucket, key))?
            .into_bytes();

        Ok(Some(BlobBytes::from(bytes)))
    }

    async fn put_bytes(&self, object_path: &str, bytes: BlobBytes) -> Result<()> {
        let key = self.object_key(object_path)?;

        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(&key)
            .body(ByteStream::from(bytes))
            .send()
            .await
            .with_context(|| format!("writing S3 object s3://{}/{}", self.bucket, key))?;

        Ok(())
    }

    async fn put_stream(&self, object_path: &str, mut reader: UploadReader) -> Result<u64> {
        let key = self.object_key(object_path)?;

        let temp_file =
            tempfile::NamedTempFile::new().context("creating temporary file for S3 upload")?;
        let temp_path = temp_file.path().to_owned();

        let mut file = tokio::fs::File::create(&temp_path).await.with_context(|| {
            format!("creating temporary S3 upload file {}", temp_path.display())
        })?;

        let written = tokio::io::copy(&mut reader, &mut file)
            .await
            .with_context(|| format!("spooling S3 upload into {}", temp_path.display()))?;

        file.flush().await.with_context(|| {
            format!("flushing temporary S3 upload file {}", temp_path.display())
        })?;

        file.sync_data()
            .await
            .with_context(|| format!("syncing temporary S3 upload file {}", temp_path.display()))?;

        drop(file);

        let body = ByteStream::read_from()
            .path(&temp_path)
            .build()
            .await
            .with_context(|| format!("opening temporary S3 upload file {}", temp_path.display()))?;

        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(&key)
            .body(body)
            .send()
            .await
            .with_context(|| format!("streaming S3 object s3://{}/{}", self.bucket, key))?;

        Ok(written)
    }

    async fn delete(&self, object_path: &str) -> Result<()> {
        let key = self.object_key(object_path)?;

        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
            .with_context(|| format!("deleting S3 object s3://{}/{}", self.bucket, key))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::body::Bytes as AxumBytes;
    use axum::extract::{Path, State};
    use axum::http::{HeaderMap, HeaderValue, StatusCode};
    use axum::response::{IntoResponse, Response};
    use axum::routing::put;
    use bytes::Bytes;
    use tokio::net::TcpListener;

    use super::*;

    #[derive(Clone, Default)]
    struct FakeS3State {
        objects: Arc<Mutex<HashMap<String, Bytes>>>,
    }

    struct FakeS3Server {
        endpoint: String,
        bucket: String,
        state: FakeS3State,
    }

    impl FakeS3Server {
        async fn start() -> Self {
            let state = FakeS3State::default();

            let app = Router::new()
                .route(
                    "/{bucket}/{*key}",
                    put(put_object)
                        .get(get_object)
                        .head(head_object)
                        .delete(delete_object),
                )
                .with_state(state.clone());

            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();

            tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });

            Self {
                endpoint: format!("http://{addr}"),
                bucket: "test-bucket".to_owned(),
                state,
            }
        }

        fn storage(&self) -> S3Storage {
            S3Storage::new(S3StorageConfig {
                endpoint: self.endpoint.clone(),
                bucket: self.bucket.clone(),
                region: "us-east-1".to_owned(),
                access_key_id: "test-access-key".to_owned(),
                secret_access_key: "test-secret-key".to_owned(),
                force_path_style: true,
                prefix: None,
            })
            .unwrap()
        }

        fn storage_with_prefix(&self, prefix: impl Into<String>) -> S3Storage {
            S3Storage::new(S3StorageConfig {
                endpoint: self.endpoint.clone(),
                bucket: self.bucket.clone(),
                region: "us-east-1".to_owned(),
                access_key_id: "test-access-key".to_owned(),
                secret_access_key: "test-secret-key".to_owned(),
                force_path_style: true,
                prefix: Some(prefix.into()),
            })
            .unwrap()
        }

        fn stored(&self, key: &str) -> Option<Bytes> {
            self.state.objects.lock().unwrap().get(key).cloned()
        }
    }

    fn full_key(bucket: &str, key: &str) -> String {
        format!("{bucket}/{key}")
    }

    async fn put_object(
        State(state): State<FakeS3State>,
        Path((bucket, key)): Path<(String, String)>,
        body: AxumBytes,
    ) -> impl IntoResponse {
        state
            .objects
            .lock()
            .unwrap()
            .insert(full_key(&bucket, &key), body);

        StatusCode::OK
    }

    async fn get_object(
        State(state): State<FakeS3State>,
        Path((bucket, key)): Path<(String, String)>,
    ) -> Response {
        match state.objects.lock().unwrap().get(&full_key(&bucket, &key)) {
            Some(bytes) => {
                let mut headers = HeaderMap::new();
                headers.insert(
                    axum::http::header::CONTENT_LENGTH,
                    HeaderValue::from_str(&bytes.len().to_string()).unwrap(),
                );
                (StatusCode::OK, headers, bytes.clone()).into_response()
            }
            None => s3_not_found_response(),
        }
    }

    async fn head_object(
        State(state): State<FakeS3State>,
        Path((bucket, key)): Path<(String, String)>,
    ) -> Response {
        match state.objects.lock().unwrap().get(&full_key(&bucket, &key)) {
            Some(bytes) => {
                let mut headers = HeaderMap::new();
                headers.insert(
                    axum::http::header::CONTENT_LENGTH,
                    HeaderValue::from_str(&bytes.len().to_string()).unwrap(),
                );
                (StatusCode::OK, headers).into_response()
            }
            None => StatusCode::NOT_FOUND.into_response(),
        }
    }

    async fn delete_object(
        State(state): State<FakeS3State>,
        Path((bucket, key)): Path<(String, String)>,
    ) -> impl IntoResponse {
        state
            .objects
            .lock()
            .unwrap()
            .remove(&full_key(&bucket, &key));
        StatusCode::NO_CONTENT
    }

    fn s3_not_found_response() -> Response {
        (
            StatusCode::NOT_FOUND,
            [(
                axum::http::header::CONTENT_TYPE,
                HeaderValue::from_static("application/xml"),
            )],
            r#"<?xml version="1.0" encoding="UTF-8"?><Error><Code>NoSuchKey</Code><Message>The specified key does not exist.</Message></Error>"#,
        )
            .into_response()
    }

    #[tokio::test]
    async fn contains_returns_false_for_missing_object() {
        let server = FakeS3Server::start().await;
        let storage = server.storage();

        assert!(!storage.contains("nar/missing.nar").await.unwrap());
    }

    #[tokio::test]
    async fn put_bytes_then_contains_returns_true() {
        let server = FakeS3Server::start().await;
        let storage = server.storage();

        storage
            .put_bytes("nar/object.nar", BlobBytes::from_static(b"hello"))
            .await
            .unwrap();

        assert!(storage.contains("nar/object.nar").await.unwrap());
    }

    #[tokio::test]
    async fn put_bytes_then_get_bytes_returns_object() {
        let server = FakeS3Server::start().await;
        let storage = server.storage();

        storage
            .put_bytes("nar/object.nar", BlobBytes::from_static(b"hello"))
            .await
            .unwrap();

        let bytes = storage.get_bytes("nar/object.nar").await.unwrap().unwrap();

        assert_eq!(bytes, BlobBytes::from_static(b"hello"));
    }

    #[tokio::test]
    async fn get_bytes_returns_none_for_missing_object() {
        let server = FakeS3Server::start().await;
        let storage = server.storage();

        assert!(
            storage
                .get_bytes("nar/missing.nar")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn put_stream_writes_object_and_returns_size() {
        let server = FakeS3Server::start().await;
        let storage = server.storage();

        let reader: UploadReader =
            Box::pin(std::io::Cursor::new(Bytes::from_static(b"streamed-object")));

        let written = storage
            .put_stream("nar/streamed.nar", reader)
            .await
            .unwrap();

        assert_eq!(written, 15);

        let bytes = storage
            .get_bytes("nar/streamed.nar")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(bytes, BlobBytes::from_static(b"streamed-object"));
    }

    #[tokio::test]
    async fn delete_removes_object() {
        let server = FakeS3Server::start().await;
        let storage = server.storage();

        storage
            .put_bytes("nar/object.nar", BlobBytes::from_static(b"hello"))
            .await
            .unwrap();

        storage.delete("nar/object.nar").await.unwrap();

        assert!(!storage.contains("nar/object.nar").await.unwrap());
        assert!(storage.get_bytes("nar/object.nar").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn delete_missing_object_succeeds() {
        let server = FakeS3Server::start().await;
        let storage = server.storage();

        storage.delete("nar/missing.nar").await.unwrap();
    }

    #[tokio::test]
    async fn prefix_is_applied_to_object_keys() {
        let server = FakeS3Server::start().await;
        let storage = server.storage_with_prefix("/cache-objects/");

        storage
            .put_bytes("nar/object.nar", BlobBytes::from_static(b"hello"))
            .await
            .unwrap();

        assert_eq!(
            server.stored("test-bucket/cache-objects/nar/object.nar"),
            Some(Bytes::from_static(b"hello"))
        );
    }

    #[tokio::test]
    async fn invalid_object_paths_are_rejected() {
        let server = FakeS3Server::start().await;
        let storage = server.storage();

        assert!(storage.contains("").await.is_err());
        assert!(storage.contains("/absolute").await.is_err());
        assert!(storage.contains("../escape").await.is_err());
        assert!(storage.contains("nar/../escape").await.is_err());
        assert!(storage.contains("nar//object").await.is_err());
        assert!(storage.contains("nar/./object").await.is_err());
    }

    #[test]
    fn invalid_prefixes_are_rejected() {
        assert!(normalize_prefix(Some("../escape".to_owned())).is_err());
        assert!(normalize_prefix(Some("cache/../escape".to_owned())).is_err());
        assert_eq!(
            normalize_prefix(Some("/cache/objects/".to_owned())).unwrap(),
            Some("cache/objects".to_owned())
        );
        assert_eq!(normalize_prefix(Some("/".to_owned())).unwrap(), None);
    }
}
