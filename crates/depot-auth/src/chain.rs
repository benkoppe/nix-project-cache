use std::sync::Arc;

use async_trait::async_trait;

use crate::{AuthError, AuthenticatedIdentity, Authorizer};

#[derive(Default)]
pub struct ChainAuthorizer {
    authorizers: Vec<Arc<dyn Authorizer>>,
}

impl ChainAuthorizer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, authorizer: Arc<dyn Authorizer>) {
        self.authorizers.push(authorizer);
    }
}

#[async_trait]
impl Authorizer for ChainAuthorizer {
    async fn authorize_bearer(
        &self,
        bearer_token: Option<&str>,
    ) -> Result<AuthenticatedIdentity, AuthError> {
        let mut saw_enabled = false;
        let mut saw_missing = false;
        let mut saw_invalid = false;
        let mut unavailable: Option<String> = None;

        for authorizer in &self.authorizers {
            match authorizer.authorize_bearer(bearer_token).await {
                Ok(identity) => return Ok(identity),
                Err(AuthError::Disabled) => {}
                Err(AuthError::MissingToken) => {
                    saw_enabled = true;
                    saw_missing = true;
                }
                Err(AuthError::InvalidToken) => {
                    saw_enabled = true;
                    saw_invalid = true;
                }
                Err(AuthError::Unavailable(message)) => {
                    saw_enabled = true;
                    if unavailable.is_none() {
                        unavailable = Some(message);
                    }
                }
            }
        }

        if !saw_enabled {
            Err(AuthError::Disabled)
        } else if let Some(message) = unavailable {
            Err(AuthError::Unavailable(message))
        } else if saw_invalid {
            Err(AuthError::InvalidToken)
        } else if saw_missing {
            Err(AuthError::MissingToken)
        } else {
            Err(AuthError::Disabled)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use serde_json::Map;

    use crate::{AuthError, AuthenticatedIdentity, Authorizer};

    use super::*;

    struct StubAuthorizer(Result<AuthenticatedIdentity, AuthError>);

    #[async_trait]
    impl Authorizer for StubAuthorizer {
        async fn authorize_bearer(
            &self,
            _bearer_token: Option<&str>,
        ) -> Result<AuthenticatedIdentity, AuthError> {
            self.0.clone()
        }
    }

    #[tokio::test]
    async fn chain_authorizer_returns_first_success() {
        let mut chain = ChainAuthorizer::new();
        chain.push(Arc::new(StubAuthorizer(Err(AuthError::InvalidToken))));
        chain.push(Arc::new(StubAuthorizer(Ok(AuthenticatedIdentity::oidc(
            "github",
            "repo:owner/repo:ref:refs/heads/main",
            "https://token.actions.githubusercontent.com",
            Some("owner/repo".to_owned()),
            Some("refs/heads/main".to_owned()),
            Map::new(),
        )))));

        let identity = chain.authorize_bearer(Some("token")).await.unwrap();

        assert_eq!(identity.subject, "repo:owner/repo:ref:refs/heads/main");
    }

    #[tokio::test]
    async fn chain_authorizer_reports_disabled_when_nothing_is_configured() {
        let chain = ChainAuthorizer::new();

        let error = chain.authorize_bearer(Some("token")).await.unwrap_err();

        assert_eq!(error, AuthError::Disabled);
    }
}
