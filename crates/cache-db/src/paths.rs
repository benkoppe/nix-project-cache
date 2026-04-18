use anyhow::{Context as _, Result};

use cache_core::narinfo::NarInfo;
use cache_core::nix::{NixContentAddress, NixHash, StorePathHash};
use cache_core::project::ProjectSlug;

use crate::models::{PathInfoRow, PathReferenceRow, PathSignatureRow};
use crate::pool::SqliteDatabase;

impl SqliteDatabase {
    pub async fn upsert_path_info(&self, narinfo: &NarInfo) -> Result<()> {
        let store_path_hash = StorePathHash::parse_from_store_path(&narinfo.store_path)
            .context("deriving store_path_hash from NarInfo")?;

        let store_path_hash_text = store_path_hash.as_str();
        let store_path = narinfo.store_path.as_str();
        let url = narinfo.url.as_str();
        let compression = narinfo.compression.as_str();
        let nar_hash = narinfo.normalized_nar_hash()?.to_string();
        let nar_size = i64::try_from(narinfo.nar_size).context("converting nar_size to i64")?;
        let deriver = narinfo.deriver.as_deref();
        let ca = narinfo
            .ca
            .as_ref()
            .map(NixContentAddress::format_for_narinfo);

        let mut tx = self
            .pool
            .begin()
            .await
            .context("beginning path_info transaction")?;

        sqlx::query!(
            r#"
            INSERT INTO path_infos (
                store_path_hash,
                store_path,
                url,
                compression,
                nar_hash,
                nar_size,
                deriver,
                ca
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(store_path_hash) DO UPDATE SET
                store_path = excluded.store_path,
                url = excluded.url,
                compression = excluded.compression,
                nar_hash = excluded.nar_hash,
                nar_size = excluded.nar_size,
                deriver = excluded.deriver,
                ca = excluded.ca
            "#,
            store_path_hash_text,
            store_path,
            url,
            compression,
            nar_hash,
            nar_size,
            deriver,
            ca,
        )
        .execute(&mut *tx)
        .await
        .context("upserting path_info row")?;

        sqlx::query!(
            r#"
            DELETE FROM path_references
            WHERE store_path_hash = ?
            "#,
            store_path_hash_text,
        )
        .execute(&mut *tx)
        .await
        .context("clearing existing path references")?;

        for (ordinal, reference) in narinfo.references.iter().enumerate() {
            let ordinal_i64 = i64::try_from(ordinal).context("converting reference ordinal")?;
            let reference_text = reference.as_str();

            sqlx::query!(
                r#"
                INSERT INTO path_references (store_path_hash, reference_store_path, ordinal)
                VALUES (?, ?, ?)
                "#,
                store_path_hash_text,
                reference_text,
                ordinal_i64,
            )
            .execute(&mut *tx)
            .await
            .context("inserting path reference")?;
        }

        sqlx::query!(
            r#"
            DELETE FROM path_signatures
            WHERE store_path_hash = ?
            "#,
            store_path_hash_text
        )
        .execute(&mut *tx)
        .await
        .context("clearing existing path signatures")?;

        for (ordinal, signature) in narinfo.signatures.iter().enumerate() {
            let ordinal_i64 = i64::try_from(ordinal).context("converting signature ordinal")?;
            let signature_text = signature.as_str();

            sqlx::query!(
                r#"
                INSERT INTO path_signatures (store_path_hash, signature, ordinal)
                VALUES (?, ?, ?)
                "#,
                store_path_hash_text,
                signature_text,
                ordinal_i64,
            )
            .execute(&mut *tx)
            .await
            .context("inserting path signature")?;
        }

        tx.commit()
            .await
            .context("committing path_info transaction")?;

        Ok(())
    }

    pub async fn link_path_to_project(
        &self,
        project_slug: &ProjectSlug,
        store_path_hash: &StorePathHash,
    ) -> Result<()> {
        let project_id = self.project_id_by_slug(project_slug).await?;
        let store_path_hash_text = store_path_hash.as_str();

        sqlx::query!(
            r#"
            INSERT OR IGNORE INTO project_paths (project_id, store_path_hash)
            VALUES (?, ?)
            "#,
            project_id,
            store_path_hash_text,
        )
        .execute(&self.pool)
        .await
        .context("linking path to project")?;

        Ok(())
    }

    pub async fn get_project_narinfo(
        &self,
        project_slug: &ProjectSlug,
        store_path_hash: &StorePathHash,
    ) -> Result<Option<NarInfo>> {
        let project_slug_text = project_slug.as_str();
        let store_path_hash_text = store_path_hash.as_str();

        let row = sqlx::query_as!(
            PathInfoRow,
            r#"
            SELECT 
                pi.store_path_hash, 
                pi.store_path, 
                pi.url, 
                pi.compression, 
                pi.nar_hash, 
                pi.nar_size, 
                pi.deriver, 
                pi.ca, 
                pi.created_at
            FROM path_infos pi
            JOIN project_paths pp ON pp.store_path_hash = pi.store_path_hash
            JOIN projects p ON p.id = pp.project_id
            WHERE p.slug = ? AND pi.store_path_hash = ?
            LIMIT 1
            "#,
            project_slug_text,
            store_path_hash_text,
        )
        .fetch_optional(&self.pool)
        .await
        .context("fetching project narinfo")?;

        let Some(row) = row else {
            return Ok(None);
        };

        self.inflate_narinfo(row).await.map(Some)
    }

    pub async fn get_aggregate_narinfo(
        &self,
        store_path_hash: &StorePathHash,
    ) -> Result<Option<NarInfo>> {
        let store_path_hash_text = store_path_hash.as_str();

        let row = sqlx::query_as!(
            PathInfoRow,
            r#"
            SELECT
                pi.store_path_hash,
                pi.store_path,
                pi.url,
                pi.compression,
                pi.nar_hash,
                pi.nar_size,
                pi.deriver,
                pi.ca,
                pi.created_at
            FROM path_infos pi
            WHERE pi.store_path_hash = ?
                AND EXISTS (
                    SELECT 1
                    FROM project_paths pp
                    JOIN projects p ON p.id = pp.project_id
                    WHERE pp.store_path_hash = pi.store_path_hash
                        AND p.public = 1
                )
            LIMIT 1
            "#,
            store_path_hash_text,
        )
        .fetch_optional(&self.pool)
        .await
        .context("fetching aggregate narinfo")?;

        let Some(row) = row else {
            return Ok(None);
        };

        self.inflate_narinfo(row).await.map(Some)
    }

    async fn inflate_narinfo(&self, path_info: PathInfoRow) -> Result<NarInfo> {
        let store_path_hash_text = path_info.store_path_hash.as_str();

        let references = sqlx::query_as!(
            PathReferenceRow,
            r#"
            SELECT
                store_path_hash,
                reference_store_path,
                ordinal
            FROM path_references
            WHERE store_path_hash = ?
            ORDER BY ordinal ASC
            "#,
            store_path_hash_text,
        )
        .fetch_all(&self.pool)
        .await
        .context("loading path references")?;

        let signatures = sqlx::query_as!(
            PathSignatureRow,
            r#"
            SELECT
                store_path_hash,
                signature,
                ordinal
            FROM path_signatures
            WHERE store_path_hash = ?
            ORDER BY ordinal ASC
            "#,
            store_path_hash_text,
        )
        .fetch_all(&self.pool)
        .await
        .context("loading path signatures")?;

        Ok(NarInfo {
            store_path: path_info.store_path,
            url: path_info.url,
            compression: path_info.compression,
            nar_hash: NixHash::Raw(path_info.nar_hash),
            nar_size: u64::try_from(path_info.nar_size).context("converting nar_size to u64")?,
            references: references
                .into_iter()
                .map(|row| row.reference_store_path)
                .collect(),
            deriver: path_info.deriver,
            signatures: signatures.into_iter().map(|row| row.signature).collect(),
            ca: path_info.ca.map(NixContentAddress::Raw),
        })
    }
}
