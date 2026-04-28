use anyhow::{Context as _, Result, bail};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use cache_core::project::ProjectSlug;

use crate::models::{AccessTokenLookupRow, AccessTokenRecord};
use crate::pool::SqliteDatabase;

#[derive(Debug, Clone)]
pub struct CreatedAccessToken {
    pub token: String,
    pub records: Vec<AccessTokenRecord>,
}

impl SqliteDatabase {
    pub async fn create_access_token(
        &self,
        name: &str,
        project: &ProjectSlug,
        ref_patterns: &[String],
        expires_at: Option<OffsetDateTime>,
    ) -> Result<CreatedAccessToken> {
        let name = name.trim();
        if name.is_empty() {
            bail!("access token name must not be empty");
        }

        let project_id = self.project_id_by_slug(project).await?;
        let token_id = Uuid::now_v7().to_string();
        let token = generate_access_token();
        let token_hash = hash_access_token(&token);
        let expires_at_text = expires_at
            .map(|value| value.format(&Rfc3339))
            .transpose()
            .context("formatting access token expires_at")?;

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
            .context("beginning create_access_token transaction")?;

        sqlx::query!(
            r#"
            INSERT INTO access_tokens (
                id,
                token_hash,
                name,
                project_id,
                expires_at
            )
            VALUES (?, ?, ?, ?, ?)
            "#,
            token_id,
            token_hash,
            name,
            project_id,
            expires_at_text
        )
        .execute(&mut *tx)
        .await
        .context("inserting access token")?;

        for ref_pattern in &normalized_patterns {
            sqlx::query!(
                r#"
                INSERT INTO access_token_ref_patterns (
                    token_id,
                    ref_pattern
                )
                VALUES (?, ?)
                "#,
                token_id,
                ref_pattern
            )
            .execute(&mut *tx)
            .await
            .context("inserting access token ref pattern")?;
        }

        tx.commit()
            .await
            .context("committing create_access_token transaction")?;

        let records = self
            .list_access_token_records_by_id(&token_id)
            .await
            .context("loading created access token")?;

        Ok(CreatedAccessToken { token, records })
    }

    pub async fn list_access_tokens(
        &self,
        project: Option<&ProjectSlug>,
    ) -> Result<Vec<AccessTokenRecord>> {
        let project_slug = project.map(ProjectSlug::as_str);

        let rows = sqlx::query_as!(
            AccessTokenLookupRow,
            r#"
            SELECT
                t.id,
                t.name,
                p.slug AS project_slug,
                r.ref_pattern,
                t.created_at,
                t.expires_at,
                t.revoked_at
            FROM access_tokens t
            JOIN projects p ON p.id = t.project_id
            JOIN access_token_ref_patterns r ON r.token_id = t.id
            WHERE (? IS NULL OR p.slug = ?)
            ORDER BY p.slug ASC, t.name ASC, r.ref_pattern ASC
            "#,
            project_slug,
            project_slug,
        )
        .fetch_all(&self.pool)
        .await
        .context("listing access tokens")?;

        rows.into_iter()
            .map(AccessTokenLookupRow::into_record)
            .collect()
    }

    pub async fn revoke_access_token(&self, token_id: &str) -> Result<bool> {
        let now = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .context("formatting revoked_at")?;

        let result = sqlx::query!(
            r#"
            UPDATE access_tokens
            SET revoked_at = ?
            WHERE id = ? AND revoked_at IS NULL
            "#,
            now,
            token_id,
        )
        .execute(&self.pool)
        .await
        .context("revoking access token")?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn list_active_access_token_records_by_token(
        &self,
        token: &str,
    ) -> Result<Vec<AccessTokenRecord>> {
        let token_hash = hash_access_token(token);
        let now = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .context("formatting current time")?;

        let rows = sqlx::query_as!(
            AccessTokenLookupRow,
            r#"
            SELECT
                t.id,
                t.name,
                p.slug AS project_slug,
                r.ref_pattern,
                t.created_at,
                t.expires_at,
                t.revoked_at
            FROM access_tokens t
            JOIN projects p ON p.id = t.project_id
            JOIN access_token_ref_patterns r ON r.token_id = t.id
            WHERE t.token_hash = ?
              AND t.revoked_at IS NULL
              AND (t.expires_at IS NULL OR t.expires_at > ?)
            ORDER BY r.ref_pattern ASC
            "#,
            token_hash,
            now,
        )
        .fetch_all(&self.pool)
        .await
        .context("looking up active access token")?;

        rows.into_iter()
            .map(AccessTokenLookupRow::into_record)
            .collect()
    }

    pub async fn list_access_token_records_by_id(
        &self,
        token_id: &str,
    ) -> Result<Vec<AccessTokenRecord>> {
        let rows = sqlx::query_as!(
            AccessTokenLookupRow,
            r#"
            SELECT
                t.id,
                t.name,
                p.slug AS project_slug,
                r.ref_pattern,
                t.created_at,
                t.expires_at,
                t.revoked_at
            FROM access_tokens t
            JOIN projects p ON p.id = t.project_id
            JOIN access_token_ref_patterns r ON r.token_id = t.id
            WHERE t.id = ?
            ORDER BY r.ref_pattern ASC
            "#,
            token_id
        )
        .fetch_all(&self.pool)
        .await
        .context("listing access token by id")?;

        rows.into_iter()
            .map(AccessTokenLookupRow::into_record)
            .collect()
    }

    pub async fn list_active_access_token_records_by_id(
        &self,
        token_id: &str,
    ) -> Result<Vec<AccessTokenRecord>> {
        let now = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .context("formatting current time")?;

        let rows = sqlx::query_as!(
            AccessTokenLookupRow,
            r#"
            SELECT
                t.id,
                t.name,
                p.slug AS project_slug,
                r.ref_pattern,
                t.created_at,
                t.expires_at,
                t.revoked_at
            FROM access_tokens t
            JOIN projects p ON p.id = t.project_id
            JOIN access_token_ref_patterns r ON r.token_id = t.id
            WHERE t.id = ?
            AND t.revoked_at IS NULL
            AND (t.expires_at IS NULL OR t.expires_at > ?)
            ORDER BY r.ref_pattern ASC
            "#,
            token_id,
            now,
        )
        .fetch_all(&self.pool)
        .await
        .context("looking up active access token by id")?;

        rows.into_iter()
            .map(AccessTokenLookupRow::into_record)
            .collect()
    }
}

pub fn hash_access_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    hex::encode(digest)
}

fn generate_access_token() -> String {
    format!("npc_{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}
