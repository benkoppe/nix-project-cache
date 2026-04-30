use anyhow::{Context as _, Result, bail};

use depot_core::project::ProjectSlug;

use crate::models::{ProjectOidcIdentityLookupRow, ProjectOidcIdentityRecord};
use crate::pool::SqliteDatabase;

impl SqliteDatabase {
    pub async fn replace_project_oidc_identity(
        &self,
        project: &ProjectSlug,
        provider: &str,
        repository: &str,
        ref_patterns: &[String],
    ) -> Result<()> {
        let provider = provider.trim();
        let repository = repository.trim();

        if provider.is_empty() {
            bail!("provider must not be empty");
        }
        if repository.is_empty() {
            bail!("repository must not be empty");
        }

        let project_id = self.project_id_by_slug(project).await?;
        let mut normalized_patterns = ref_patterns
            .iter()
            .map(|pattern| pattern.trim())
            .filter(|pattern| !pattern.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();

        normalized_patterns.sort();
        normalized_patterns.dedup();

        if normalized_patterns.is_empty() {
            normalized_patterns.push(String::new());
        }

        let mut tx = self
            .pool()
            .begin()
            .await
            .context("beginning replace_project_oidc_identity transaction")?;

        sqlx::query!(
            r#"
            DELETE FROM project_oidc_identities
            WHERE project_id = ? AND provider = ? AND repository = ?
            "#,
            project_id,
            provider,
            repository
        )
        .execute(&mut *tx)
        .await
        .context("deleting existing project oidc identities")?;

        for ref_pattern in &normalized_patterns {
            sqlx::query!(
                r#"
                INSERT INTO project_oidc_identities (
                    project_id,
                    provider,
                    repository,
                    ref_pattern
                )
                VALUES (?, ?, ?, ?)
                "#,
                project_id,
                provider,
                repository,
                ref_pattern
            ).execute(&mut *tx)
            .await
            .with_context(|| {
                format!(
                    "inserting project oidc identity for provider {} repository {} ref_pattern {:?}",
                    provider, repository, ref_pattern
                )
            })?;
        }

        tx.commit()
            .await
            .context("committing replace_project_oidc_identity transaction")?;

        Ok(())
    }

    pub async fn list_project_oidc_identities(
        &self,
        project: &ProjectSlug,
    ) -> Result<Vec<ProjectOidcIdentityRecord>> {
        let project_id = self.project_id_by_slug(project).await?;

        let rows = sqlx::query_as!(
            ProjectOidcIdentityLookupRow,
            r#"
            SELECT
                p.slug AS project_slug,
                i.provider,
                i.repository,
                i.ref_pattern,
                i.created_at
            FROM project_oidc_identities i
            JOIN projects p ON p.id = i.project_id
            WHERE i.project_id = ?
            ORDER BY i.provider ASC, i.repository ASC, i.ref_pattern ASC
            "#,
            project_id
        )
        .fetch_all(&self.pool)
        .await
        .context("listing project oidc identities")?;

        rows.into_iter()
            .map(ProjectOidcIdentityLookupRow::into_record)
            .collect()
    }

    pub async fn delete_project_oidc_identity(
        &self,
        project: &ProjectSlug,
        provider: &str,
        repository: &str,
    ) -> Result<bool> {
        let project_id = self.project_id_by_slug(project).await?;
        let provider = provider.trim();
        let repository = repository.trim();

        let result = sqlx::query!(
            r#"
            DELETE FROM project_oidc_identities
            WHERE project_id = ? AND provider = ? AND repository = ?
            "#,
            project_id,
            provider,
            repository,
        )
        .execute(&self.pool)
        .await
        .context("deleting project oidc identity")?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn list_matching_project_oidc_identities(
        &self,
        provider: &str,
        repository: &str,
    ) -> Result<Vec<ProjectOidcIdentityRecord>> {
        let rows = sqlx::query_as!(
            ProjectOidcIdentityLookupRow,
            r#"
            SELECT
                p.slug AS project_slug,
                i.provider,
                i.repository,
                i.ref_pattern,
                i.created_at
            FROM project_oidc_identities i
            JOIN projects p ON p.id = i.project_id
            WHERE i.provider = ? AND i.repository = ?
            ORDER BY p.slug ASC, i.ref_pattern ASC
            "#,
            provider,
            repository,
        )
        .fetch_all(&self.pool)
        .await
        .context("listing matching project oidc identities")?;

        rows.into_iter()
            .map(ProjectOidcIdentityLookupRow::into_record)
            .collect()
    }
}
