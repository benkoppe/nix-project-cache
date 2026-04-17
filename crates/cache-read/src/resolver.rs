use std::collections::HashMap;

use anyhow::Result;
use async_trait::async_trait;

use cache_core::narinfo::NarInfo;
use cache_core::nix::StorePathHash;
use cache_core::project::ProjectSlug;
use cache_core::view::CacheView;

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
