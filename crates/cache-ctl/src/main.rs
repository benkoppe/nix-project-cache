mod cli;
mod gc;
mod output;
mod pins;
mod projects;
mod tokens;

use anyhow::{Context as _, Result};
use clap::Parser as _;

use cache_client::CacheClient;

use crate::cli::{Cli, Command};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .without_time()
        .with_target(false)
        .init();

    let Cli {
        server,
        auth_token,
        json,
        command,
    } = Cli::parse();

    let server =
        server.ok_or_else(|| anyhow::anyhow!("--server or CACHE_SERVER_URL is required"))?;

    let auth_token = auth_token
        .ok_or_else(|| anyhow::anyhow!("--auth-token or CACHE_ADMIN_TOKEN is required"))?;

    let client = CacheClient::new(&server, auth_token)
        .with_context(|| format!("creating client for {}", server))?;

    let mut stdout = std::io::stdout();

    match command {
        Command::Projects(command) => projects::handle(&client, &mut stdout, json, command).await,
        Command::Tokens(command) => tokens::handle(&client, &mut stdout, json, command).await,
        Command::Pins(command) => pins::handle(&client, &mut stdout, json, command).await,
        Command::Gc(command) => gc::handle(&client, &mut stdout, json, command).await,
    }
}
