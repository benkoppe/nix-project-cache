mod cli;
mod output;
mod projects;

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

    let client = CacheClient::new(&server, auth_token)
        .with_context(|| format!("creating client for {}", server))?;

    let mut stdout = std::io::stdout();

    match command {
        Command::Projects(command) => projects::handle(&client, &mut stdout, json, command).await,
    }
}
