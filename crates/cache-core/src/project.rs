use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ProjectSlug(String);

#[derive(Debug, Error)]
pub enum ProjectSlugError {
    #[error("project slug must be lowercase ascii with '-' or '_'")]
    Invalid,
}

impl ProjectSlug {
    pub fn parse(input: &str) -> Result<Self, ProjectSlugError> {
        let valid = !input.is_empty()
            && input
                .bytes()
                .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_'));
        if !valid {
            return Err(ProjectSlugError::Invalid);
        }
        Ok(Self(input.to_owned()))
    }
}
