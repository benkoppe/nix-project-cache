use anyhow::{Context as _, Result, anyhow};
use uuid::Uuid;

use depot_core::project::ProjectSlug;
use depot_core::storage::StorageId;

use crate::models::{ProjectLookupRow, ProjectRecord};
use crate::pool::SqliteDatabase;

impl SqliteDatabase {
    pub async fn insert_project(
        &self,
        slug: &ProjectSlug,
        display_name: &str,
        public: bool,
        storage_id: &StorageId,
    ) -> Result<()> {
        let id = Uuid::now_v7().to_string();
        let slug_text = slug.as_str();
        let public_int = if public { 1_i64 } else { 0_i64 };
        let storage_id_text = storage_id.as_str();

        sqlx::query!(
            r#"
            INSERT INTO projects (id, slug, display_name, public, storage_id)
            VALUES (?, ?, ?, ?, ?)
            ON CONFLICT(slug) DO UPDATE SET
                display_name = excluded.display_name,
                public = excluded.public,
                storage_id = excluded.storage_id
            "#,
            id,
            slug_text,
            display_name,
            public_int,
            storage_id_text,
        )
        .execute(&self.pool)
        .await
        .context("inserting project")?;

        Ok(())
    }

    pub async fn get_project_by_slug(&self, slug: &ProjectSlug) -> Result<Option<ProjectRecord>> {
        let slug_text = slug.as_str();

        let row = sqlx::query_as!(
            ProjectLookupRow,
            r#"
            SELECT
                id,
                slug,
                display_name,
                public,
                storage_id,
                created_at
            FROM projects
            WHERE slug = ?
            LIMIT 1
            "#,
            slug_text,
        )
        .fetch_optional(&self.pool)
        .await
        .context("getting project by slug")?;

        row.map(ProjectLookupRow::into_record).transpose()
    }

    pub async fn list_projects(&self) -> Result<Vec<ProjectRecord>> {
        let rows = sqlx::query_as!(
            ProjectLookupRow,
            r#"
            SELECT
                id,
                slug,
                display_name,
                public,
                storage_id,
                created_at
            FROM projects
            ORDER BY slug ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .context("listing projects")?;

        rows.into_iter()
            .map(ProjectLookupRow::into_record)
            .collect()
    }

    pub async fn get_project_storage_id(&self, slug: &ProjectSlug) -> Result<StorageId> {
        let slug_text = slug.as_str();

        let storage_id = sqlx::query_scalar!(
            r#"
            SELECT storage_id
            FROM projects
            WHERE slug = ?
            LIMIT 1
            "#,
            slug_text,
        )
        .fetch_optional(&self.pool)
        .await
        .context("getting project storage id")?
        .ok_or_else(|| anyhow!("project {} not found", slug_text))?;

        StorageId::new(storage_id)
            .map_err(anyhow::Error::new)
            .context("parsing project storage_id")
    }

    pub async fn list_project_storage_ids(&self) -> Result<Vec<(ProjectSlug, StorageId)>> {
        let rows = sqlx::query!(
            r#"
            SELECT slug, storage_id
            FROM projects
            ORDER BY slug ASC
            "#
        )
        .fetch_all(&self.pool)
        .await
        .context("listing project storage ids")?;
        rows.into_iter()
            .map(|row| {
                let project = ProjectSlug::parse(&row.slug)
                    .map_err(|_| anyhow!("invalid project slug {}", row.slug))?;
                let storage_id = StorageId::new(row.storage_id)
                    .map_err(anyhow::Error::new)
                    .context("parsing project storage_id")?;
                Ok((project, storage_id))
            })
            .collect()
    }

    pub(crate) async fn project_id_by_slug(&self, slug: &ProjectSlug) -> Result<String> {
        let slug_text = slug.as_str();

        let id = sqlx::query_scalar!(
            r#"
            SELECT id
            FROM projects
            WHERE slug = ?
            LIMIT 1
            "#,
            slug_text,
        )
        .fetch_optional(&self.pool)
        .await
        .context("looking up project id by slug")?
        .ok_or_else(|| anyhow!("project {} not found", slug_text))?;

        Ok(id)
    }
}
