use std::sync::Arc;

use async_trait::async_trait;
use axum::http::{HeaderMap, header};
use thiserror::Error;
use wildmatch::WildMatch;

use cache_auth::{AuthError, AuthenticatedIdentity, Authorizer, IdentityKind, OidcIdentity};
use cache_core::project::ProjectSlug;
use cache_db::{AccessTokenRecord, ProjectOidcIdentityRecord, SqliteDatabase};

#[derive(Clone)]
pub struct DbAccessTokenAuthorizer {
    db: SqliteDatabase,
}

impl DbAccessTokenAuthorizer {
    pub fn new(db: SqliteDatabase) -> Self {
        Self { db }
    }
}

#[async_trait]
impl Authorizer for DbAccessTokenAuthorizer {
    async fn authorize_bearer(
        &self,
        bearer_token: Option<&str>,
    ) -> Result<AuthenticatedIdentity, AuthError> {
        let token = bearer_token.ok_or(AuthError::MissingToken)?;

        let records = self
            .db
            .list_active_access_token_records_by_token(token)
            .await
            .map_err(|error| AuthError::Unavailable(error.to_string()))?;

        let Some(first) = records.first() else {
            return Err(AuthError::InvalidToken);
        };

        Ok(AuthenticatedIdentity::access_token(
            first.id.clone(),
            format!("access-token:{}", first.name),
        ))
    }
}

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

    pub async fn authenticate_headers(
        &self,
        headers: &HeaderMap,
    ) -> Result<AuthenticatedIdentity, AuthorizationServiceError> {
        match self
            .authorizer
            .authorize_bearer(bearer_from_headers(headers))
            .await
        {
            Ok(identity) => Ok(identity),
            Err(AuthError::MissingToken | AuthError::InvalidToken) => {
                Err(AuthorizationServiceError::Unauthorized)
            }
            Err(AuthError::Disabled | AuthError::Unavailable(_)) => {
                Err(AuthorizationServiceError::ServiceUnavailable)
            }
        }
    }

    pub async fn authorize_admin(
        &self,
        identity: AuthenticatedIdentity,
    ) -> Result<AuthorizedPrincipal, AuthorizationServiceError> {
        match identity.kind {
            IdentityKind::BootstrapAdmin => Ok(AuthorizedPrincipal {
                identity,
                scope: AuthorizedScope::Admin,
            }),
            IdentityKind::Oidc(_) | IdentityKind::AccessToken(_) => {
                Err(AuthorizationServiceError::Forbidden)
            }
        }
    }

    pub async fn authorize_project(
        &self,
        identity: AuthenticatedIdentity,
        project: &ProjectSlug,
    ) -> Result<AuthorizedPrincipal, AuthorizationServiceError> {
        match identity.kind.clone() {
            IdentityKind::BootstrapAdmin => Ok(AuthorizedPrincipal {
                identity,
                scope: AuthorizedScope::Admin,
            }),
            IdentityKind::Oidc(oidc) => self.authorize_oidc_project(identity, oidc, project).await,
            IdentityKind::AccessToken(access_token) => {
                self.authorize_access_token_project(identity, &access_token.token_id, project)
                    .await
            }
        }
    }

    pub async fn authorize_project_ref(
        &self,
        identity: AuthenticatedIdentity,
        project: &ProjectSlug,
        ref_name: &str,
    ) -> Result<AuthorizedPrincipal, AuthorizationServiceError> {
        match identity.kind.clone() {
            IdentityKind::BootstrapAdmin => Ok(AuthorizedPrincipal {
                identity,
                scope: AuthorizedScope::Admin,
            }),
            IdentityKind::Oidc(oidc) => {
                self.authorize_oidc_project_ref(identity, oidc, project, ref_name)
                    .await
            }
            IdentityKind::AccessToken(access_token) => {
                self.authorize_access_token_project_ref(
                    identity,
                    &access_token.token_id,
                    project,
                    ref_name,
                )
                .await
            }
        }
    }

    async fn authorize_oidc_project(
        &self,
        identity: AuthenticatedIdentity,
        oidc: OidcIdentity,
        project: &ProjectSlug,
    ) -> Result<AuthorizedPrincipal, AuthorizationServiceError> {
        let bindings = self
            .matching_oidc_bindings(&identity, &oidc)
            .await?
            .into_iter()
            .filter(|binding| &binding.project_slug == project)
            .collect::<Vec<_>>();

        if bindings.is_empty() {
            return Err(AuthorizationServiceError::Forbidden);
        }

        Ok(AuthorizedPrincipal {
            identity,
            scope: AuthorizedScope::Project {
                project: project.clone(),
                ref_patterns: ref_patterns_from_oidc_bindings(&bindings),
            },
        })
    }

    async fn authorize_oidc_project_ref(
        &self,
        identity: AuthenticatedIdentity,
        oidc: OidcIdentity,
        project: &ProjectSlug,
        ref_name: &str,
    ) -> Result<AuthorizedPrincipal, AuthorizationServiceError> {
        if let Some(token_ref) = oidc.ref_name.as_deref()
            && token_ref != ref_name
        {
            return Err(AuthorizationServiceError::Forbidden);
        }

        let bindings = self
            .matching_oidc_bindings(&identity, &oidc)
            .await?
            .into_iter()
            .filter(|binding| &binding.project_slug == project)
            .filter(|binding| {
                ref_pattern_matches(binding.ref_pattern.as_deref(), oidc.ref_name.as_deref())
            })
            .collect::<Vec<_>>();

        if bindings.is_empty() {
            return Err(AuthorizationServiceError::Forbidden);
        }

        Ok(AuthorizedPrincipal {
            identity,
            scope: AuthorizedScope::Project {
                project: project.clone(),
                ref_patterns: ref_patterns_from_oidc_bindings(&bindings),
            },
        })
    }

    async fn matching_oidc_bindings(
        &self,
        identity: &AuthenticatedIdentity,
        oidc: &OidcIdentity,
    ) -> Result<Vec<ProjectOidcIdentityRecord>, AuthorizationServiceError> {
        let provider = identity
            .provider
            .as_deref()
            .ok_or(AuthorizationServiceError::Forbidden)?;
        let repository = oidc
            .repository
            .as_deref()
            .ok_or(AuthorizationServiceError::Forbidden)?;

        self.db
            .list_matching_project_oidc_identities(provider, repository)
            .await
            .map_err(|error| {
                tracing::error!(?error, provider, repository, "listing oidc bindings failed");
                AuthorizationServiceError::ServiceUnavailable
            })
    }

    async fn authorize_access_token_project(
        &self,
        identity: AuthenticatedIdentity,
        token_id: &str,
        project: &ProjectSlug,
    ) -> Result<AuthorizedPrincipal, AuthorizationServiceError> {
        let records = self.active_access_token_records(token_id).await?;

        let matching = records
            .into_iter()
            .filter(|record| &record.project_slug == project)
            .collect::<Vec<_>>();

        if matching.is_empty() {
            return Err(AuthorizationServiceError::Forbidden);
        }

        Ok(AuthorizedPrincipal {
            identity,
            scope: AuthorizedScope::Project {
                project: project.clone(),
                ref_patterns: ref_patterns_from_access_token_records(&matching),
            },
        })
    }

    async fn authorize_access_token_project_ref(
        &self,
        identity: AuthenticatedIdentity,
        token_id: &str,
        project: &ProjectSlug,
        ref_name: &str,
    ) -> Result<AuthorizedPrincipal, AuthorizationServiceError> {
        let records = self.active_access_token_records(token_id).await?;

        let matching = records
            .into_iter()
            .filter(|record| &record.project_slug == project)
            .filter(|record| ref_pattern_matches(record.ref_pattern.as_deref(), Some(ref_name)))
            .collect::<Vec<_>>();

        if matching.is_empty() {
            return Err(AuthorizationServiceError::Forbidden);
        }

        Ok(AuthorizedPrincipal {
            identity,
            scope: AuthorizedScope::Project {
                project: project.clone(),
                ref_patterns: ref_patterns_from_access_token_records(&matching),
            },
        })
    }

    async fn active_access_token_records(
        &self,
        token_id: &str,
    ) -> Result<Vec<AccessTokenRecord>, AuthorizationServiceError> {
        let records = self
            .db
            .list_active_access_token_records_by_id(token_id)
            .await
            .map_err(|error| {
                tracing::error!(?error, token_id, "looking up active access token failed");
                AuthorizationServiceError::ServiceUnavailable
            })?;

        if records.is_empty() {
            Err(AuthorizationServiceError::Unauthorized)
        } else {
            Ok(records)
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

fn bearer_from_headers(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
}

fn ref_patterns_from_oidc_bindings(bindings: &[ProjectOidcIdentityRecord]) -> Vec<String> {
    if bindings.iter().any(|binding| binding.ref_pattern.is_none()) {
        Vec::new()
    } else {
        sorted_ref_patterns(
            bindings
                .iter()
                .filter_map(|binding| binding.ref_pattern.clone()),
        )
    }
}

fn ref_patterns_from_access_token_records(records: &[AccessTokenRecord]) -> Vec<String> {
    if records.iter().any(|record| record.ref_pattern.is_none()) {
        Vec::new()
    } else {
        sorted_ref_patterns(
            records
                .iter()
                .filter_map(|record| record.ref_pattern.clone()),
        )
    }
}

fn sorted_ref_patterns(patterns: impl Iterator<Item = String>) -> Vec<String> {
    let mut patterns = patterns.collect::<Vec<_>>();
    patterns.sort();
    patterns.dedup();
    patterns
}

fn ref_pattern_matches(pattern: Option<&str>, ref_name: Option<&str>) -> bool {
    match pattern {
        None => true,
        Some(pattern) => ref_name
            .map(|ref_name| WildMatch::new(pattern).matches(ref_name))
            .unwrap_or(false),
    }
}
