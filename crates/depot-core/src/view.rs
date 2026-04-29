use crate::project::ProjectSlug;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheView {
    Aggregate,
    Project(ProjectSlug),
}
