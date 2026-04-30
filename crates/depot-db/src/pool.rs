use std::fs;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context as _, Result};
use sqlx::SqlitePool;
use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

#[derive(Clone)]
pub struct SqliteDatabase {
    pub(crate) pool: SqlitePool,
}

impl SqliteDatabase {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("creating sqlite parent directory {}", parent.display())
            })?;
        }

        let url = format!("sqlite://{}", path.to_string_lossy());
        let options = SqliteConnectOptions::from_str(&url)
            .context("building sqlite connect options")?
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(5));

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .context("opening sqlite pool")?;

        let db = Self { pool };
        db.migrate().await?;

        Ok(db)
    }

    pub async fn open_read_only(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let url = format!("sqlite://{}", path.to_string_lossy());

        let options = SqliteConnectOptions::from_str(&url)
            .context("building sqlite read-only connect options")?
            .read_only(true)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .context("opening sqlite read-only pool")?;

        Ok(Self { pool })
    }

    #[cfg(test)]
    pub async fn open_temp_for_tests() -> Result<(Self, tempfile::TempDir)> {
        let temp_dir = tempfile::tempdir().context("creating temporary sqlite test directory")?;
        let db_path = temp_dir.path().join("depot.db");
        let db = Self::open(&db_path).await?;
        Ok((db, temp_dir))
    }

    pub async fn migrate(&self) -> Result<()> {
        MIGRATOR
            .run(&self.pool)
            .await
            .context("running sqlite migrations")?;

        Ok(())
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}
