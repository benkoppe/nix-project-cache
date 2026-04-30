use anyhow::{Context as _, Result};
use uuid::Uuid;

use depot_core::project::ProjectSlug;
use depot_store::upstream::UpstreamCache;

use crate::models::{UpstreamCacheLookupRow, UpstreamCacheRecord};
use crate::pool::SqliteDatabase;

impl SqliteDatabase {
    pub async fn insert_upstream_cache(
        &self,
        upstream: &UpstreamCache,
        enabled: bool,
    ) -> Result<()> {
        let upstream_id = upstream.id.to_string();
        let name = upstream.name.as_str();
        let base_url = upstream.base_url.as_str();
        let priority = i64::from(upstream.priority);
        let enabled_int = if enabled { 1_i64 } else { 0_i64 };

        sqlx::query!(
            r#"
            INSERT INTO upstream_caches (id, name, base_url, priority, enabled)
            VALUES (?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                base_url = excluded.base_url,
                priority = excluded.priority,
                enabled = excluded.enabled
            "#,
            upstream_id,
            name,
            base_url,
            priority,
            enabled_int
        )
        .execute(&self.pool)
        .await
        .context("inserting upstream cache")?;

        Ok(())
    }

    pub async fn upsert_upstream_cache_by_name(
        &self,
        name: &str,
        base_url: &str,
        priority: u32,
        enabled: bool,
    ) -> Result<()> {
        let existing_id = sqlx::query_scalar!(
            r#"
            SELECT id
            FROM upstream_caches
            WHERE name = ?
            LIMIT 1
            "#,
            name,
        )
        .fetch_optional(&self.pool)
        .await
        .context("looking up upstream by name")?;

        let upstream_id = existing_id.unwrap_or_else(|| Uuid::now_v7().to_string());
        let priority = i64::from(priority);
        let enabled_int = if enabled { 1_i64 } else { 0_i64 };
        let normalized_base_url = base_url.trim_end_matches('/');

        sqlx::query!(
            r#"
            INSERT INTO upstream_caches (id, name, base_url, priority, enabled)
            VALUES (?, ?, ?, ?, ?)
            ON CONFLICT(name) DO UPDATE SET
                base_url = excluded.base_url,
                priority = excluded.priority,
                enabled = excluded.enabled
            "#,
            upstream_id,
            name,
            normalized_base_url,
            priority,
            enabled_int,
        )
        .execute(&self.pool)
        .await
        .context("upserting upstream cache by name")?;

        Ok(())
    }

    pub async fn list_upstream_caches(&self) -> Result<Vec<UpstreamCacheRecord>> {
        let rows = sqlx::query_as!(
            UpstreamCacheLookupRow,
            r#"
            SELECT
                id,
                name,
                base_url,
                priority,
                enabled,
                created_at
            FROM upstream_caches
            ORDER BY priority ASC, name ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .context("listing upstream caches")?;

        rows.into_iter()
            .map(UpstreamCacheLookupRow::into_record)
            .collect()
    }

    pub async fn get_upstream_cache_by_name(
        &self,
        name: &str,
    ) -> Result<Option<UpstreamCacheRecord>> {
        let row = sqlx::query_as!(
            UpstreamCacheLookupRow,
            r#"
            SELECT
                id,
                name,
                base_url,
                priority,
                enabled,
                created_at
            FROM upstream_caches
            WHERE name = ?
            LIMIT 1
            "#,
            name,
        )
        .fetch_optional(&self.pool)
        .await
        .context("getting upstream cache by name")?;

        row.map(UpstreamCacheLookupRow::into_record).transpose()
    }

    pub async fn set_upstream_enabled(&self, name: &str, enabled: bool) -> Result<bool> {
        let enabled_int = if enabled { 1_i64 } else { 0_i64 };

        let result = sqlx::query!(
            r#"
            UPDATE upstream_caches
            SET enabled = ?
            WHERE name = ?
            "#,
            enabled_int,
            name,
        )
        .execute(&self.pool)
        .await
        .context("setting upstream enabled state")?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn link_project_upstream(
        &self,
        project_slug: &ProjectSlug,
        upstream_id: Uuid,
    ) -> Result<()> {
        let project_id = self.project_id_by_slug(project_slug).await?;
        let upstream_id_text = upstream_id.to_string();

        sqlx::query!(
            r#"
            INSERT OR IGNORE INTO project_upstreams (project_id, upstream_id)
            VALUES (?, ?)
            "#,
            project_id,
            upstream_id_text
        )
        .execute(&self.pool)
        .await
        .context("linking project upstream")?;

        Ok(())
    }

    pub async fn link_project_upstream_by_name(
        &self,
        project_slug: &ProjectSlug,
        upstream_name: &str,
    ) -> Result<bool> {
        let Some(upstream) = self.get_upstream_cache_by_name(upstream_name).await? else {
            return Ok(false);
        };

        self.link_project_upstream(project_slug, upstream.id)
            .await?;
        Ok(true)
    }

    pub async fn unlink_project_upstream_by_name(
        &self,
        project_slug: &ProjectSlug,
        upstream_name: &str,
    ) -> Result<bool> {
        let project_id = self.project_id_by_slug(project_slug).await?;

        let result = sqlx::query!(
            r#"
            DELETE FROM project_upstreams
            WHERE project_id = ?
              AND upstream_id = (
                  SELECT id
                  FROM upstream_caches
                  WHERE name = ?
                  LIMIT 1
              )
            "#,
            project_id,
            upstream_name,
        )
        .execute(&self.pool)
        .await
        .context("unlinking project upstream by name")?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn list_enabled_upstreams(&self) -> Result<Vec<UpstreamCacheRecord>> {
        let rows = sqlx::query_as!(
            UpstreamCacheLookupRow,
            r#"
            SELECT
                id,
                name,
                base_url,
                priority,
                enabled,
                created_at
            FROM upstream_caches
            WHERE enabled = 1
            ORDER BY priority ASC, name ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .context("listing enabled upstreams")?;

        rows.into_iter()
            .map(UpstreamCacheLookupRow::into_record)
            .collect()
    }

    pub async fn list_upstreams_for_project(
        &self,
        project_slug: &ProjectSlug,
    ) -> Result<Vec<UpstreamCacheRecord>> {
        let project_slug_text = project_slug.as_str();

        let rows = sqlx::query_as!(
            UpstreamCacheLookupRow,
            r#"
            SELECT
                uc.id,
                uc.name,
                uc.base_url,
                uc.priority,
                uc.enabled,
                uc.created_at
            FROM upstream_caches uc
            JOIN project_upstreams pu ON pu.upstream_id = uc.id
            JOIN projects p ON p.id = pu.project_id
            WHERE p.slug = ?1
            ORDER BY uc.priority ASC, uc.name ASC
            "#,
            project_slug_text
        )
        .fetch_all(&self.pool)
        .await
        .context("listing project upstreams")?;

        rows.into_iter()
            .map(UpstreamCacheLookupRow::into_record)
            .collect()
    }

    pub async fn list_enabled_upstreams_for_project(
        &self,
        project_slug: &ProjectSlug,
    ) -> Result<Vec<UpstreamCacheRecord>> {
        let project_slug_text = project_slug.as_str();

        let rows = sqlx::query_as!(
            UpstreamCacheLookupRow,
            r#"
            SELECT 
                uc.id, 
                uc.name, 
                uc.base_url, 
                uc.priority, 
                uc.enabled, 
                uc.created_at
            FROM upstream_caches uc
            JOIN project_upstreams pu ON pu.upstream_id = uc.id
            JOIN projects p ON p.id = pu.project_id
            WHERE p.slug = ?1 AND uc.enabled = 1
            ORDER BY uc.priority ASC, uc.name ASC
            "#,
            project_slug_text
        )
        .fetch_all(&self.pool)
        .await
        .context("listing enabled project upstreams")?;

        rows.into_iter()
            .map(UpstreamCacheLookupRow::into_record)
            .collect()
    }
}
