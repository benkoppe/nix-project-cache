use anyhow::{Context as _, Result};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use cache_core::nix::StorePathHash;
use cache_core::storage::{LocalBackendName, PathObjectKind};
use cache_store::blob::BlobMetadata;

use crate::models::LocalObjectLookupRow;
use crate::pool::SqliteDatabase;

#[derive(Debug, Clone)]
pub struct LocalObjectRecord {
    pub metadata: BlobMetadata,
    pub storage_backend: LocalBackendName,
    pub storage_key: String,
}

impl LocalObjectLookupRow {
    pub fn into_record(self) -> Result<LocalObjectRecord> {
        let last_modified = self
            .last_modified
            .as_deref()
            .map(|value| OffsetDateTime::parse(value, &Rfc3339))
            .transpose()
            .context("parsing local object last_modified")?;

        Ok(LocalObjectRecord {
            metadata: BlobMetadata::new(
                self.content_type,
                self.content_length
                    .map(u64::try_from)
                    .transpose()
                    .context("converting context_length to u64")?,
                self.etag,
                last_modified,
            ),
            storage_backend: LocalBackendName::new(self.storage_backend)
                .map_err(anyhow::Error::new)
                .context("parsing local object storage_backend")?,
            storage_key: self.storage_key,
        })
    }
}

impl SqliteDatabase {
    pub async fn upsert_local_object(
        &self,
        object_path: &str,
        metadata: &BlobMetadata,
        storage_backend: &LocalBackendName,
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
        let storage_backend_text = storage_backend.as_str();

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
                storage_key = excluded.storage_key,
                deleted_at = NULL,
                first_deleted_at = NULL
            "#,
            object_path,
            content_type,
            content_length,
            etag,
            last_modified,
            storage_backend_text,
            storage_key,
        )
        .execute(&self.pool)
        .await
        .context("upserting local object")?;

        Ok(())
    }

    pub async fn get_local_object(&self, object_path: &str) -> Result<Option<LocalObjectRecord>> {
        let row = sqlx::query_as!(
            LocalObjectLookupRow,
            r#"
            SELECT
                content_type,
                content_length,
                etag,
                last_modified,
                storage_backend,
                storage_key
            FROM local_objects
            WHERE object_path = ?
            LIMIT 1
            "#,
            object_path,
        )
        .fetch_optional(&self.pool)
        .await
        .context("fetching local object")?;

        row.map(LocalObjectLookupRow::into_record).transpose()
    }

    pub async fn link_path_object(
        &self,
        store_path_hash: &StorePathHash,
        object_path: &str,
        kind: PathObjectKind,
    ) -> Result<()> {
        let store_path_hash = store_path_hash.as_str();
        let kind_text = kind.as_str();

        sqlx::query!(
            r#"
            INSERT OR IGNORE INTO path_objects (store_path_hash, object_path, kind)
            VALUES (?, ?, ?)
            "#,
            store_path_hash,
            object_path,
            kind_text,
        )
        .execute(&self.pool)
        .await
        .context("linking path object")?;

        Ok(())
    }

    pub async fn path_has_object(
        &self,
        store_path_hash: &StorePathHash,
        object_path: &str,
        kind: PathObjectKind,
    ) -> Result<bool> {
        let store_path_hash_text = store_path_hash.as_str();
        let kind_text = kind.as_str();

        let row = sqlx::query!(
            r#"
            SELECT 1 AS "present!: i64"
            FROM path_objects
            WHERE store_path_hash = ?
              AND object_path = ?
              AND kind = ?
            LIMIT 1
            "#,
            store_path_hash_text,
            object_path,
            kind_text,
        )
        .fetch_optional(&self.pool)
        .await
        .context("checking path object link")?;

        Ok(row.is_some())
    }

    pub async fn delete_local_object(&self, object_path: &str) -> Result<()> {
        sqlx::query!(
            r#"
            DELETE FROM local_objects
            WHERE object_path = ?
            "#,
            object_path,
        )
        .execute(&self.pool)
        .await
        .context("deleting local object")?;

        Ok(())
    }
}
