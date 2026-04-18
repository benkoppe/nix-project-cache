use anyhow::{Context as _, Result, anyhow};
use uuid::Uuid;

use cache_core::project::ProjectSlug;

use crate::models::{ProjectLookupRow, ProjectRecord};
use crate::pool::SqliteDatabase;

impl SqliteDatabase {
    pub async fn insert_project(
        &self,
        slug: &ProjectSlug,
        display_name: &str,
        public: bool,
    ) -> Result<()> {
        let id = Uuid::now_v7().to_string();
        let slug_text = slug.as_str();
        let public_int = if public { 1_i64 } else { 0_i64 };

        sqlx::query!(
            r#"
            INSERT INTO projects (id, slug, display_name, public)
            VALUES (?, ?, ?, ?)
            ON CONFLICT(slug) DO UPDATE SET
                display_name = excluded.display_name,
                public = excluded.public
            "#,
            id,
            slug_text,
            display_name,
            public_int,
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
