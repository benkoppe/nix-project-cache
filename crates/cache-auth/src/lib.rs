mod chain;
mod error;
mod oidc;
mod oidc_claims;
mod oidc_config;
mod oidc_http;
mod principal;
mod static_token;

use async_trait::async_trait;

pub use chain::ChainAuthorizer;
pub use error::AuthError;
pub use oidc::OidcAuthorizer;
pub use oidc_config::{ConfiguredOidcProvider, OidcConfig, OidcConfigError, OidcProviderConfig};
pub use oidc_http::{OidcHttpClient, OidcHttpError, ReqwestOidcHttpClient, StaticOidcHttpClient};
pub use principal::Principal;
pub use static_token::StaticTokenAuthorizer;

#[async_trait]
pub trait Authorizer: Send + Sync + 'static {
    async fn authorize_bearer(&self, bearer_token: Option<&str>) -> Result<Principal, AuthError>;
}
