use bytes::Bytes;
use time::OffsetDateTime;

pub type BlobBytes = Bytes;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobMetadata {
    pub content_type: String,
    pub content_length: Option<u64>,
    pub etag: Option<String>,
    pub last_modified: Option<OffsetDateTime>,
}

impl BlobMetadata {
    pub fn new(
        content_type: impl Into<String>,
        content_length: Option<u64>,
        etag: Option<String>,
        last_modified: Option<OffsetDateTime>,
    ) -> Self {
        Self {
            content_type: content_type.into(),
            content_length,
            etag,
            last_modified,
        }
    }
}
