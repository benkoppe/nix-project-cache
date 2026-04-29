use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AuthError {
    #[error("write API is disabled")]
    Disabled,
    #[error("write auth backend is temporarily unavailable: {0}")]
    Unavailable(String),
    #[error("missing bearer token")]
    MissingToken,
    #[error("invalid bearer token")]
    InvalidToken,
}
