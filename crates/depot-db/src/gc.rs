use anyhow::{Context as _, Result};
use sqlx::{QueryBuilder, Sqlite};

use depot_core::nix::StorePathHash;
use depot_core::storage::StorageId;

use crate::pool::SqliteDatabase;

#[derive(Debug, Clone)]
pub struct StorageObjectKey {
    pub storage_id: StorageId,
    pub object_path: String,
}

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

        let pending_rows = sqlx::query!(
            r#"
            SELECT DISTINCT bp.store_path_hash
            FROM build_paths bp
            JOIN builds b ON b.id = bp.build_id
            WHERE b.status = 'pending'
            "#
        )
        .fetch_all(&self.pool)
        .await
        .context("listing pending build root paths")?;

        for row in pending_rows {
            hashes.push(
                StorePathHash::from_hash(&row.store_path_hash)
                    .with_context(|| format!("invalid store_path_hash {}", row.store_path_hash))?,
            );
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

    pub async fn list_live_storage_object_paths(&self) -> Result<Vec<String>> {
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
            .context("listing live storage object paths")?;

        Ok(rows.into_iter().map(|(object_path,)| object_path).collect())
    }

    pub async fn list_stale_storage_objects(&self) -> Result<Vec<StorageObjectKey>> {
        let roots = self.list_root_store_path_hashes().await?;

        let mut builder = QueryBuilder::<Sqlite>::new("");
        push_live_objects_cte(&mut builder, &roots);
        builder.push(
            r#"
            SELECT so.storage_id, so.object_path
            FROM storage_objects so
            WHERE NOT EXISTS (
                SELECT 1
                FROM live_objects lio
                WHERE lio.object_path = so.object_path
            )
            ORDER BY so.storage_id ASC, so.object_path ASC
            "#,
        );

        let rows = builder
            .build_query_as::<(String, String)>()
            .fetch_all(&self.pool)
            .await
            .context("listing stale storage objects")?;

        rows.into_iter()
            .map(|(storage_id, object_path)| {
                Ok(StorageObjectKey {
                    storage_id: StorageId::new(storage_id)
                        .map_err(anyhow::Error::new)
                        .context("parsing storage_id")?,
                    object_path,
                })
            })
            .collect()
    }

    pub async fn clear_deleted_state_for_live_storage_objects(&self) -> Result<()> {
        let roots = self.list_root_store_path_hashes().await?;

        let mut builder = QueryBuilder::<Sqlite>::new("");
        push_live_objects_cte(&mut builder, &roots);
        builder.push(
            r#"
            UPDATE storage_objects
            SET
                deleted_at = NULL,
                first_deleted_at = NULL
            WHERE (deleted_at IS NOT NULL OR first_deleted_at IS NOT NULL)
            AND EXISTS (
                SELECT 1
                FROM live_objects lio
                WHERE lio.object_path = storage_objects.object_path
            )
            "#,
        );

        builder
            .build()
            .execute(&self.pool)
            .await
            .context("clearing tombstones for live storage objects")?;

        Ok(())
    }

    pub async fn mark_stale_storage_objects(&self) -> Result<()> {
        let roots = self.list_root_store_path_hashes().await?;

        let mut builder = QueryBuilder::<Sqlite>::new("");
        push_live_objects_cte(&mut builder, &roots);
        builder.push(
            r#"
            UPDATE storage_objects
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
                WHERE lio.object_path = storage_objects.object_path
            )
            "#,
        );

        builder
            .build()
            .execute(&self.pool)
            .await
            .context("marking stale storage objects")?;

        Ok(())
    }

    pub async fn list_storage_objects_ready_for_deletion(
        &self,
        grace_period_seconds: i64,
    ) -> Result<Vec<StorageObjectKey>> {
        let rows = sqlx::query!(
            r#"
            SELECT storage_id, object_path
            FROM storage_objects
            WHERE deleted_at IS NOT NULL
            AND first_deleted_at IS NOT NULL
            AND unixepoch(first_deleted_at) <= unixepoch('now') - ?
            ORDER BY storage_id ASC, object_path ASC
            "#,
            grace_period_seconds,
        )
        .fetch_all(&self.pool)
        .await
        .context("listing storage objects ready for deletion")?;

        rows.into_iter()
            .map(|row| {
                Ok(StorageObjectKey {
                    storage_id: StorageId::new(row.storage_id)
                        .map_err(anyhow::Error::new)
                        .context("parsing storage_id")?,
                    object_path: row.object_path,
                })
            })
            .collect()
    }

    pub async fn delete_storage_objects(&self, objects: &[StorageObjectKey]) -> Result<()> {
        let mut tx = self
            .pool
            .begin()
            .await
            .context("beginning delete_storage_objects transaction")?;

        for object in objects {
            let storage_id = object.storage_id.as_str();

            sqlx::query!(
                r#"
                DELETE FROM storage_objects
                WHERE storage_id = ?
                  AND object_path = ?
            "#,
                storage_id,
                object.object_path,
            )
            .execute(&mut *tx)
            .await
            .with_context(|| {
                format!(
                    "deleting storage object row {}:{}",
                    object.storage_id, object.object_path
                )
            })?;
        }

        tx.commit()
            .await
            .context("committing delete_storage_objects transaction")?;

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
