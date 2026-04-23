use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcConfig {
    pub providers: BTreeMap<String, OidcProviderConfig>,
    #[serde(default)]
    pub allow_insecure: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcProviderConfig {
    pub issuer: String,
    pub audience: String,
    #[serde(default)]
    pub bound_claims: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub bound_subject: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct ConfiguredOidcProvider<'a> {
    pub name: &'a str,
    pub config: &'a OidcProviderConfig,
}

#[derive(Debug, Error)]
pub enum OidcConfigError {
    #[error("reading OIDC config file {path}: {message}")]
    Read { path: String, message: String },
    #[error("parsing OIDC config file {path}: {message}")]
    Parse { path: String, message: String },
    #[error("invalid OIDC config: {0}")]
    Invalid(String),
}

impl OidcConfig {
    pub fn load_from_path(path: &Path) -> Result<Self, OidcConfigError> {
        let path_text = path.display().to_string();
        let raw = std::fs::read_to_string(path).map_err(|error| OidcConfigError::Read {
            path: path_text.clone(),
            message: error.to_string(),
        })?;

        let config =
            serde_json::from_str::<Self>(&raw).map_err(|error| OidcConfigError::Parse {
                path: path_text,
                message: error.to_string(),
            })?;

        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), OidcConfigError> {
        if self.providers.is_empty() {
            return Err(OidcConfigError::Invalid(
                "no providers configured".to_owned(),
            ));
        }

        let mut issuers = HashMap::<&str, &str>::new();

        for (name, provider) in &self.providers {
            if provider.issuer.trim().is_empty() {
                return Err(OidcConfigError::Invalid(format!(
                    "provider {name:?}: missing issuer"
                )));
            }

            let issuer = reqwest::Url::parse(&provider.issuer).map_err(|error| {
                OidcConfigError::Invalid(format!(
                    "provider {name:?}: invalid issuer URL {:?}: {}",
                    provider.issuer, error
                ))
            })?;

            if issuer.scheme().is_empty() || issuer.host_str().is_none() {
                return Err(OidcConfigError::Invalid(format!(
                    "provider {name:?}: issuer URL {:?} must be absolute with scheme and host",
                    provider.issuer
                )));
            }

            if issuer.scheme() != "https" && !self.allow_insecure {
                return Err(OidcConfigError::Invalid(format!(
                    "provider {name:?}: issuer URL {:?} must use HTTPS",
                    provider.issuer
                )));
            }

            if provider.audience.trim().is_empty() {
                return Err(OidcConfigError::Invalid(format!(
                    "provider {name:?}: missing audience"
                )));
            }

            if let Some(existing) = issuers.insert(provider.issuer.as_str(), name.as_str()) {
                return Err(OidcConfigError::Invalid(format!(
                    "provider {name:?}: duplicate issuer already used by {existing:?}"
                )));
            }
        }

        Ok(())
    }

    pub fn provider_for_issuer(&self, issuer: &str) -> Option<ConfiguredOidcProvider<'_>> {
        self.providers
            .iter()
            .find(|(_, provider)| provider.issuer == issuer)
            .map(|(name, config)| ConfiguredOidcProvider { name, config })
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    fn valid_provider(issuer: &str) -> OidcProviderConfig {
        OidcProviderConfig {
            issuer: issuer.to_owned(),
            audience: "https://cache.example.com".to_owned(),
            bound_claims: BTreeMap::new(),
            bound_subject: Vec::new(),
        }
    }

    #[test]
    fn oidc_config_rejects_duplicate_issuers() {
        let config = OidcConfig {
            providers: BTreeMap::from([
                (
                    "one".to_owned(),
                    valid_provider("https://issuer.example.com"),
                ),
                (
                    "two".to_owned(),
                    valid_provider("https://issuer.example.com"),
                ),
            ]),
            allow_insecure: false,
        };

        assert!(config.validate().is_err());
    }

    #[test]
    fn oidc_config_rejects_missing_audience() {
        let config = OidcConfig {
            providers: BTreeMap::from([(
                "github".to_owned(),
                OidcProviderConfig {
                    issuer: "https://token.actions.githubusercontent.com".to_owned(),
                    audience: String::new(),
                    bound_claims: BTreeMap::new(),
                    bound_subject: Vec::new(),
                },
            )]),
            allow_insecure: false,
        };

        assert!(config.validate().is_err());
    }

    #[test]
    fn oidc_config_rejects_insecure_issuer_by_default() {
        let config = OidcConfig {
            providers: BTreeMap::from([(
                "github".to_owned(),
                valid_provider("http://token.actions.githubusercontent.com"),
            )]),
            allow_insecure: false,
        };

        assert!(config.validate().is_err());
    }

    #[test]
    fn oidc_config_load_from_path_round_trips() {
        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path().join("oidc.json");

        std::fs::write(
            &path,
            serde_json::to_string(&OidcConfig {
                providers: BTreeMap::from([(
                    "github".to_owned(),
                    OidcProviderConfig {
                        issuer: "https://token.actions.githubusercontent.com".to_owned(),
                        audience: "https://cache.example.com".to_owned(),
                        bound_claims: BTreeMap::from([(
                            "repository".to_owned(),
                            vec!["owner/repo".to_owned()],
                        )]),
                        bound_subject: vec!["repo:owner/repo:*".to_owned()],
                    },
                )]),
                allow_insecure: false,
            })
            .unwrap(),
        )
        .unwrap();

        let loaded = OidcConfig::load_from_path(&path).unwrap();

        assert!(loaded.providers.contains_key("github"));
        assert!(!loaded.allow_insecure);
    }
}
