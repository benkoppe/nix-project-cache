use std::collections::BTreeSet;
use std::sync::Arc;

use axum::http::{HeaderMap, header};
use thiserror::Error;
use wildmatch::WildMatch;

use cache_auth::{AuthError, AuthenticatedIdentity, Authorizer, IdentityKind};
use cache_core::project::ProjectSlug;
use cache_db::SqliteDatabase;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorizedScope {
    Admin,
    Project {
        project: ProjectSlug,
        ref_patterns: Vec<String>,
    },
}

#[derive(Debug, Clone)]
pub struct AuthorizedPrincipal {
    pub identity: AuthenticatedIdentity,
    pub scope: AuthorizedScope,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AuthorizationServiceError {
    #[error("authorization service unavailable")]
    ServiceUnavailable,
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
}

#[derive(Clone)]
pub struct AuthorizationService {
    db: SqliteDatabase,
    authorizer: Arc<dyn Authorizer>,
}

impl AuthorizationService {
    pub fn new(db: SqliteDatabase, authorizer: Arc<dyn Authorizer>) -> Self {
        Self { db, authorizer }
    }

    pub async fn authorize_headers(
        &self,
        headers: &HeaderMap,
    ) -> Result<AuthorizedPrincipal, AuthorizationServiceError> {
        let bearer = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .map(str::trim);

        let identity = match self.authorizer.authorize_bearer(bearer).await {
            Ok(identity) => identity,
            Err(AuthError::Disabled) | Err(AuthError::Unavailable(_)) => {
                return Err(AuthorizationServiceError::ServiceUnavailable);
            }
            Err(AuthError::MissingToken) | Err(AuthError::InvalidToken) => {
                return Err(AuthorizationServiceError::Unauthorized);
            }
        };

        self.resolve_identity(identity).await
    }

    async fn resolve_identity(
        &self,
        identity: AuthenticatedIdentity,
    ) -> Result<AuthorizedPrincipal, AuthorizationServiceError> {
        match &identity.kind {
            IdentityKind::BootstrapAdmin => Ok(AuthorizedPrincipal {
                identity,
                scope: AuthorizedScope::Admin,
            }),
            IdentityKind::Oidc(oidc) => {
                let provider = identity
                    .provider
                    .as_deref()
                    .ok_or(AuthorizationServiceError::Forbidden)?;
                let repository = oidc
                    .repository
                    .as_deref()
                    .ok_or(AuthorizationServiceError::Forbidden)?;

                let bindings = self
                    .db
                    .list_matching_project_oidc_identities(provider, repository)
                    .await
                    .map_err(|error| {
                        tracing::error!(
                            ?error,
                            provider,
                            repository,
                            "listing oidc bindings failed"
                        );
                        AuthorizationServiceError::ServiceUnavailable
                    })?;

                let matching = bindings
                    .into_iter()
                    .filter(|binding| {
                        ref_pattern_matches(
                            binding.ref_pattern.as_deref(),
                            oidc.ref_name.as_deref(),
                        )
                    })
                    .collect::<Vec<_>>();

                if matching.is_empty() {
                    return Err(AuthorizationServiceError::Forbidden);
                }

                let distinct_projects = matching
                    .iter()
                    .map(|binding| binding.project_slug.clone())
                    .collect::<BTreeSet<_>>();

                if distinct_projects.len() != 1 {
                    return Err(AuthorizationServiceError::Forbidden);
                }

                let project = distinct_projects.into_iter().next().unwrap();

                let ref_patterns = if matching.iter().any(|binding| binding.ref_pattern.is_none()) {
                    Vec::new()
                } else {
                    let mut patterns = matching
                        .into_iter()
                        .filter_map(|binding| binding.ref_pattern)
                        .collect::<Vec<_>>();
                    patterns.sort();
                    patterns.dedup();
                    patterns
                };

                Ok(AuthorizedPrincipal {
                    identity,
                    scope: AuthorizedScope::Project {
                        project,
                        ref_patterns,
                    },
                })
            }
            IdentityKind::AccessToken(_) => Err(AuthorizationServiceError::Forbidden),
        }
    }
}

impl AuthorizedPrincipal {
    pub fn require_admin(&self) -> Result<(), AuthorizationServiceError> {
        match self.scope {
            AuthorizedScope::Admin => Ok(()),
            AuthorizedScope::Project { .. } => Err(AuthorizationServiceError::Forbidden),
        }
    }

    pub fn require_project(&self, project: &ProjectSlug) -> Result<(), AuthorizationServiceError> {
        match &self.scope {
            AuthorizedScope::Admin => Ok(()),
            AuthorizedScope::Project {
                project: principal_project,
                ..
            } if principal_project == project => Ok(()),
            AuthorizedScope::Project { .. } => Err(AuthorizationServiceError::Forbidden),
        }
    }

    pub fn require_project_ref(
        &self,
        project: &ProjectSlug,
        ref_name: &str,
    ) -> Result<(), AuthorizationServiceError> {
        match &self.scope {
            AuthorizedScope::Admin => Ok(()),
            AuthorizedScope::Project {
                project: principal_project,
                ref_patterns,
            } if principal_project == project => {
                if ref_patterns.is_empty()
                    || ref_patterns
                        .iter()
                        .any(|pattern| WildMatch::new(pattern).matches(ref_name))
                {
                    Ok(())
                } else {
                    Err(AuthorizationServiceError::Forbidden)
                }
            }
            AuthorizedScope::Project { .. } => Err(AuthorizationServiceError::Forbidden),
        }
    }
}

fn ref_pattern_matches(pattern: Option<&str>, ref_name: Option<&str>) -> bool {
    match pattern {
        None => true,
        Some(pattern) => ref_name
            .map(|ref_name| WildMatch::new(pattern).matches(ref_name))
            .unwrap_or(false),
    }
}
