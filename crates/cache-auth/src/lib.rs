use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    pub subject: String,
}

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("write API is disabled")]
    Disabled,
    #[error("missing bearer token")]
    MissingToken,
    #[error("invalid bearer token")]
    InvalidToken,
}

pub trait Authorizer: Send + Sync + 'static {
    fn authorize_bearer(&self, bearer_token: Option<&str>) -> Result<Principal, AuthError>;
}

#[derive(Debug, Clone)]
pub struct StaticTokenAuthorizer {
    configured_token: Option<String>,
}

impl StaticTokenAuthorizer {
    pub fn new(configured_token: Option<String>) -> Self {
        Self { configured_token }
    }
}

impl Authorizer for StaticTokenAuthorizer {
    fn authorize_bearer(&self, bearer_token: Option<&str>) -> Result<Principal, AuthError> {
        let configured = self
            .configured_token
            .as_deref()
            .ok_or(AuthError::Disabled)?;

        let provided = bearer_token.ok_or(AuthError::MissingToken)?;

        if provided == configured {
            Ok(Principal {
                subject: "static-token".to_owned(),
            })
        } else {
            Err(AuthError::InvalidToken)
        }
    }
}
