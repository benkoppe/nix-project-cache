use anyhow::{Context as _, Result};
use uuid::Uuid;

use depot_core::narinfo::NarInfo;
use depot_core::nix::{NixContentAddress, NixHash, StorePathHash};
use depot_core::project::ProjectSlug;

use crate::models::{PathInfoLookupRow, PathReferenceValueRow, PathSignatureValueRow};
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

        let result = sqlx::query!(
            r#"
            INSERT OR IGNORE INTO path_infos (
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
        .context("inserting path_info row")?;

        if result.rows_affected() == 0 {
            tx.commit()
                .await
                .context("committing path_info validation transaction")?;

            let existing = self
                .get_path_info_by_hash(&store_path_hash)
                .await?
                .ok_or_else(|| anyhow::anyhow!("path_info disappeared during validation"))?;

            ensure_compatible_path_info(&existing, narinfo)?;
            return Ok(());
        }

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
                ordinal_i64
            )
            .execute(&mut *tx)
            .await
            .context("inserting path reference")?;
        }

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

    pub async fn get_project_narinfo(
        &self,
        project_slug: &ProjectSlug,
        store_path_hash: &StorePathHash,
    ) -> Result<Option<NarInfo>> {
        let project_slug_text = project_slug.as_str();
        let store_path_hash_text = store_path_hash.as_str();

        let row = sqlx::query_as!(
            PathInfoLookupRow,
            r#"
            SELECT 
                pi.store_path_hash,
                pi.store_path,
                pi.url,
                pi.compression,
                pi.nar_hash,
                pi.nar_size,
                pi.deriver,
                pi.ca
            FROM path_infos pi
            JOIN project_visible_paths pvp ON pvp.store_path_hash = pi.store_path_hash
            JOIN projects p ON p.id = pvp.project_id
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
            PathInfoLookupRow,
            r#"
            SELECT
                pi.store_path_hash,
                pi.store_path,
                pi.url,
                pi.compression,
                pi.nar_hash,
                pi.nar_size,
                pi.deriver,
                pi.ca
            FROM path_infos pi
            WHERE pi.store_path_hash = ?
                AND EXISTS (
                    SELECT 1
                    FROM aggregate_visible_paths avp
                    WHERE avp.store_path_hash = pi.store_path_hash
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

    pub async fn get_build_path_narinfo(
        &self,
        build_id: Uuid,
        store_path_hash: &StorePathHash,
    ) -> Result<Option<NarInfo>> {
        let build_id_text = build_id.to_string();
        let store_path_hash_text = store_path_hash.as_str();

        let row = sqlx::query_as!(
            PathInfoLookupRow,
            r#"
             SELECT
                pi.store_path_hash,
                pi.store_path,
                pi.url,
                pi.compression,
                pi.nar_hash,
                pi.nar_size,
                pi.deriver,
                pi.ca
            FROM build_paths bp
            JOIN path_infos pi ON pi.store_path_hash = bp.store_path_hash
            WHERE bp.build_id = ?
            AND bp.store_path_hash = ?
            LIMIT 1
            "#,
            build_id_text,
            store_path_hash_text
        )
        .fetch_optional(&self.pool)
        .await
        .context("fetching build path narinfo")?;

        match row {
            Some(row) => self.inflate_narinfo(row).await.map(Some),
            None => Ok(None),
        }
    }

    pub async fn get_path_info_by_hash(
        &self,
        store_path_hash: &StorePathHash,
    ) -> Result<Option<NarInfo>> {
        let store_path_hash_text = store_path_hash.as_str();

        let row = sqlx::query_as!(
            PathInfoLookupRow,
            r#"
            SELECT
                store_path_hash,
                store_path,
                url,
                compression,
                nar_hash,
                nar_size,
                deriver,
                ca
            FROM path_infos
            WHERE store_path_hash = ?
            LIMIT 1
            "#,
            store_path_hash_text
        )
        .fetch_optional(&self.pool)
        .await
        .context("fetching path_info by hash")?;

        match row {
            Some(row) => self.inflate_narinfo(row).await.map(Some),
            None => Ok(None),
        }
    }

    async fn inflate_narinfo(&self, path_info: PathInfoLookupRow) -> Result<NarInfo> {
        let store_path_hash_text = path_info.store_path_hash.as_str();

        let references = sqlx::query_as!(
            PathReferenceValueRow,
            r#"
            SELECT
                reference_store_path
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
            PathSignatureValueRow,
            r#"
            SELECT
                signature
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

fn ensure_compatible_path_info(existing: &NarInfo, incoming: &NarInfo) -> Result<()> {
    let existing_ca = existing
        .ca
        .as_ref()
        .map(NixContentAddress::format_for_narinfo);
    let incoming_ca = incoming
        .ca
        .as_ref()
        .map(NixContentAddress::format_for_narinfo);

    let mut existing_refs = existing.references.clone();
    let mut incoming_refs = incoming.references.clone();
    existing_refs.sort();
    incoming_refs.sort();

    let compatible = existing.store_path == incoming.store_path
        && existing.url == incoming.url
        && existing.compression == incoming.compression
        && existing.normalized_nar_hash()? == incoming.normalized_nar_hash()?
        && existing.nar_size == incoming.nar_size
        && existing_refs == incoming_refs
        && existing.deriver == incoming.deriver
        && existing_ca == incoming_ca;

    if compatible {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "conflicting narinfo metadata for store path {}",
            incoming.store_path
        ))
    }
}
