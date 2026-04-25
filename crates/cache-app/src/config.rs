use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use figment::{
    Figment,
    providers::{Env, Format, Serialized, Toml},
};
use serde::{Deserialize, Serialize};

use cache_auth::OidcConfig;
use cache_core::key_crypto::KeyEncryptionKey;
use cache_core::nix::StoreDir;
use cache_core::signing::NamedSigningKey;

use crate::storage::{RawStorageConfig, StorageConfig};

#[derive(Clone)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub nix: NixConfig,
    pub logging: LoggingConfig,
    pub auth: AuthConfig,
    pub signing: SigningConfig,
    pub storage: StorageConfig,
}

#[derive(Clone)]
pub struct ServerConfig {
    pub bind_address: String,
}

#[derive(Clone)]
pub struct DatabaseConfig {
    pub path: PathBuf,
}

#[derive(Clone)]
pub struct NixConfig {
    pub store_dir: StoreDir,
}

#[derive(Clone)]
pub struct LoggingConfig {
    pub filter: String,
}

#[derive(Clone)]
pub struct AuthConfig {
    pub write_token: Option<String>,
    pub oidc: Option<OidcConfig>,
}

#[derive(Clone)]
pub struct SigningConfig {
    pub aggregate_signing_key: Option<NamedSigningKey>,
    pub project_key_encryption_key: Option<KeyEncryptionKey>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct RawAppConfig {
    server: RawServerConfig,
    database: RawDatabaseConfig,
    nix: RawNixConfig,
    logging: RawLoggingConfig,
    auth: RawAuthConfig,
    signing: RawSigningConfig,
    storage: RawStorageConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct RawServerConfig {
    bind_address: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct RawDatabaseConfig {
    path: PathBuf,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct RawNixConfig {
    store_dir: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct RawLoggingConfig {
    filter: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(default, deny_unknown_fields)]
struct RawAuthConfig {
    write_token: Option<String>,
    oidc_config_file: Option<PathBuf>,
    oidc: Option<OidcConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(default, deny_unknown_fields)]
struct RawSigningConfig {
    aggregate_key_file: Option<PathBuf>,
    project_key_encryption_key: Option<String>,
}

impl Default for RawServerConfig {
    fn default() -> Self {
        Self {
            bind_address: "127.0.0.1:8080".to_owned(),
        }
    }
}

impl Default for RawDatabaseConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from("./cache_db/cache.sqlite"),
        }
    }
}

impl Default for RawNixConfig {
    fn default() -> Self {
        Self {
            store_dir: "/nix/store".to_owned(),
        }
    }
}

impl Default for RawLoggingConfig {
    fn default() -> Self {
        Self {
            filter: "cache_app=info,cache_read=info".to_owned(),
        }
    }
}

impl AppConfig {
    pub fn load(config_path: Option<&Path>) -> Result<Self> {
        let mut figment = Figment::new().merge(Serialized::defaults(RawAppConfig::default()));

        if let Some(path) = config_path {
            figment = figment.merge(Toml::file(path));
        }

        figment = figment.merge(Env::prefixed("CACHE_APP_").split("__"));

        Self::from_figment(figment)
    }

    pub fn from_figment(figment: Figment) -> Result<Self> {
        let raw = figment
            .extract::<RawAppConfig>()
            .context("extracting app config")?;

        raw.try_into()
    }
}

impl TryFrom<RawAppConfig> for AppConfig {
    type Error = anyhow::Error;

    fn try_from(raw: RawAppConfig) -> Result<Self> {
        Ok(Self {
            server: raw.server.into(),
            database: raw.database.into(),
            nix: raw.nix.try_into()?,
            logging: raw.logging.into(),
            auth: raw.auth.try_into()?,
            signing: raw.signing.try_into()?,
            storage: raw.storage.try_into()?,
        })
    }
}

impl From<RawServerConfig> for ServerConfig {
    fn from(raw: RawServerConfig) -> Self {
        Self {
            bind_address: raw.bind_address,
        }
    }
}

impl From<RawDatabaseConfig> for DatabaseConfig {
    fn from(raw: RawDatabaseConfig) -> Self {
        Self { path: raw.path }
    }
}

impl TryFrom<RawNixConfig> for NixConfig {
    type Error = anyhow::Error;

    fn try_from(raw: RawNixConfig) -> Result<Self> {
        let store_dir = StoreDir::new(raw.store_dir)
            .map_err(anyhow::Error::new)
            .context("parsing nix.store_dir")?;
        Ok(Self { store_dir })
    }
}

impl From<RawLoggingConfig> for LoggingConfig {
    fn from(raw: RawLoggingConfig) -> Self {
        Self { filter: raw.filter }
    }
}

impl TryFrom<RawAuthConfig> for AuthConfig {
    type Error = anyhow::Error;

    fn try_from(raw: RawAuthConfig) -> Result<Self> {
        let oidc_config_file = non_empty_path(raw.oidc_config_file);

        if oidc_config_file.is_some() && raw.oidc.is_some() {
            bail!("configure either auth.oidc_config_file or auth.oidc, but not both");
        }

        let oidc = match (oidc_config_file, raw.oidc) {
            (Some(path), None) => Some(
                OidcConfig::load_from_path(&path)
                    .with_context(|| format!("loading OIDC config from {}", path.display()))?,
            ),
            (None, Some(oidc)) => {
                oidc.validate().context("validating auth.oidc")?;
                Some(oidc)
            }
            (None, None) => None,
            (Some(_), Some(_)) => unreachable!(),
        };

        Ok(Self {
            write_token: non_empty_option(raw.write_token),
            oidc,
        })
    }
}

impl TryFrom<RawSigningConfig> for SigningConfig {
    type Error = anyhow::Error;

    fn try_from(raw: RawSigningConfig) -> Result<Self> {
        Ok(Self {
            aggregate_signing_key: load_aggregate_signing_key(raw.aggregate_key_file.as_deref())?,
            project_key_encryption_key: non_empty_option(raw.project_key_encryption_key)
                .map(|value| KeyEncryptionKey::parse_base64(&value).map_err(anyhow::Error::new))
                .transpose()
                .context("parsing signing.project_key_encryption_key")?,
        })
    }
}

fn load_aggregate_signing_key(path: Option<&Path>) -> Result<Option<NamedSigningKey>> {
    let Some(path) = path else {
        return Ok(None);
    };

    let raw =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;

    let signing_key = NamedSigningKey::parse(raw.trim())
        .map_err(anyhow::Error::new)
        .with_context(|| format!("parsing aggregate signing key from {}", path.display()))?;

    Ok(Some(signing_key))
}

fn non_empty_option(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn non_empty_path(path: Option<PathBuf>) -> Option<PathBuf> {
    path.filter(|path| !path.as_os_str().is_empty())
}
