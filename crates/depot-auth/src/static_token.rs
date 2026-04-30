use async_trait::async_trait;

use crate::{AuthError, AuthenticatedIdentity, Authorizer};

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
    async fn authorize_bearer(
        &self,
        bearer_token: Option<&str>,
    ) -> Result<AuthenticatedIdentity, AuthError> {
        let configured = self
            .configured_token
            .as_deref()
            .ok_or(AuthError::Disabled)?;

        let provided = bearer_token.ok_or(AuthError::MissingToken)?;

        if provided == configured {
            Ok(AuthenticatedIdentity::bootstrap_admin())
        } else {
            Err(AuthError::InvalidToken)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::IdentityKind;

    use super::*;

    #[tokio::test]
    async fn static_token_authorizes_matching_token() {
        let authorizer = StaticTokenAuthorizer::new(Some("secret".to_owned()));

        let identity = authorizer.authorize_bearer(Some("secret")).await.unwrap();

        assert_eq!(identity.subject, "static-token");
        assert_eq!(identity.provider.as_deref(), Some("static-token"));
        assert_eq!(identity.kind, IdentityKind::BootstrapAdmin);
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
