use anyhow::{Context as _, Result};
use sqlx::{QueryBuilder, Sqlite};

use cache_core::nix::StorePathHash;

use crate::pool::SqliteDatabase;

impl SqliteDatabase {
    pub async fn list_root_store_path_hashes(&self) -> Result<Vec<StorePathHash>> {
        let retained_build_ids = self.list_retained_build_ids_for_gc().await?;

        let mut hashes = Vec::new();

        for build_id in retained_build_ids {
            let rows = sqlx::query!(
                r#"
                SELECT store_path_hash
                FROM build_paths
                WHERE build_id = ?
                "#,
                build_id,
            )
            .fetch_all(&self.pool)
            .await
            .context("listing retained build root paths")?;

            for row in rows {
                hashes.push(
                    StorePathHash::from_hash(&row.store_path_hash).with_context(|| {
                        format!("invalid store_path_hash {}", row.store_path_hash)
                    })?,
                );
            }
        }

        let pin_rows = sqlx::query!(
            r#"
            SELECT DISTINCT store_path_hash
            FROM pins
            "#
        )
        .fetch_all(&self.pool)
        .await
        .context("listing pinned root paths")?;

        for row in pin_rows {
            hashes.push(
                StorePathHash::from_hash(&row.store_path_hash)
                    .with_context(|| format!("invalid store_path_hash {}", row.store_path_hash))?,
            );
        }

        hashes.sort();
        hashes.dedup();

        Ok(hashes)
    }

    pub async fn list_live_store_path_hashes(&self) -> Result<Vec<StorePathHash>> {
        let roots = self.list_root_store_path_hashes().await?;

        let mut builder = QueryBuilder::<Sqlite>::new("");
        push_live_paths_cte(&mut builder, &roots);
        builder.push(
            r#"
            SELECT DISTINCT store_path_hash
            FROM live_paths
            ORDER BY store_path_hash ASC
            "#,
        );

        let rows = builder
            .build_query_as::<(String,)>()
            .fetch_all(&self.pool)
            .await
            .context("listing live store_path_hashes")?;

        rows.into_iter()
            .map(|(store_path_hash,)| {
                StorePathHash::from_hash(&store_path_hash)
                    .with_context(|| format!("invalid store_path_hash {store_path_hash}"))
            })
            .collect()
    }

    pub async fn list_live_local_object_paths(&self) -> Result<Vec<String>> {
        let roots = self.list_root_store_path_hashes().await?;

        let mut builder = QueryBuilder::<Sqlite>::new("");
        push_live_paths_cte(&mut builder, &roots);
        builder.push(
            r#"
            SELECT DISTINCT po.object_path
            FROM live_paths lp
            JOIN path_objects po ON po.store_path_hash = lp.store_path_hash
            ORDER BY po.object_path ASC
            "#,
        );

        let rows = builder
            .build_query_as::<(String,)>()
            .fetch_all(&self.pool)
            .await
            .context("listing live local object paths")?;

        Ok(rows.into_iter().map(|(object_path,)| object_path).collect())
    }

    pub async fn list_stale_local_object_paths(&self) -> Result<Vec<String>> {
        let roots = self.list_root_store_path_hashes().await?;

        let mut builder = QueryBuilder::<Sqlite>::new("");
        push_live_objects_cte(&mut builder, &roots);
        builder.push(
            r#"
            SELECT lo.object_path
            FROM local_objects lo
            WHERE NOT EXISTS (
                SELECT 1
                FROM live_objects lio
                WHERE lio.object_path = lo.object_path
            )
            ORDER BY lo.object_path ASC
            "#,
        );

        let rows = builder
            .build_query_as::<(String,)>()
            .fetch_all(&self.pool)
            .await
            .context("listing stale local object paths")?;

        Ok(rows.into_iter().map(|(object_path,)| object_path).collect())
    }

