use std::io::Write;

use anyhow::{Context as _, Result, bail};

use cache_api::{AccessTokenInfo, CreateAccessTokenRequest};
use cache_client::CacheClient;
use cache_core::project::ProjectSlug;

use crate::cli::{CreateTokenCommand, ListTokensCommand, RevokeTokenCommand, TokensCommand};
use crate::output;

pub async fn handle(
    client: &CacheClient,
    writer: &mut impl Write,
    json_output: bool,
    command: TokensCommand,
) -> Result<()> {
    match command {
        TokensCommand::List(command) => list_tokens(client, writer, json_output, command).await,
        TokensCommand::Create(command) => create_token(client, writer, json_output, command).await,
        TokensCommand::Revoke(command) => revoke_token(client, writer, json_output, command).await,
    }
}

async fn list_tokens(
    client: &CacheClient,
    writer: &mut impl Write,
    json_output: bool,
    command: ListTokensCommand,
) -> Result<()> {
    let project = command
        .project
        .as_deref()
        .map(parse_project_slug)
        .transpose()?;

    let tokens = client
        .list_access_tokens(project.as_ref())
        .await
        .context("listing access tokens")?;

    print_tokens(writer, json_output, &tokens)
}

async fn create_token(
    client: &CacheClient,
    writer: &mut impl Write,
    json_output: bool,
    command: CreateTokenCommand,
) -> Result<()> {
    if command.name.trim().is_empty() {
        bail!("token name must not be empty");
    }

    let project = parse_project_slug(&command.project)?;

    let response = client
        .create_access_token(CreateAccessTokenRequest {
            name: command.name,
            project: project.as_str().to_owned(),
            ref_patterns: command.ref_patterns,
            expires_at: command.expires_at,
        })
        .await
        .context("creating access token")?;

    if json_output {
        output::print_json(writer, &response)?;
    } else {
        writeln!(writer, "created token {}", response.info.id)?;
        writeln!(writer, "token: {}", response.token)?;
    }

    Ok(())
}

async fn revoke_token(
    client: &CacheClient,
    writer: &mut impl Write,
    json_output: bool,
    command: RevokeTokenCommand,
) -> Result<()> {
    let revoked = client
        .revoke_access_token(&command.token_id)
        .await
        .with_context(|| format!("revoking access token {}", command.token_id))?;

    if !revoked && !command.ignore_missing {
        bail!("access token {} does not exist", command.token_id);
    }

    if json_output {
        output::print_status_json(
            writer,
            if revoked { "revoked" } else { "missing" },
            [("token_id", serde_json::json!(command.token_id))],
        )?;
    } else if revoked {
        writeln!(writer, "revoked token {}", command.token_id)?;
    } else {
        writeln!(writer, "token {} was already absent", command.token_id)?;
    }

    Ok(())
}

fn parse_project_slug(slug: &str) -> Result<ProjectSlug> {
    ProjectSlug::parse(slug).map_err(|_| anyhow::anyhow!("invalid project slug {}", slug))
}

fn print_tokens(
    writer: &mut impl Write,
    json_output: bool,
    tokens: &[AccessTokenInfo],
) -> Result<()> {
    if json_output {
        output::print_json(writer, tokens)?;
    } else {
        for token in tokens {
            let refs = if token.ref_patterns.is_empty() {
                "*".to_owned()
            } else {
                token.ref_patterns.join(",")
            };

            writeln!(
                writer,
                "{}\t{}\t{}\trefs={}\trevoked={}",
                token.id,
                token.name,
                token.project,
                refs,
                token.revoked_at.is_some()
            )?;
        }
    }
    Ok(())
}
