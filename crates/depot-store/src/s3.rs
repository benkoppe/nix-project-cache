use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use async_trait::async_trait;
use aws_credential_types::Credentials as AwsCredentials;
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{
    Builder as S3ConfigBuilder, Region, RequestChecksumCalculation, ResponseChecksumValidation,
};
use aws_sdk_s3::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_s3::presigning::PresigningConfig;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{CompletedMultipartUpload as AwsCompletedMultipartUpload, CompletedPart};
use bytes::Bytes;
use time::OffsetDateTime;
use tokio::io::AsyncReadExt as _;

use crate::blob::{BlobBytes, BlobMetadata};
use crate::local::{
    CacheStorage, CompletedMultipartUpload, CompletedMultipartUploadPart, MultipartUpload,
    PresignedUploadPartUrl, UploadReader,
};

const PRESIGNED_MULTIPART_PART_TTL_SKEW_SECONDS: i64 = 0;
const MIN_MULTIPART_PART_SIZE: usize = 5 * 1024 * 1024;
const DEFAULT_MULTIPART_PART_SIZE: usize = 16 * 1024 * 1024;
const MAX_MULTIPART_PARTS: i32 = 10_000;

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
        let credentials = AwsCredentials::new(
            config.access_key_id.clone(),
            config.secret_access_key.clone(),
            None,
            None,
            "depot-store-s3",
        );

        let s3_config = S3ConfigBuilder::new()
            .behavior_version_latest()
            .region(Region::new(config.region.clone()))
            .credentials_provider(credentials)
            .endpoint_url(config.endpoint.clone())
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

    async fn create_multipart_upload_for_key(&self, key: &str) -> Result<MultipartUpload> {
        let response = self
            .client
            .create_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .content_type("application/octet-stream")
            .send()
            .await
            .with_context(|| {
                format!("creating multipart upload for s3://{}/{}", self.bucket, key)
            })?;

        let upload_id = response
            .upload_id()
            .ok_or_else(|| anyhow!("S3 create multipart upload response missing upload id"))?
            .to_owned();

        Ok(MultipartUpload {
            upload_id,
            part_size: DEFAULT_MULTIPART_PART_SIZE as u64,
        })
    }

    async fn presign_multipart_upload_part_for_key(
        &self,
        key: &str,
        upload_id: &str,
        part_number: i32,
        content_length: u64,
        expires_in: Duration,
    ) -> Result<PresignedUploadPartUrl> {
        validate_part_number(part_number)?;

        let presigning_config = PresigningConfig::expires_in(expires_in)
            .context("building S3 upload-part presigning config")?;

        let request = self
            .client
            .upload_part()
            .bucket(&self.bucket)
            .key(key)
            .upload_id(upload_id)
            .part_number(part_number)
            .content_length(
                content_length
                    .try_into()
                    .context("converting content length")?,
            )
            .presigned(presigning_config)
            .await
            .with_context(|| {
                format!(
                    "presigning multipart upload part {} for s3://{}/{}",
                    part_number, self.bucket, key
                )
            })?;

        let ttl = time::Duration::try_from(expires_in)
            .context("converting presigned upload-part ttl")?
            - time::Duration::seconds(PRESIGNED_MULTIPART_PART_TTL_SKEW_SECONDS);

        Ok(PresignedUploadPartUrl {
            url: request.uri().to_string(),
            expires_at: OffsetDateTime::now_utc() + ttl,
        })
    }

    async fn complete_multipart_upload_for_key(
        &self,
        key: &str,
        upload_id: &str,
        parts: Vec<CompletedMultipartUploadPart>,
        content_length: u64,
    ) -> Result<CompletedMultipartUpload> {
        if parts.is_empty() {
            return Err(anyhow!("cannot complete multipart upload without parts"));
        }

        let completed_parts = parts
            .into_iter()
            .map(|part| {
                validate_part_number(part.part_number)?;

                Ok(CompletedPart::builder()
                    .part_number(part.part_number)
                    .e_tag(part.etag)
                    .build())
            })
            .collect::<Result<Vec<_>>>()?;

        let multipart_upload = AwsCompletedMultipartUpload::builder()
            .set_parts(Some(completed_parts))
            .build();

        let response = self
            .client
            .complete_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .upload_id(upload_id)
            .multipart_upload(multipart_upload)
            .send()
            .await
            .with_context(|| {
                format!(
                    "completing multipart upload for s3://{}/{}",
                    self.bucket, key
                )
            })?;

        Ok(CompletedMultipartUpload {
            content_length,
            e_tag: response.e_tag().map(str::to_owned),
        })
    }

    async fn abort_multipart_upload_for_key(&self, key: &str, upload_id: &str) -> Result<()> {
        self.client
            .abort_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .upload_id(upload_id)
            .send()
            .await
            .with_context(|| {
                format!("aborting multipart upload for s3://{}/{}", self.bucket, key)
            })?;

        Ok(())
    }

    async fn put_stream_multipart(&self, key: &str, mut reader: UploadReader) -> Result<u64> {
        let MultipartUpload {
            upload_id,
            part_size,
        } = self.create_multipart_upload_for_key(key).await?;

        let part_size = usize::try_from(part_size).context("converting part size")?;

        if part_size < MIN_MULTIPART_PART_SIZE {
            return Err(anyhow!("multipart part size must be at least 5 MiB"));
        }

        let result = async {
            let mut part_number = 1;
            let mut total_content_length = 0_u64;
            let mut completed_parts = Vec::new();

            while let Some(part_bytes) = read_next_part(&mut reader, part_size).await? {
                validate_part_number(part_number)?;

                let part_len =
                    u64::try_from(part_bytes.len()).context("converting multipart part length")?;
                total_content_length += part_len;

                let response = self
                    .client
                    .upload_part()
                    .bucket(&self.bucket)
                    .key(key)
                    .upload_id(&upload_id)
                    .part_number(part_number)
                    .content_length(part_len.try_into().context("converting part length")?)
                    .body(ByteStream::from(part_bytes))
                    .send()
                    .await
                    .with_context(|| {
                        format!(
                            "uploading multipart part {} for s3://{}/{}",
                            part_number, self.bucket, key
                        )
                    })?;

                let etag = response
                    .e_tag()
                    .ok_or_else(|| anyhow!("S3 upload-part response missing ETag"))?
                    .to_owned();

                completed_parts.push(CompletedMultipartUploadPart { part_number, etag });
                part_number += 1;
            }

            if completed_parts.is_empty() {
                return Err(anyhow!("cannot upload empty stream as multipart object"));
            }

            self.complete_multipart_upload_for_key(
                key,
                &upload_id,
                completed_parts,
                total_content_length,
            )
            .await?;

            Ok(total_content_length)
        }
        .await;

        if result.is_err()
            && let Err(error) = self.abort_multipart_upload_for_key(key, &upload_id).await
        {
            tracing::warn!(
                ?error,
                bucket = %self.bucket,
                key,
                upload_id,
                "failed to abort multipart upload after upload failure"
            );
        }

        result
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

fn validate_part_number(part_number: i32) -> Result<()> {
    if !(1..=MAX_MULTIPART_PARTS).contains(&part_number) {
        return Err(anyhow!(
            "invalid multipart upload part number {part_number}"
        ));
    }

    Ok(())
}

async fn read_next_part(reader: &mut UploadReader, part_size: usize) -> Result<Option<Bytes>> {
    let mut buffer = Vec::with_capacity(part_size);
    let mut chunk = vec![0_u8; 64 * 1024];

    while buffer.len() < part_size {
        let remaining = part_size - buffer.len();
        let read_size = remaining.min(chunk.len());

        let bytes_read = reader
            .read(&mut chunk[..read_size])
            .await
            .context("reading multipart upload part")?;

        if bytes_read == 0 {
            break;
        }

        buffer.extend_from_slice(&chunk[..bytes_read]);
    }

    if buffer.is_empty() {
        Ok(None)
    } else {
        Ok(Some(Bytes::from(buffer)))
    }
}

#[async_trait]
impl CacheStorage for S3Storage {
    async fn head(&self, object_path: &str) -> Result<Option<BlobMetadata>> {
        let key = self.object_key(object_path)?;

        let response = match self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) if is_not_found(&error) => return Ok(None),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("checking s3 object s3://{}/{}", self.bucket, key));
            }
        };

        let content_length = response
            .content_length()
            .and_then(|value| u64::try_from(value).ok());

        Ok(Some(BlobMetadata::new(
            response
                .content_type()
                .unwrap_or("application/octet-stream")
                .to_owned(),
            content_length,
            response.e_tag().map(str::to_owned),
            None,
        )))
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

    async fn put_stream(&self, object_path: &str, reader: UploadReader) -> Result<u64> {
        let key = self.object_key(object_path)?;
        self.put_stream_multipart(&key, reader).await
    }

    async fn create_multipart_upload(&self, object_path: &str) -> Result<Option<MultipartUpload>> {
        let key = self.object_key(object_path)?;
        self.create_multipart_upload_for_key(&key).await.map(Some)
    }

    async fn presign_multipart_upload_part(
        &self,
        object_path: &str,
        upload_id: &str,
        part_number: i32,
        content_length: u64,
        expires_in: Duration,
    ) -> Result<Option<PresignedUploadPartUrl>> {
        let key = self.object_key(object_path)?;

        self.presign_multipart_upload_part_for_key(
            &key,
            upload_id,
            part_number,
            content_length,
            expires_in,
        )
        .await
        .map(Some)
    }

    async fn complete_multipart_upload(
        &self,
        object_path: &str,
        upload_id: &str,
        parts: Vec<CompletedMultipartUploadPart>,
        content_length: u64,
    ) -> Result<CompletedMultipartUpload> {
        let key = self.object_key(object_path)?;

        self.complete_multipart_upload_for_key(&key, upload_id, parts, content_length)
            .await
    }

    async fn abort_multipart_upload(&self, object_path: &str, upload_id: &str) -> Result<()> {
        let key = self.object_key(object_path)?;
        self.abort_multipart_upload_for_key(&key, upload_id).await
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
    use std::collections::{BTreeMap, HashMap};
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::body::Bytes as AxumBytes;
    use axum::extract::{Path, Query, State};
    use axum::http::{HeaderMap, HeaderValue, StatusCode};
    use axum::response::{IntoResponse, Response};
    use axum::routing::put;
    use bytes::Bytes;
    use serde::Deserialize;
    use tokio::net::TcpListener;

    use super::*;

    #[derive(Clone, Default)]
    struct FakeS3State {
        objects: Arc<Mutex<HashMap<String, Bytes>>>,
        multipart_uploads: Arc<Mutex<HashMap<String, FakeMultipartUpload>>>,
        next_upload_id: Arc<Mutex<u64>>,
    }

    #[derive(Default)]
    struct FakeMultipartUpload {
        parts: BTreeMap<i32, Bytes>,
    }

    #[derive(Debug, Deserialize)]
    struct S3MultipartQuery {
        uploads: Option<String>,
        #[serde(rename = "uploadId")]
        upload_id: Option<String>,
        #[serde(rename = "partNumber")]
        part_number: Option<i32>,
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
                        .post(post_object)
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
        Query(query): Query<S3MultipartQuery>,
        body: AxumBytes,
    ) -> impl IntoResponse {
        match (query.upload_id.as_deref(), query.part_number) {
            (Some(upload_id), Some(part_number)) => {
                let mut uploads = state.multipart_uploads.lock().unwrap();
                let Some(upload) = uploads.get_mut(upload_id) else {
                    return StatusCode::NOT_FOUND.into_response();
                };

                upload.parts.insert(part_number, body.clone());

                let mut headers = HeaderMap::new();
                headers.insert(
                    axum::http::header::ETAG,
                    HeaderValue::from_str(&format!("\"part-{part_number}\"")).unwrap(),
                );

                (StatusCode::OK, headers).into_response()
            }
            _ => {
                state
                    .objects
                    .lock()
                    .unwrap()
                    .insert(full_key(&bucket, &key), body);

                StatusCode::OK.into_response()
            }
        }
    }

    async fn post_object(
        State(state): State<FakeS3State>,
        Path((bucket, key)): Path<(String, String)>,
        Query(query): Query<S3MultipartQuery>,
    ) -> Response {
        if query.uploads.is_some() {
            let upload_id = {
                let mut next_upload_id = state.next_upload_id.lock().unwrap();
                *next_upload_id += 1;
                (*next_upload_id).to_string()
            };

            state
                .multipart_uploads
                .lock()
                .unwrap()
                .insert(upload_id.clone(), FakeMultipartUpload::default());

            return (
            StatusCode::OK,
                [(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("application/xml"),
                )],
                format!(
                "<InitiateMultipartUploadResult><Bucket>{}</Bucket><Key>{}</Key><UploadId>{}</UploadId></InitiateMultipartUploadResult>",
                bucket, key, upload_id
            ),
        ).into_response();
        }

        let Some(upload_id) = query.upload_id.as_deref() else {
            return StatusCode::BAD_REQUEST.into_response();
        };

        let Some(upload) = state.multipart_uploads.lock().unwrap().remove(upload_id) else {
            return StatusCode::NOT_FOUND.into_response();
        };

        let mut object_bytes = Vec::new();
        for part in upload.parts.values() {
            object_bytes.extend_from_slice(part);
        }

        state
            .objects
            .lock()
            .unwrap()
            .insert(full_key(&bucket, &key), Bytes::from(object_bytes));

        (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/xml"),
        )],
        r#"<CompleteMultipartUploadResult><ETag>"complete-etag"</ETag></CompleteMultipartUploadResult>"#,
    )
        .into_response()
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
        Query(query): Query<S3MultipartQuery>,
    ) -> impl IntoResponse {
        if let Some(upload_id) = query.upload_id.as_deref() {
            state.multipart_uploads.lock().unwrap().remove(upload_id);
            return StatusCode::NO_CONTENT;
        }

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
    async fn put_stream_writes_object_with_multipart_and_returns_size() {
        let server = FakeS3Server::start().await;
        let storage = server.storage();

        let payload = Bytes::from_static(b"streamed-object");
        let reader: UploadReader = Box::pin(std::io::Cursor::new(payload.clone()));

        let written = storage
            .put_stream("nar/streamed.nar", reader)
            .await
            .unwrap();

        assert_eq!(written, 15);

        assert_eq!(server.stored("test-bucket/nar/streamed.nar"), Some(payload))
    }

    #[tokio::test]
    async fn presigned_multipart_part_uploads_to_prefixed_key() {
        let server = FakeS3Server::start().await;
        let storage = server.storage_with_prefix("/cache-objects/");

        let upload = storage
            .create_multipart_upload("nar/test.nar.zst")
            .await
            .unwrap()
            .expect("S3 storage should support multipart uploads");

        let body = Bytes::from_static(b"presigned-part");
        let content_length = u64::try_from(body.len()).unwrap();

        let presigned = storage
            .presign_multipart_upload_part(
                "nar/test.nar.zst",
                &upload.upload_id,
                1,
                content_length,
                std::time::Duration::from_secs(300),
            )
            .await
            .unwrap()
            .expect("S3 storage should support presigned multipart upload parts");

        let response = reqwest::Client::new()
            .put(&presigned.url)
            .header(reqwest::header::CONTENT_LENGTH, body.len())
            .body(body.clone())
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let etag = response
            .headers()
            .get(reqwest::header::ETAG)
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();

        storage
            .complete_multipart_upload(
                "nar/test.nar.zst",
                &upload.upload_id,
                vec![CompletedMultipartUploadPart {
                    part_number: 1,
                    etag,
                }],
                content_length,
            )
            .await
            .unwrap();

        assert_eq!(
            server.stored("test-bucket/cache-objects/nar/test.nar.zst"),
            Some(body)
        );
    }

    #[tokio::test]
    async fn multipart_completion_concatenates_parts_in_part_number_order() {
        let server = FakeS3Server::start().await;
        let storage = server.storage();

        let upload = storage
            .create_multipart_upload("nar/ordered.nar.zst")
            .await
            .unwrap()
            .unwrap();

        let mut completed_parts = Vec::new();

        for (part_number, body) in [
            (2, Bytes::from_static(b"world")),
            (1, Bytes::from_static(b"hello ")),
        ] {
            let presigned = storage
                .presign_multipart_upload_part(
                    "nar/ordered.nar.zst",
                    &upload.upload_id,
                    part_number,
                    u64::try_from(body.len()).unwrap(),
                    std::time::Duration::from_secs(300),
                )
                .await
                .unwrap()
                .unwrap();

            let response = reqwest::Client::new()
                .put(&presigned.url)
                .header(reqwest::header::CONTENT_LENGTH, body.len())
                .body(body)
                .send()
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::OK);

            let etag = response
                .headers()
                .get(reqwest::header::ETAG)
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned();

            completed_parts.push(CompletedMultipartUploadPart { part_number, etag });
        }

        // Intentionally keep completion parts in upload order; the fake server stores parts
        // by part number and should assemble the object in ascending part-number order.
        storage
            .complete_multipart_upload(
                "nar/ordered.nar.zst",
                &upload.upload_id,
                completed_parts,
                11,
            )
            .await
            .unwrap();

        assert_eq!(
            server.stored("test-bucket/nar/ordered.nar.zst"),
            Some(Bytes::from_static(b"hello world"))
        );
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
