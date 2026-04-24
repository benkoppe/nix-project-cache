use anyhow::{Result, bail};
use clap::Parser;

#[derive(Debug, Clone, Parser)]
#[command(name = "cache-push")]
#[command(about = "Push Nix store paths into the cache")]
pub struct Cli {
    #[arg(long, env = "CACHE_SERVER_URL")]
    pub server: String,

    #[arg(long, env = "CACHE_WRITE_TOKEN")]
    pub auth_token: Option<String>,

    #[arg(long, env = "CACHE_OIDC_AUDIENCE")]
    pub oidc_audience: Option<String>,

    #[arg(long)]
    pub project: String,

    #[arg(long = "ref")]
    pub ref_name: Option<String>,

    #[arg(long)]
    pub revision: Option<String>,

    #[arg(long)]
    pub pin: Option<String>,

    #[arg(long, default_value_t = 4)]
    pub max_concurrent_uploads: usize,

    #[arg(required = true)]
    pub paths: Vec<String>,
}

impl Cli {
    pub fn validate(&self) -> Result<()> {
        if self.paths.is_empty() {
            bail!("at least one store path is required");
        }

        if self.pin.is_some() && self.paths.len() != 1 {
            bail!("--pin requires exactly one top-level store path");
        }

        Ok(())
    }
}
