use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct AuthenticatedIdentity {
    pub subject: String,
    pub provider: Option<String>,
    pub kind: IdentityKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IdentityKind {
    BootstrapAdmin,
    Oidc(OidcIdentity),
    AccessToken(AccessTokenIdentity),
}

#[derive(Debug, Clone, PartialEq)]
pub struct OidcIdentity {
    pub issuer: String,
    pub repository: Option<String>,
    pub ref_name: Option<String>,
    pub claims: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessTokenIdentity {
    pub token_id: String,
}

impl AuthenticatedIdentity {
    pub fn bootstrap_admin() -> Self {
        Self {
            subject: "static-token".to_owned(),
            provider: Some("static-token".to_owned()),
            kind: IdentityKind::BootstrapAdmin,
        }
    }

    pub fn oidc(
        provider: impl Into<String>,
        subject: impl Into<String>,
        issuer: impl Into<String>,
        repository: Option<String>,
        ref_name: Option<String>,
        claims: Map<String, Value>,
    ) -> Self {
        Self {
            subject: subject.into(),
            provider: Some(provider.into()),
            kind: IdentityKind::Oidc(OidcIdentity {
                issuer: issuer.into(),
                repository,
                ref_name,
                claims,
            }),
        }
    }

    pub fn access_token(token_id: impl Into<String>, subject: impl Into<String>) -> Self {
        Self {
            subject: subject.into(),
            provider: Some("access-token".to_owned()),
            kind: IdentityKind::AccessToken(AccessTokenIdentity {
                token_id: token_id.into(),
            }),
        }
    }
}
