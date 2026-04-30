use anyhow::{Context as _, Result};
use uuid::Uuid;

use depot_core::key_crypto::{EncryptedSigningKey, KeyEncryptionKey};
use depot_core::project::ProjectSlug;
use depot_core::signing::NamedSigningKey;

use crate::models::{ProjectSigningKeyLookupRow, ProjectSigningKeyRecord};
use crate::pool::SqliteDatabase;

impl SqliteDatabase {
    pub async fn active_project_signing_key_public(
        &self,
        project: &ProjectSlug,
    ) -> Result<Option<String>> {
        Ok(self
            .active_project_signing_key_record(project)
            .await?
            .map(|record| record.public_key))
    }

    pub async fn active_project_signing_key(
        &self,
        project: &ProjectSlug,
        key_encryption_key: &KeyEncryptionKey,
    ) -> Result<Option<NamedSigningKey>> {
        let Some(record) = self.active_project_signing_key_record(project).await? else {
            return Ok(None);
        };

        let aad = project_signing_key_aad(&record.project_slug, &record.name);
        let private_key_text = key_encryption_key
            .decrypt(
                &EncryptedSigningKey {
                    ciphertext: record.encrypted_private_key,
                    nonce: record.nonce,
                },
                aad.as_bytes(),
            )
            .context("decrypting project signing key")?;

        Ok(Some(
            NamedSigningKey::parse(&private_key_text)
                .map_err(anyhow::Error::new)
                .context("parsing decrypted project signing key")?,
        ))
    }

    pub async fn active_project_signing_key_record(
        &self,
        project: &ProjectSlug,
    ) -> Result<Option<ProjectSigningKeyRecord>> {
        let project_slug = project.as_str();

        let row = sqlx::query_as!(
            ProjectSigningKeyLookupRow,
            r#"
            SELECT
                psk.id,
                p.slug AS project_slug,
                psk.name,
                psk.public_key,
                psk.encrypted_private_key,
                psk.nonce,
                psk.created_at,
                psk.retired_at
            FROM project_signing_keys psk
            JOIN projects p ON p.id = psk.project_id
            WHERE p.slug = ?
              AND psk.retired_at IS NULL
            LIMIT 1
            "#,
            project_slug,
        )
        .fetch_optional(&self.pool)
        .await
        .context("loading active project signing key")?;

        row.map(ProjectSigningKeyLookupRow::into_record).transpose()
    }

    pub async fn ensure_project_signing_key(
        &self,
        project: &ProjectSlug,
        key_encryption_key: &KeyEncryptionKey,
    ) -> Result<String> {
        if let Some(public_key) = self.active_project_signing_key_public(project).await? {
            return Ok(public_key);
        }

        self.generate_project_signing_key(project, None, key_encryption_key)
            .await
    }

    pub async fn generate_project_signing_key(
        &self,
        project: &ProjectSlug,
        name: Option<&str>,
        key_encryption_key: &KeyEncryptionKey,
    ) -> Result<String> {
        let name = match name {
            Some(name) if !name.trim().is_empty() => name.trim().to_owned(),
            _ => self.next_project_signing_key_name(project).await?,
        };

        let signing_key = NamedSigningKey::generate(&name)
            .map_err(anyhow::Error::new)
            .context("generating project signing key")?;

        self.replace_project_signing_key(project, &signing_key, key_encryption_key)
            .await
    }

    pub async fn import_project_signing_key(
        &self,
        project: &ProjectSlug,
        signing_key: NamedSigningKey,
        key_encryption_key: &KeyEncryptionKey,
    ) -> Result<String> {
        self.replace_project_signing_key(project, &signing_key, key_encryption_key)
            .await
    }

    pub async fn replace_project_signing_key(
        &self,
        project: &ProjectSlug,
        signing_key: &NamedSigningKey,
        key_encryption_key: &KeyEncryptionKey,
    ) -> Result<String> {
        let project_id = self.project_id_by_slug(project).await?;
        let id = Uuid::now_v7().to_string();
        let name = signing_key.name();
        let public_key = signing_key.public_key_text();
        let private_key_text = signing_key.private_key_text();

        let aad = project_signing_key_aad(project, name);
        let encrypted = key_encryption_key
            .encrypt(&private_key_text, aad.as_bytes())
            .context("encrypting project signing key")?;

        let mut tx = self
            .pool
            .begin()
            .await
            .context("beginning replace_project_signing_key transaction")?;

        sqlx::query!(
            r#"
            UPDATE project_signing_keys
            SET retired_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE project_id = ?
              AND retired_at IS NULL
            "#,
            project_id,
        )
        .execute(&mut *tx)
        .await
        .context("retiring existing project signing key")?;

        sqlx::query!(
            r#"
            INSERT INTO project_signing_keys (
                id,
                project_id,
                name,
                public_key,
                encrypted_private_key,
                nonce
            )
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
            id,
            project_id,
            name,
            public_key,
            encrypted.ciphertext,
            encrypted.nonce,
        )
        .execute(&mut *tx)
        .await
        .context("inserting project signing key")?;

        tx.commit()
            .await
            .context("committing replace_project_signing_key transaction")?;

        Ok(public_key)
    }

    async fn next_project_signing_key_name(&self, project: &ProjectSlug) -> Result<String> {
        let project_id = self.project_id_by_slug(project).await?;

        let count = sqlx::query_scalar!(
            r#"
            SELECT COUNT(*) AS "count!: i64"
            FROM project_signing_keys
            WHERE project_id = ?
            "#,
            project_id,
        )
        .fetch_one(&self.pool)
        .await
        .context("counting project signing keys")?;

        Ok(format!("{}-{}", project.as_str(), count + 1))
    }
}

fn project_signing_key_aad(project: &ProjectSlug, key_name: &str) -> String {
    format!("project-signing-key:{}:{key_name}", project.as_str())
}
