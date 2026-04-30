use anyhow::{Context as _, Result, anyhow};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use depot_core::nix::StorePathHash;
use depot_core::project::ProjectSlug;

use crate::models::{
    BuildContextRecord, BuildContextRow, BuildLookupRow, BuildRecord, BuildStatus,
};
use crate::pool::SqliteDatabase;

impl SqliteDatabase {
    pub async fn begin_build(
        &self,
        project: &ProjectSlug,
        ref_name: &str,
        revision: Option<&str>,
    ) -> Result<BuildRecord> {
        let project_id = self.project_id_by_slug(project).await?;
        let build_id = Uuid::now_v7();
        let build_id_text = build_id.to_string();
        let status = BuildStatus::Pending.as_str();

        sqlx::query!(
            r#"
            INSERT INTO builds (id, project_id, ref_name, revision, status)
            VALUES (?, ?, ?, ?, ?)
            "#,
            build_id_text,
            project_id,
            ref_name,
            revision,
            status
        )
        .execute(&self.pool)
        .await
        .context("inserting build")?;

        self.get_build(build_id)
            .await?
            .ok_or_else(|| anyhow!("build {} not found after insert", build_id))
    }

    pub async fn get_build(&self, build_id: Uuid) -> Result<Option<BuildRecord>> {
        let build_id_text = build_id.to_string();

        let row = sqlx::query_as!(
            BuildLookupRow,
            r#"
            SELECT
                id,
                project_id,
                ref_name,
                revision,
                status,
                created_at,
                finalized_at
            FROM builds
            WHERE id = ?
            LIMIT 1
            "#,
            build_id_text
        )
        .fetch_optional(&self.pool)
        .await
        .context("fetching build")?;

        row.map(BuildLookupRow::into_record).transpose()
    }

    pub async fn get_build_context(&self, build_id: Uuid) -> Result<Option<BuildContextRecord>> {
        let build_id_text = build_id.to_string();

        let row = sqlx::query_as!(
            BuildContextRow,
            r#"
            SELECT
                b.id as build_id,
                p.id as project_id,
                p.slug as project_slug,
                b.ref_name,
                b.revision,
                b.status
            FROM builds b
            JOIN projects p ON p.id = b.project_id
            WHERE b.id = ?
            LIMIT 1
            "#,
            build_id_text
        )
        .fetch_optional(&self.pool)
        .await
        .context("fetching build context")?;

        row.map(BuildContextRow::into_record).transpose()
    }

    pub async fn attach_build_path(
        &self,
        build_id: Uuid,
        store_path_hash: &StorePathHash,
    ) -> Result<()> {
        let build_id_text = build_id.to_string();
        let store_path_hash_text = store_path_hash.as_str();

        sqlx::query!(
            r#"
            INSERT OR IGNORE INTO build_paths (build_id, store_path_hash)
            VALUES (?, ?)
            "#,
            build_id_text,
            store_path_hash_text,
        )
        .execute(&self.pool)
        .await
        .context("attaching build path")?;

        Ok(())
    }

    pub async fn attach_build_paths(
        &self,
        build_id: Uuid,
        store_path_hashes: &[StorePathHash],
    ) -> Result<()> {
        for store_path_hash in store_path_hashes {
            self.attach_build_path(build_id, store_path_hash).await?;
        }

        Ok(())
    }

    pub async fn get_build_path_nar_object_path(
        &self,
        build_id: Uuid,
        store_path_hash: &StorePathHash,
    ) -> Result<Option<String>> {
        let build_id_text = build_id.to_string();
        let store_path_hash_text = store_path_hash.as_str();

        let row = sqlx::query!(
            r#"
            SELECT
                pi.url
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
        .context("fetching build path nar object path")?;

        Ok(row.map(|row| row.url))
    }

    pub async fn list_build_path_nar_objects(
        &self,
        build_id: Uuid,
    ) -> Result<Vec<(StorePathHash, String)>> {
        let build_id_text = build_id.to_string();

        let rows = sqlx::query!(
            r#"
            SELECT
                bp.store_path_hash,
                pi.url
            FROM build_paths bp
            JOIN path_infos pi ON pi.store_path_hash = bp.store_path_hash
            WHERE bp.build_id = ?
            ORDER BY bp.store_path_hash ASC
            "#,
            build_id_text
        )
        .fetch_all(&self.pool)
        .await
        .context("listing build path nar objects")?;

        let mut result = Vec::with_capacity(rows.len());
        for row in rows {
            let store_path_hash = StorePathHash::from_hash(&row.store_path_hash)?;
            result.push((store_path_hash, row.url));
        }

        Ok(result)
    }

    pub async fn publish_build_to_ref(
        &self,
        project: &ProjectSlug,
        ref_name: &str,
        build_id: Uuid,
    ) -> Result<()> {
        let project_id = self.project_id_by_slug(project).await?;
        let build_id_text = build_id.to_string();
        let finalized_at = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .context("formatting finalized_at")?;
        let build_status = BuildStatus::Finalized.as_str();

        let mut tx = self
            .pool
            .begin()
            .await
            .context("beginning publish_build_to_ref transaction")?;

        sqlx::query!(
            r#"
            INSERT INTO project_refs (project_id, ref_name, build_id)
            VALUES (?, ?, ?)
            ON CONFLICT(project_id, ref_name) DO UPDATE SET
                build_id = excluded.build_id,
                updated_at = (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
            "#,
            project_id,
            ref_name,
            build_id_text,
        )
        .execute(&mut *tx)
        .await
        .context("setting project ref")?;

        sqlx::query!(
            r#"
            UPDATE builds
            SET status = ?, finalized_at = ?
            WHERE id = ?
            "#,
            build_status,
            finalized_at,
            build_id_text,
        )
        .execute(&mut *tx)
        .await
        .context("marking build finalized")?;

        tx.commit()
            .await
            .context("committing publish_build_to_ref transaction")?;

        Ok(())
    }
}
