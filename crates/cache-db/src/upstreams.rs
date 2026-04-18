use anyhow::{Context as _, Result};
use uuid::Uuid;

use cache_core::project::ProjectSlug;
use cache_store::upstream::UpstreamCache;

use crate::models::UpstreamCacheRow;
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

    pub async fn list_enabled_upstreams(&self) -> Result<Vec<UpstreamCache>> {
        let rows = sqlx::query_as!(
            UpstreamCacheRow,
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

        rows.into_iter().map(row_to_upstream).collect()
    }

    pub async fn list_enabled_upstreams_for_project(
        &self,
        project_slug: &ProjectSlug,
    ) -> Result<Vec<UpstreamCache>> {
        let project_slug_text = project_slug.as_str();

        let rows = sqlx::query_as!(
            UpstreamCacheRow,
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

        rows.into_iter().map(row_to_upstream).collect()
    }
}

fn row_to_upstream(row: UpstreamCacheRow) -> Result<UpstreamCache> {
    Ok(UpstreamCache::new(
        Uuid::parse_str(&row.id).context("parsing upstream id")?,
        row.name,
        row.base_url,
        u32::try_from(row.priority).context("converting upstream priority")?,
    ))
}
