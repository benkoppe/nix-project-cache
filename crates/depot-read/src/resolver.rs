use std::collections::HashMap;

use anyhow::Result;
use async_trait::async_trait;

use depot_core::narinfo::NarInfo;
use depot_core::nix::StorePathHash;
use depot_core::project::ProjectSlug;
use depot_core::view::CacheView;
use depot_db::SqliteDatabase;

#[async_trait]
pub trait NarInfoResolver: Send + Sync + 'static {
    async fn resolve_narinfo(
        &self,
        view: &CacheView,
        store_path_hash: &StorePathHash,
    ) -> Result<Option<NarInfo>>;
}

#[derive(Debug, Default, Clone)]
pub struct InMemoryNarInfoResolver {
    aggregate: HashMap<StorePathHash, NarInfo>,
    projects: HashMap<ProjectSlug, HashMap<StorePathHash, NarInfo>>,
}

impl InMemoryNarInfoResolver {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert_aggregate(&mut self, store_path_hash: StorePathHash, narinfo: NarInfo) {
        self.aggregate.insert(store_path_hash, narinfo);
    }

    pub fn insert_project(
        &mut self,
        project: ProjectSlug,
        store_path_hash: StorePathHash,
        narinfo: NarInfo,
    ) {
        self.projects
            .entry(project)
            .or_default()
            .insert(store_path_hash, narinfo);
    }
}

#[async_trait]
impl NarInfoResolver for InMemoryNarInfoResolver {
    async fn resolve_narinfo(
        &self,
        view: &CacheView,
        store_path_hash: &StorePathHash,
    ) -> Result<Option<NarInfo>> {
        let result = match view {
            CacheView::Aggregate => self.aggregate.get(store_path_hash).cloned(),
            CacheView::Project(project) => self
                .projects
                .get(project)
                .and_then(|entries| entries.get(store_path_hash))
                .cloned(),
        };

        Ok(result)
    }
}

#[derive(Clone)]
pub struct DbNarInfoResolver {
    db: SqliteDatabase,
}

impl DbNarInfoResolver {
    pub fn new(db: SqliteDatabase) -> Self {
        Self { db }
    }
}

#[async_trait]
impl NarInfoResolver for DbNarInfoResolver {
    async fn resolve_narinfo(
        &self,
        view: &CacheView,
        store_path_hash: &StorePathHash,
    ) -> Result<Option<NarInfo>> {
        match view {
            CacheView::Aggregate => self.db.get_aggregate_narinfo(store_path_hash).await,
            CacheView::Project(project) => {
                self.db.get_project_narinfo(project, store_path_hash).await
            }
        }
    }
}
