use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(name = "cache-ctl")]
#[command(about = "Manage nix-project-cache resources")]
pub struct Cli {
    #[arg(long, env = "CACHE_SERVER_URL", global = true)]
    pub server: Option<String>,

    #[arg(long, env = "CACHE_ADMIN_TOKEN", global = true)]
    pub auth_token: Option<String>,

    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    #[command(subcommand)]
    Projects(ProjectsCommand),

    #[command(subcommand)]
    Tokens(TokensCommand),

    #[command(subcommand)]
    Pins(PinsCommand),

    #[command(subcommand)]
    Upstreams(UpstreamsCommand),

    #[command(subcommand)]
    Gc(GcCommand),
}

#[derive(Debug, Subcommand)]
pub enum ProjectsCommand {
    List,
    Create(CreateProjectCommand),

    #[command(subcommand)]
    Oidc(ProjectOidcCommand),

    #[command(subcommand)]
    Retention(ProjectRetentionCommand),
}

#[derive(Debug, Parser)]
pub struct CreateProjectCommand {
    pub slug: String,

    #[arg(long)]
    pub display_name: Option<String>,

    #[arg(long)]
    pub public: bool,

    #[arg(long)]
    pub if_not_exists: bool,
}

#[derive(Debug, Subcommand)]
pub enum ProjectOidcCommand {
    List(ListProjectOidcCommand),
    Add(AddProjectOidcCommand),
    Remove(RemoveProjectOidcCommand),
}

#[derive(Debug, Parser)]
pub struct ListProjectOidcCommand {
    pub project: String,
}

#[derive(Debug, Parser)]
pub struct AddProjectOidcCommand {
    pub project: String,

    #[arg(long)]
    pub provider: String,

    #[arg(long)]
    pub repository: String,

    #[arg(long = "ref")]
    pub ref_patterns: Vec<String>,

    #[arg(long)]
    pub if_not_exists: bool,
}

#[derive(Debug, Parser)]
pub struct RemoveProjectOidcCommand {
    pub project: String,

    #[arg(long)]
    pub provider: String,

    #[arg(long)]
    pub repository: String,

    #[arg(long)]
    pub ignore_missing: bool,
}

#[derive(Debug, Subcommand)]
pub enum ProjectRetentionCommand {
    Get(GetProjectRetentionCommand),
    Set(SetProjectRetentionCommand),
    Reset(ResetProjectRetentionCommand),
}

#[derive(Debug, Parser)]
pub struct GetProjectRetentionCommand {
    pub project: String,
}

#[derive(Debug, Parser)]
pub struct SetProjectRetentionCommand {
    pub project: String,

    #[arg(long, value_enum)]
    pub profile: Option<RetentionProfile>,

    #[arg(long)]
    pub keep_builds: Option<u32>,

    #[arg(long)]
    pub object_delete_grace: Option<String>,

    #[arg(long = "rule")]
    pub rules: Vec<String>,
}

#[derive(Debug, Parser)]
pub struct ResetProjectRetentionCommand {
    pub project: String,

    #[arg(long)]
    pub ignore_missing: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum RetentionProfile {
    Aggressive,
    Balanced,
    Conservative,
}

#[derive(Debug, Subcommand)]
pub enum TokensCommand {
    List(ListTokensCommand),
    Create(CreateTokenCommand),
    Revoke(RevokeTokenCommand),
}

#[derive(Debug, Parser)]
pub struct ListTokensCommand {
    #[arg(long)]
    pub project: Option<String>,
}

#[derive(Debug, Parser)]
pub struct CreateTokenCommand {
    pub name: String,

    #[arg(long)]
    pub project: String,

    #[arg(long = "ref")]
    pub ref_patterns: Vec<String>,

    #[arg(long)]
    pub expires_at: Option<String>,
}

#[derive(Debug, Parser)]
pub struct RevokeTokenCommand {
    pub token_id: String,

    #[arg(long)]
    pub ignore_missing: bool,
}

#[derive(Debug, Subcommand)]
pub enum PinsCommand {
    List(ListPinsCommand),
    Create(CreatePinCommand),
    Delete(DeletePinCommand),
}

#[derive(Debug, Parser)]
pub struct ListPinsCommand {
    #[arg(long)]
    pub project: Option<String>,
}

#[derive(Debug, Parser)]
pub struct CreatePinCommand {
    pub name: String,

    #[arg(long)]
    pub store_path: String,

    #[arg(long)]
    pub project: Option<String>,
}

#[derive(Debug, Parser)]
pub struct DeletePinCommand {
    pub name: String,

    #[arg(long)]
    pub project: Option<String>,

    #[arg(long)]
    pub ignore_missing: bool,
}

#[derive(Debug, Subcommand)]
pub enum UpstreamsCommand {
    List,
    Upsert(UpsertUpstreamCommand),
    Enable(UpstreamNameCommand),
    Disable(UpstreamNameCommand),
    Link(LinkProjectUpstreamCommand),
    Unlink(LinkProjectUpstreamCommand),
}

#[derive(Debug, Parser)]
pub struct UpsertUpstreamCommand {
    pub name: String,

    #[arg(long)]
    pub url: String,

    #[arg(long, default_value_t = 50)]
    pub priority: u32,

    #[arg(long)]
    pub disabled: bool,
}

#[derive(Debug, Parser)]
pub struct UpstreamNameCommand {
    pub name: String,

    #[arg(long)]
    pub ignore_missing: bool,
}

#[derive(Debug, Parser)]
pub struct LinkProjectUpstreamCommand {
    pub project: String,
    pub upstream: String,

    #[arg(long)]
    pub ignore_missing: bool,
}

#[derive(Debug, Subcommand)]
pub enum GcCommand {
    Run(RunGcCommand),
}

#[derive(Debug, Parser)]
pub struct RunGcCommand {
    #[arg(long)]
    pub dry_run: bool,

    #[arg(long)]
    pub grace_period_seconds: Option<u64>,
}
