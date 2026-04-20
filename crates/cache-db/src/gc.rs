use anyhow::{Context as _, Result};

use cache_core::nix::StorePathHash;

use crate::pool::SqliteDatabase;

impl SqliteDatabase {
    pub async fn list_root_store_path_hashes(&self) -> Result<Vec<StorePathHash>> {
        let rows = sqlx::query!(
            r#"
            SELECT DISTINCT store_path_hash
            FROM (
                SELECT bp.store_path_hash AS store_path_hash
                FROM project_refs pr
                JOIN build_paths bp ON bp.build_id = pr.build_id

                UNION

                SELECT pins.store_path_hash AS store_path_hash
                FROM pins
            )
            ORDER BY store_path_hash ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .context("listing root store_path_hashes")?;

        rows.into_iter()
            .map(|row| {
                StorePathHash::from_hash(&row.store_path_hash)
                    .with_context(|| format!("invalid store_path_hash {}", row.store_path_hash))
            })
            .collect()
    }

    pub async fn list_live_store_path_hashes(&self) -> Result<Vec<StorePathHash>> {
        let rows = sqlx::query!(
            r#"
            WITH RECURSIVE live_paths(store_path_hash, store_path) AS (
                SELECT DISTINCT
                    pi.store_path_hash,
                    pi.store_path
                FROM path_infos pi
                WHERE pi.store_path_hash IN (
                    SELECT bp.store_path_hash
                    FROM project_refs pr
                    JOIN build_paths bp ON bp.build_id = pr.build_id

                    UNION

                    SELECT pins.store_path_hash
                    FROM pins
                )

                UNION

                SELECT DISTINCT
                    ref_pi.store_path_hash,
                    ref_pi.store_path
                FROM live_paths lp
                JOIN path_references pr
                    ON pr.store_path_hash = lp.store_path_hash
                JOIN path_infos ref_pi
                    ON ref_pi.store_path = pr.reference_store_path
            )
            SELECT DISTINCT store_path_hash
            FROM live_paths
            ORDER BY store_path_hash ASC
            "#
        )
        .fetch_all(&self.pool)
        .await
        .context("listing live store_path_hashes")?;

        rows.into_iter()
            .map(|row| {
                StorePathHash::from_hash(&row.store_path_hash)
                    .with_context(|| format!("invalid store_path_hash {}", row.store_path_hash))
            })
            .collect()
    }

    pub async fn list_live_local_object_paths(&self) -> Result<Vec<String>> {
        let rows = sqlx::query!(
            r#"
            WITH RECURSIVE live_paths(store_path_hash, store_path) AS (
                SELECT DISTINCT
                    pi.store_path_hash,
                    pi.store_path
                FROM path_infos pi
                WHERE pi.store_path_hash IN (
                    SELECT bp.store_path_hash
                    FROM project_refs pr
                    JOIN build_paths bp ON bp.build_id = pr.build_id

                    UNION

                    SELECT pins.store_path_hash
                    FROM pins
                )

                UNION

                SELECT DISTINCT
                    ref_pi.store_path_hash,
                    ref_pi.store_path
                FROM live_paths lp
                JOIN path_references pr
                    ON pr.store_path_hash = lp.store_path_hash
                JOIN path_infos ref_pi
                    ON ref_pi.store_path = pr.reference_store_path
            )
            SELECT DISTINCT po.object_path
            FROM live_paths lp
            JOIN path_objects po ON po.store_path_hash = lp.store_path_hash
            ORDER BY po.object_path ASC
            "#
        )
        .fetch_all(&self.pool)
        .await
        .context("listing live local object paths")?;

        Ok(rows.into_iter().map(|row| row.object_path).collect())
    }

    pub async fn list_stale_local_object_paths(&self) -> Result<Vec<String>> {
        let rows = sqlx::query!(
            r#"
            WITH RECURSIVE live_paths(store_path_hash, store_path) AS (
                SELECT DISTINCT
                    pi.store_path_hash,
                    pi.store_path
                FROM path_infos pi
                WHERE pi.store_path_hash IN (
                    SELECT bp.store_path_hash
                    FROM project_refs pr
                    JOIN build_paths bp ON bp.build_id = pr.build_id

                    UNION

                    SELECT pins.store_path_hash
                    FROM pins
                )

                UNION

                SELECT DISTINCT
                    ref_pi.store_path_hash,
                    ref_pi.store_path
                FROM live_paths lp
                JOIN path_references pr
                    ON pr.store_path_hash = lp.store_path_hash
                JOIN path_infos ref_pi
                    ON ref_pi.store_path = pr.reference_store_path
            ),
            live_objects AS (
                SELECT DISTINCT po.object_path
                FROM live_paths lp
                JOIN path_objects po ON po.store_path_hash = lp.store_path_hash
            )
            SELECT lo.object_path
            FROM local_objects lo
            WHERE NOT EXISTS (
                SELECT 1
                FROM live_objects lio
                WHERE lio.object_path = lo.object_path
            )
            ORDER BY lo.object_path ASC
            "#
        )
        .fetch_all(&self.pool)
        .await
        .context("listing stale local object paths")?;

        Ok(rows.into_iter().map(|row| row.object_path).collect())
    }

    pub async fn delete_local_objects_by_path(&self, object_paths: &[String]) -> Result<()> {
        let mut tx = self
            .pool
            .begin()
            .await
            .context("beginning delete_local_objects_by_path transaction")?;

        for object_path in object_paths {
            sqlx::query!(
                r#"
                DELETE FROM local_objects
                WHERE object_path = ?
                "#,
                object_path,
            )
            .execute(&mut *tx)
            .await
            .with_context(|| format!("deleting local object row {}", object_path))?;
        }

        tx.commit()
            .await
            .context("committing delete_local_objects_by_path transaction")?;

        Ok(())
    }
}