    pub async fn clear_deleted_state_for_live_local_objects(&self) -> Result<()> {
        let roots = self.list_root_store_path_hashes().await?;

        let mut builder = QueryBuilder::<Sqlite>::new("");
        push_live_objects_cte(&mut builder, &roots);
        builder.push(
            r#"
            UPDATE local_objects
            SET
                deleted_at = NULL,
                first_deleted_at = NULL
            WHERE (deleted_at IS NOT NULL OR first_deleted_at IS NOT NULL)
            AND EXISTS (
                SELECT 1
                FROM live_objects lio
                WHERE lio.object_path = local_objects.object_path
            )
            "#,
        );

        builder
            .build()
            .execute(&self.pool)
            .await
            .context("clearing tombstones for live local objects")?;

        Ok(())
    }

    pub async fn mark_stale_local_objects(&self) -> Result<()> {
        let roots = self.list_root_store_path_hashes().await?;

        let mut builder = QueryBuilder::<Sqlite>::new("");
        push_live_objects_cte(&mut builder, &roots);
        builder.push(
            r#"
            UPDATE local_objects
            SET
                deleted_at = COALESCE(
                    deleted_at,
                    strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                ),
                first_deleted_at = COALESCE(
                    first_deleted_at,
                    strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                )
            WHERE NOT EXISTS (
                SELECT 1
                FROM live_objects lio
                WHERE lio.object_path = local_objects.object_path
            )
            "#,
        );

        builder
            .build()
            .execute(&self.pool)
            .await
            .context("marking stale local objects")?;

        Ok(())
    }

    pub async fn list_local_objects_ready_for_deletion(
        &self,
        grace_period_seconds: i64,
    ) -> Result<Vec<String>> {
        let rows = sqlx::query!(
            r#"
            SELECT object_path
            FROM local_objects
            WHERE deleted_at IS NOT NULL
              AND first_deleted_at IS NOT NULL
              AND unixepoch(first_deleted_at) <= unixepoch('now') - ?
            ORDER BY object_path ASC
            "#,
            grace_period_seconds,
        )
        .fetch_all(&self.pool)
        .await
        .context("listing local objects ready for deletion")?;

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

fn push_root_values(builder: &mut QueryBuilder<'_, Sqlite>, roots: &[StorePathHash]) {
    if roots.is_empty() {
        builder.push("SELECT CAST(NULL AS TEXT) WHERE 0");
        return;
    }

    builder.push("VALUES ");

    for (index, root) in roots.iter().enumerate() {
        if index > 0 {
            builder.push(", ");
        }
        builder
            .push("(")
            .push_bind(root.as_str().to_owned())
            .push(")");
    }
}

fn push_live_paths_cte(builder: &mut QueryBuilder<'_, Sqlite>, roots: &[StorePathHash]) {
    builder.push(
        r#"
        WITH RECURSIVE root_store_path_hashes(store_path_hash) AS (
        "#,
    );

    push_root_values(builder, roots);

    builder.push(
        r#"
        ),
        live_paths(store_path_hash, store_path) AS (
            SELECT DISTINCT
                pi.store_path_hash,
                pi.store_path
            FROM path_infos pi
            JOIN root_store_path_hashes roots
                ON roots.store_path_hash = pi.store_path_hash

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
        "#,
    );
}

fn push_live_objects_cte(builder: &mut QueryBuilder<'_, Sqlite>, roots: &[StorePathHash]) {
    builder.push(
        r#"
        WITH RECURSIVE root_store_path_hashes(store_path_hash) AS (
        "#,
    );

    push_root_values(builder, roots);

    builder.push(
        r#"
        ),
        live_paths(store_path_hash, store_path) AS (
            SELECT DISTINCT
                pi.store_path_hash,
                pi.store_path
            FROM path_infos pi
            JOIN root_store_path_hashes roots
                ON roots.store_path_hash = pi.store_path_hash

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
        "#,
    );
}
