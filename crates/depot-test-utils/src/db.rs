use std::path::PathBuf;

use anyhow::Result;
use tempfile::TempDir;

use depot_core::{project::ProjectSlug, storage::StorageId};
use depot_db::SqliteDatabase;

use crate::fixtures::{EXAMPLE_PROJECT_NAME, example_project};

pub struct TestDatabase {
    pub db: SqliteDatabase,
    pub temp_dir: TempDir,
}

impl TestDatabase {
    pub async fn new() -> Result<Self> {
        let temp_dir = tempfile::tempdir()?;
        let db = SqliteDatabase::open(temp_dir.path().join("depot.db")).await?;

        Ok(Self { db, temp_dir })
    }

    pub fn objects_root(&self) -> PathBuf {
        self.temp_dir.path().join("objects")
    }

    pub async fn insert_example_project(&self) -> Result<ProjectSlug> {
        let project = example_project();
        self.db
            .insert_project(&project, EXAMPLE_PROJECT_NAME, true, &StorageId::main())
            .await?;
        Ok(project)
    }
}
