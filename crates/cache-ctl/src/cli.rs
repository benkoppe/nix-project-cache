use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "cache-ctl")]
#[command(about = "Manage nix-project-cache resources")]
pub struct Cli {
    #[arg(long, env = "CACHE_SERVER_URL", global = true)]
    pub server: String,

    #[arg(long, env = "CACHE_ADMIN_TOKEN", global = true)]
    pub auth_token: String,

    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    #[command(subcommand)]
    Projects(ProjectsCommand),
}

#[derive(Debug, Subcommand)]
pub enum ProjectsCommand {
    List,
    Create(CreateProjectCommand),

    #[command(subcommand)]
    Oidc(ProjectOidcCommand),
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
