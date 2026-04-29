use std::collections::HashMap;

use anyhow::Result;
use async_trait::async_trait;

use depot_core::project::ProjectSlug;
use depot_core::view::DepotView;
use depot_db::SqliteDatabase;
use depot_store::upstream::UpstreamCache;

#[async_trait]
pub trait UpstreamSelector: Send + Sync + 'static {
    async fn upstreams_for_view(&self, view: &DepotView) -> Result<Vec<UpstreamCache>>;
}

#[derive(Default, Clone)]
pub struct StaticUpstreamSelector {
    aggregate: Vec<UpstreamCache>,
    projects: HashMap<ProjectSlug, Vec<UpstreamCache>>,
}

impl StaticUpstreamSelector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_aggregate_upstreams(&mut self, upstreams: Vec<UpstreamCache>) {
        self.aggregate = upstreams;
    }

    pub fn set_project_upstreams(&mut self, project: ProjectSlug, upstreams: Vec<UpstreamCache>) {
        self.projects.insert(project, upstreams);
    }
}

#[async_trait]
impl UpstreamSelector for StaticUpstreamSelector {
    async fn upstreams_for_view(&self, view: &DepotView) -> Result<Vec<UpstreamCache>> {
        let upstreams = match view {
            DepotView::Aggregate => self.aggregate.clone(),
            DepotView::Project(project) => self.projects.get(project).cloned().unwrap_or_default(),
        };

        Ok(upstreams)
    }
}

#[derive(Clone)]
pub struct DbUpstreamSelector {
    db: SqliteDatabase,
}

impl DbUpstreamSelector {
    pub fn new(db: SqliteDatabase) -> Self {
        Self { db }
    }
}

#[async_trait]
impl UpstreamSelector for DbUpstreamSelector {
    async fn upstreams_for_view(&self, view: &DepotView) -> Result<Vec<UpstreamCache>> {
        match view {
            DepotView::Aggregate => Ok(self
                .db
                .list_enabled_upstreams()
                .await?
                .into_iter()
                .map(|record| record.into_runtime_config())
                .collect()),
            DepotView::Project(project) => Ok(self
                .db
                .list_enabled_upstreams_for_project(project)
                .await?
                .into_iter()
                .map(|record| record.into_runtime_config())
                .collect()),
        }
    }
}

#[cfg(test)]
mod tests {
    use depot_core::storage::StorageId;
    use tempfile::tempdir;
    use uuid::Uuid;

    use depot_db::SqliteDatabase;
    use depot_test_utils::{EXAMPLE_PROJECT_NAME, example_project};

    use super::*;

    #[tokio::test]
    async fn static_upstream_selector_returns_configured_project_upstreams() {
        let project = example_project();
        let upstream = UpstreamCache::new(
            Uuid::now_v7(),
            "cache.nixos.org",
            "https://cache.nixos.org",
            10,
        );

        let mut selector = StaticUpstreamSelector::new();
        selector.set_project_upstreams(project.clone(), vec![upstream.clone()]);

        let loaded = selector
            .upstreams_for_view(&DepotView::Project(project))
            .await
            .unwrap();

        assert_eq!(loaded, vec![upstream]);
    }

    #[tokio::test]
    async fn db_upstream_selector_scopes_project_upstreams() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("depot.db");
        let db = SqliteDatabase::open(&db_path).await.unwrap();

        let project = ProjectSlug::parse("example_repo").unwrap();
        let upstream = UpstreamCache::new(
            Uuid::now_v7(),
            "cache.nixos.org",
            "https://cache.nixos.org",
            10,
        );

        db.insert_project(&project, EXAMPLE_PROJECT_NAME, true, &StorageId::main())
            .await
            .unwrap();
        db.insert_upstream_cache(&upstream, true).await.unwrap();
        db.link_project_upstream(&project, upstream.id)
            .await
            .unwrap();

        let selector = DbUpstreamSelector::new(db);

        let aggregate = selector
            .upstreams_for_view(&DepotView::Aggregate)
            .await
            .unwrap();
        let project_upstreams = selector
            .upstreams_for_view(&DepotView::Project(project))
            .await
            .unwrap();

        assert_eq!(aggregate.len(), 1);
        assert_eq!(project_upstreams.len(), 1);
        assert_eq!(project_upstreams[0].id, upstream.id);
    }
}
