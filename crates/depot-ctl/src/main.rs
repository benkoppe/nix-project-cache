mod cli;
mod gc;
mod keys;
mod output;
mod pins;
mod projects;
mod tokens;
mod upstreams;

use anyhow::{Context as _, Result};
use clap::Parser as _;

use depot_client::DepotClient;

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

    let mut stdout = std::io::stdout();

    if let Command::Keys(command) = command {
        return keys::handle(&mut stdout, json, command).await;
    }

    let server =
        server.ok_or_else(|| anyhow::anyhow!("--server or DEPOT_SERVER_URL is required"))?;

    let auth_token = auth_token
        .ok_or_else(|| anyhow::anyhow!("--auth-token or DEPOT_ADMIN_TOKEN is required"))?;

    let client = DepotClient::new(&server, auth_token)
        .with_context(|| format!("creating client for {}", server))?;

    match command {
        Command::Projects(command) => projects::handle(&client, &mut stdout, json, command).await,
        Command::Tokens(command) => tokens::handle(&client, &mut stdout, json, command).await,
        Command::Pins(command) => pins::handle(&client, &mut stdout, json, command).await,
        Command::Upstreams(command) => upstreams::handle(&client, &mut stdout, json, command).await,
        Command::Gc(command) => gc::handle(&client, &mut stdout, json, command).await,
        Command::Keys(_) => unreachable!("keys command handled before client construction"),
    }
}
