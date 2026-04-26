use anyhow::{Context as _, Result};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use cache_core::nix::StorePathHash;
use cache_core::storage::{PathObjectKind, StorageId};
use cache_core::view::CacheView;
use cache_store::blob::BlobMetadata;

use crate::models::StorageObjectLookupRow;
use crate::pool::SqliteDatabase;

#[derive(Debug, Clone)]
pub struct StorageObjectRecord {
    pub storage_id: StorageId,
    pub metadata: BlobMetadata,
}

impl StorageObjectLookupRow {
    pub fn into_record(self) -> Result<StorageObjectRecord> {
        let last_modified = self
            .last_modified
            .as_deref()
            .map(|value| OffsetDateTime::parse(value, &Rfc3339))
            .transpose()
            .context("parsing local object last_modified")?;

        Ok(StorageObjectRecord {
            storage_id: StorageId::new(self.storage_id)
                .map_err(anyhow::Error::new)
                .context("parsing storage object storage_id")?,
            metadata: BlobMetadata::new(
                self.content_type,
                self.content_length
                    .map(u64::try_from)
                    .transpose()
                    .context("converting content_length to u64")?,
                self.etag,
                last_modified,
            ),
        })
    }
}

impl SqliteDatabase {
    pub async fn upsert_storage_object(
        &self,
        storage_id: &StorageId,
        object_path: &str,
        metadata: &BlobMetadata,
    ) -> Result<()> {
        let storage_id_text = storage_id.as_str();
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
            INSERT INTO storage_objects (
                storage_id,
                object_path,
                content_type,
                content_length,
                etag,
                last_modified
            )
            VALUES (?, ?, ?, ?, ?, ?)
            ON CONFLICT(storage_id, object_path) DO UPDATE SET
                content_type = excluded.content_type,
                content_length = excluded.content_length,
                etag = excluded.etag,
                last_modified = excluded.last_modified,
                deleted_at = NULL,
                first_deleted_at = NULL
            "#,
            storage_id_text,
            object_path,
            content_type,
            content_length,
            etag,
            last_modified,
        )
        .execute(&self.pool)
        .await
        .context("upserting local object")?;

        Ok(())
    }

    pub async fn list_storage_objects(
        &self,
        object_path: &str,
    ) -> Result<Vec<StorageObjectRecord>> {
        let rows = sqlx::query_as!(
            StorageObjectLookupRow,
            r#"
            SELECT
                storage_id,
                content_type,
                content_length,
                etag,
                last_modified
            FROM storage_objects
            WHERE object_path = ?
              AND deleted_at IS NULL
            ORDER BY storage_id ASC
            "#,
            object_path,
        )
        .fetch_all(&self.pool)
        .await
        .context("listing storage objects")?;

        rows.into_iter()
            .map(StorageObjectLookupRow::into_record)
            .collect()
    }

    pub async fn storage_object_visible_in_view(
        &self,
        view: &CacheView,
        object_path: &str,
    ) -> Result<bool> {
        let value = match view {
            CacheView::Aggregate => sqlx::query!(
                r#"
                SELECT 1 AS "present!: i64"
                FROM path_objects po
                JOIN aggregate_visible_paths avp
                    ON avp.store_path_hash = po.store_path_hash
                WHERE po.object_path = ?
                LIMIT 1
                "#,
                object_path,
            )
            .fetch_optional(&self.pool)
            .await
            .context("checking aggregate object visibility")?
            .is_some(),
            CacheView::Project(project) => {
                let project_slug = project.as_str();
                sqlx::query!(
                    r#"
                SELECT 1 AS "present!: i64"
                FROM path_objects po
                JOIN project_visible_paths pvp
                    ON pvp.store_path_hash = po.store_path_hash
                JOIN projects p
                    ON p.id = pvp.project_id
                WHERE p.slug = ?
                  AND po.object_path = ?
                LIMIT 1
                "#,
                    project_slug,
                    object_path,
                )
                .fetch_optional(&self.pool)
                .await
                .context("checking project object visibility")?
                .is_some()
            }
        };

        Ok(value)
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
}
