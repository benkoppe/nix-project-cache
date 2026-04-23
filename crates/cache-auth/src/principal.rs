use cache_core::project::ProjectSlug;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    pub subject: String,
    pub provider: Option<String>,
    pub project: Option<ProjectSlug>,
    pub ref_name: Option<String>,
}

impl Principal {
    pub fn static_token() -> Self {
        Self {
            subject: "static-token".to_owned(),
            provider: Some("static-token".to_owned()),
            project: None,
            ref_name: None,
        }
    }
}
