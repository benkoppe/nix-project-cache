use anyhow::{Context as _, Result, anyhow};

use cache_core::nix::StorePathHash;
use cache_core::project::ProjectSlug;

use crate::models::{PinLookupRow, PinRecord};
use crate::pool::SqliteDatabase;

const GLOBAL_SCOPE_KEY: &str = "__global__";

impl SqliteDatabase {
    pub async fn upsert_pin(
        &self,
        name: &str,
        project: Option<&ProjectSlug>,
        store_path_hash: &StorePathHash,
        store_path: &str,
    ) -> Result<()> {
        let (scope_key, project_id) = self.pin_scope(project).await?;
        let store_path_hash_text = store_path_hash.as_str();

        let existing = sqlx::query_scalar!(
            r#"
            SELECT store_path
            FROM path_infos
            WHERE store_path_hash = ?
            LIMIT 1
            "#,
            store_path_hash_text,
        )
        .fetch_optional(&self.pool)
        .await
        .context("checking pinned store path exists")?;

        let Some(existing_store_path) = existing else {
            return Err(anyhow!(
                "cannot pin unknown store path hash {}",
                store_path_hash_text
            ));
        };

        if existing_store_path != store_path {
            return Err(anyhow!(
                "pin store path {} does not match registered path {}",
                store_path,
                existing_store_path
            ));
        }

        sqlx::query!(
            r#"
            INSERT INTO pins (scope_key, name, project_id, store_path_hash, store_path)
            VALUES (?, ?, ?, ?, ?)
            ON CONFLICT(scope_key, name) DO UPDATE SET
                project_id = excluded.project_id,
                store_path_hash = excluded.store_path_hash,
                store_path = excluded.store_path,
                updated_at = (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
            "#,
            scope_key,
            name,
            project_id,
            store_path_hash_text,
            store_path,
        )
        .execute(&self.pool)
        .await
        .context("upserting pin")?;

        Ok(())
    }

    pub async fn get_pin(
        &self,
        name: &str,
        project: Option<&ProjectSlug>,
    ) -> Result<Option<PinRecord>> {
        let (scope_key, _) = self.pin_scope(project).await?;

        let row = sqlx::query_as!(
            PinLookupRow,
            r#"
            SELECT
                pins.scope_key,
                pins.name,
                projects.slug AS project_slug,
                pins.store_path_hash,
                pins.store_path,
                pins.created_at,
                pins.updated_at
            FROM pins
            LEFT JOIN projects ON projects.id = pins.project_id
            WHERE pins.scope_key = ? AND pins.name = ?
            LIMIT 1
            "#,
            scope_key,
            name,
        )
        .fetch_optional(&self.pool)
        .await
        .context("getting pin")?;

        row.map(PinLookupRow::into_record).transpose()
    }

    pub async fn list_pins(&self, project: Option<&ProjectSlug>) -> Result<Vec<PinRecord>> {
        let rows = if let Some(project) = project {
            let (scope_key, _) = self.pin_scope(Some(project)).await?;
            sqlx::query_as!(
                PinLookupRow,
                r#"
                SELECT
                    pins.scope_key,
                    pins.name,
                    projects.slug AS project_slug,
                    pins.store_path_hash,
                    pins.store_path,
                    pins.created_at,
                    pins.updated_at
                FROM pins
                LEFT JOIN projects ON projects.id = pins.project_id
                WHERE pins.scope_key = ?
                ORDER BY pins.name ASC
                "#,
                scope_key
            )
            .fetch_all(&self.pool)
            .await
            .context("listing project pins")?
        } else {
            sqlx::query_as!(
                PinLookupRow,
                r#"
                SELECT
                    pins.scope_key,
                    pins.name,
                    projects.slug AS project_slug,
                    pins.store_path_hash,
                    pins.store_path,
                    pins.created_at,
                    pins.updated_at
                FROM pins
                LEFT JOIN projects ON projects.id = pins.project_id
                ORDER BY pins.scope_key ASC, pins.name ASC
                "#
            )
            .fetch_all(&self.pool)
            .await
            .context("listing all pins")?
        };

        rows.into_iter().map(PinLookupRow::into_record).collect()
    }

    pub async fn delete_pin(&self, name: &str, project: Option<&ProjectSlug>) -> Result<bool> {
        let (scope_key, _) = self.pin_scope(project).await?;

        let result = sqlx::query!(
            r#"
            DELETE FROM pins
            WHERE scope_key = ? AND name = ?
            "#,
            scope_key,
            name
        )
        .execute(&self.pool)
        .await
        .context("deleting pin")?;

        Ok(result.rows_affected() > 0)
    }

    async fn pin_scope(&self, project: Option<&ProjectSlug>) -> Result<(String, Option<String>)> {
        match project {
            Some(project) => {
                let project_id = self.project_id_by_slug(project).await?;
                Ok((project_id.clone(), Some(project_id)))
            }
            None => Ok((GLOBAL_SCOPE_KEY.to_owned(), None)),
        }
    }
}
