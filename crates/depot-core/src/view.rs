use crate::project::ProjectSlug;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DepotView {
    Aggregate,
    Project(ProjectSlug),
}
