use async_trait::async_trait;

use crate::{AuthError, Authorizer, Principal};

#[derive(Debug, Clone)]
pub struct StaticTokenAuthorizer {
    configured_token: Option<String>,
}

impl StaticTokenAuthorizer {
    pub fn new(configured_token: Option<String>) -> Self {
        Self { configured_token }
    }
}

#[async_trait]
impl Authorizer for StaticTokenAuthorizer {
    async fn authorize_bearer(&self, bearer_token: Option<&str>) -> Result<Principal, AuthError> {
        let configured = self
            .configured_token
            .as_deref()
            .ok_or(AuthError::Disabled)?;

        let provided = bearer_token.ok_or(AuthError::MissingToken)?;

        if provided == configured {
            Ok(Principal::static_token())
        } else {
            Err(AuthError::InvalidToken)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn static_token_authorizes_matching_token() {
        let authorizer = StaticTokenAuthorizer::new(Some("secret".to_owned()));

        let principal = authorizer.authorize_bearer(Some("secret")).await.unwrap();

        assert_eq!(principal.subject, "static-token");
        assert_eq!(principal.provider.as_deref(), Some("static-token"));
    }

    #[tokio::test]
    async fn static_token_rejects_wrong_token() {
        let authorizer = StaticTokenAuthorizer::new(Some("secret".to_owned()));

        let error = authorizer
            .authorize_bearer(Some("wrong"))
            .await
            .unwrap_err();

        assert_eq!(error, AuthError::InvalidToken);
    }
}
