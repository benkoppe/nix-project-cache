use std::io::Write;

use anyhow::{Context as _, Result, bail};

use cache_api::{UpsertUpstreamRequest, UpstreamInfo};
use cache_client::CacheClient;
use cache_core::project::ProjectSlug;

use crate::cli::{
    LinkProjectUpstreamCommand, UpsertUpstreamCommand, UpstreamNameCommand, UpstreamsCommand,
};
use crate::output;

pub async fn handle(
    client: &CacheClient,
    writer: &mut impl Write,
    json_output: bool,
    command: UpstreamsCommand,
) -> Result<()> {
    match command {
        UpstreamsCommand::List => list_upstreams(client, writer, json_output).await,
        UpstreamsCommand::Upsert(command) => {
            upsert_upstream(client, writer, json_output, command).await
        }
        UpstreamsCommand::Enable(command) => {
            set_upstream_enabled(client, writer, json_output, command, true).await
        }
        UpstreamsCommand::Disable(command) => {
            set_upstream_enabled(client, writer, json_output, command, false).await
        }
        UpstreamsCommand::Link(command) => {
            link_project_upstream(client, writer, json_output, command).await
        }
        UpstreamsCommand::Unlink(command) => {
            unlink_project_upstream(client, writer, json_output, command).await
        }
    }
}

async fn list_upstreams(
    client: &CacheClient,
    writer: &mut impl Write,
    json_output: bool,
) -> Result<()> {
    let upstreams = client.list_upstreams().await.context("listing upstreams")?;
    print_upstreams(writer, json_output, &upstreams)
}

async fn upsert_upstream(
    client: &CacheClient,
    writer: &mut impl Write,
    json_output: bool,
    command: UpsertUpstreamCommand,
) -> Result<()> {
    if command.name.trim().is_empty() {
        bail!("upstream name must not be empty");
    }

    if command.url.trim().is_empty() {
        bail!("--url must not be empty");
    }

    client
        .upsert_upstream(UpsertUpstreamRequest {
            name: command.name.clone(),
            base_url: command.url.clone(),
            priority: command.priority,
            enabled: !command.disabled,
        })
        .await
        .with_context(|| format!("upserting upstream {}", command.name))?;

    if json_output {
        output::print_status_json(
            writer,
            "upserted",
            [
                ("name", serde_json::json!(command.name)),
                ("base_url", serde_json::json!(command.url)),
                ("priority", serde_json::json!(command.priority)),
                ("enabled", serde_json::json!(!command.disabled)),
            ],
        )?;
    } else {
        writeln!(writer, "upserted upstream {}", command.name)?;
    }
    Ok(())
}

async fn set_upstream_enabled(
    client: &CacheClient,
    writer: &mut impl Write,
    json_output: bool,
    command: UpstreamNameCommand,
    enabled: bool,
) -> Result<()> {
    let changed = client
        .set_upstream_enabled(&command.name, enabled)
        .await
        .with_context(|| format!("setting upstream {} enabled={enabled}", command.name))?;

    if !changed && !command.ignore_missing {
        bail!("upstream {} does not exist", command.name);
    }

    if json_output {
        output::print_status_json(
            writer,
            if changed { "updated" } else { "missing" },
            [
                ("name", serde_json::json!(command.name)),
                ("enabled", serde_json::json!(enabled)),
            ],
        )?;
    } else if changed {
        let action = if enabled { "enabled" } else { "disabled" };
        writeln!(writer, "{action} upstream {}", command.name)?;
    } else {
        writeln!(writer, "upstream {} was already absent", command.name)?;
    }

    Ok(())
}

async fn link_project_upstream(
    client: &CacheClient,
    writer: &mut impl Write,
    json_output: bool,
    command: LinkProjectUpstreamCommand,
) -> Result<()> {
    let project = parse_project_slug(&command.project)?;

    let linked = client
        .link_project_upstream(&project, &command.upstream)
        .await
        .with_context(|| {
            format!(
                "linking upstream {} to project {}",
                command.upstream,
                project.as_str()
            )
        })?;

    if !linked && !command.ignore_missing {
        bail!("upstream {} does not exist", command.upstream);
    }

    if json_output {
        output::print_status_json(
            writer,
            if linked { "linked" } else { "missing" },
            [
                ("project", serde_json::json!(project.as_str())),
                ("upstream", serde_json::json!(command.upstream)),
            ],
        )?;
    } else if linked {
        writeln!(
            writer,
            "linked upstream {} to project {}",
            command.upstream,
            project.as_str()
        )?;
    } else {
        writeln!(writer, "upstream {} was absent", command.upstream)?;
    }

    Ok(())
}

async fn unlink_project_upstream(
    client: &CacheClient,
    writer: &mut impl Write,
    json_output: bool,
    command: LinkProjectUpstreamCommand,
) -> Result<()> {
    let project = parse_project_slug(&command.project)?;

    let unlinked = client
        .unlink_project_upstream(&project, &command.upstream)
        .await
        .with_context(|| {
            format!(
                "unlinking upstream {} from project {}",
                command.upstream,
                project.as_str()
            )
        })?;

    if !unlinked && !command.ignore_missing {
        bail!(
            "upstream {} is not linked to project {}",
            command.upstream,
            project.as_str()
        );
    }

    if json_output {
        output::print_status_json(
            writer,
            if unlinked { "unlinked" } else { "missing" },
            [
                ("project", serde_json::json!(project.as_str())),
                ("upstream", serde_json::json!(command.upstream)),
            ],
        )?;
    } else if unlinked {
        writeln!(
            writer,
            "unlinked upstream {} from project {}",
            command.upstream,
            project.as_str()
        )?;
    } else {
        writeln!(
            writer,
            "upstream {} was not linked to project {}",
            command.upstream,
            project.as_str()
        )?;
    }

    Ok(())
}

fn parse_project_slug(slug: &str) -> Result<ProjectSlug> {
    ProjectSlug::parse(slug).map_err(|_| anyhow::anyhow!("invalid project slug {}", slug))
}

fn print_upstreams(
    writer: &mut impl Write,
    json_output: bool,
    upstreams: &[UpstreamInfo],
) -> Result<()> {
    if json_output {
        output::print_json(writer, upstreams)?;
    } else {
        for upstream in upstreams {
            writeln!(
                writer,
                "{}\t{}\tpriority={}\tenabled={}",
                upstream.name, upstream.base_url, upstream.priority, upstream.enabled
            )?;
        }
    }

    Ok(())
}
