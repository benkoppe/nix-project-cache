use anyhow::{Context as _, Result};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use cache_store::blob::BlobMetadata;

use crate::models::LocalObjectRow;
use crate::pool::SqliteDatabase;

#[derive(Debug, Clone)]
pub struct LocalObjectRecord {
    pub metadata: BlobMetadata,
    pub storage_backend: String,
    pub storage_key: String,
}

impl SqliteDatabase {
    pub async fn upsert_local_object(
        &self,
        object_path: &str,
        metadata: &BlobMetadata,
        storage_backend: &str,
        storage_key: &str,
    ) -> Result<()> {
        let content_type = metadata.content_type.as_str();
        let content_length = metadata
            .content_length
            .map(i64::try_from)
            .transpose()
            .context("converting content_length to i64")?;
        let etag = metadata.etag.as_deref();
        let last_modified = metadata
            .last_modified
            .map(|value| value.format(&Rfc3339))
            .transpose()
            .context("formatting last_modified")?;

        sqlx::query!(
            r#"
            INSERT INTO local_objects (
                object_path,
                content_type,
                content_length,
                etag,
                last_modified,
                storage_backend,
                storage_key
            )
            VALUES (?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(object_path) DO UPDATE SET
                content_type = excluded.content_type,
                content_length = excluded.content_length,
                etag = excluded.etag,
                last_modified = excluded.last_modified,
                storage_backend = excluded.storage_backend,
                storage_key = excluded.storage_key
            "#,
            object_path,
            content_type,
            content_length,
            etag,
            last_modified,
            storage_backend,
            storage_key,
        )
        .execute(&self.pool)
        .await
        .context("upserting local object")?;

        Ok(())
    }

    pub async fn get_local_object(&self, object_path: &str) -> Result<Option<LocalObjectRecord>> {
        let row = sqlx::query_as!(
            LocalObjectRow,
            r#"
            SELECT
                object_path,
                content_type,
                content_length,
                etag,
                last_modified,
                storage_backend,
                storage_key,
                created_at
            FROM local_objects
            WHERE object_path = ?
            LIMIT 1
            "#,
            object_path,
        )
        .fetch_optional(&self.pool)
        .await
        .context("fetching local object")?;

        row.map(row_to_local_object_record).transpose()
    }
}

fn row_to_local_object_record(row: LocalObjectRow) -> Result<LocalObjectRecord> {
    let last_modified = row
        .last_modified
        .as_deref()
        .map(|value| OffsetDateTime::parse(value, &Rfc3339))
        .transpose()
        .context("parsing local object last_modified")?;

    Ok(LocalObjectRecord {
        metadata: BlobMetadata::new(
            row.content_type,
            row.content_length
                .map(u64::try_from)
                .transpose()
                .context("converting context_length to u64")?,
            row.etag,
            last_modified,
        ),
        storage_backend: row.storage_backend,
        storage_key: row.storage_key,
    })
}
