use std::net::IpAddr;
use std::path::PathBuf;

/// mock-mesh: a high-throughput mock server driven by an OpenAPI spec.
#[derive(Debug, clap::Parser)]
#[command(name = "mock-mesh", version, about)]
pub struct Cli {
    /// Optional subcommand. With none, mock-mesh runs the server (default).
    #[command(subcommand)]
    pub command: Option<Command>,

    /// OpenAPI 3.0/3.1 spec file (JSON or YAML). Required to run the server
    /// (i.e. when no subcommand is given).
    #[arg(long, short, value_name = "PATH")]
    pub spec: Option<PathBuf>,

    /// mock-mesh behavior config file (JSON or YAML)
    #[arg(long, short, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Address to bind. Binding non-loopback without --admin-token exposes
    /// chaos controls to the network.
    #[arg(long, default_value = "127.0.0.1")]
    pub host: IpAddr,

    /// Port to bind (0 picks a free port)
    #[arg(long, short, default_value_t = 8080)]
    pub port: u16,

    /// Bearer token required for /_mockmesh admin endpoints
    #[arg(long, env = "MOCKMESH_ADMIN_TOKEN")]
    pub admin_token: Option<String>,

    /// Disable the /_mockmesh admin API entirely
    #[arg(long)]
    pub no_admin: bool,

    /// Disable file watching / hot reload
    #[arg(long)]
    pub no_watch: bool,

    /// Seed for deterministic fake-data generation (responses become
    /// byte-identical per endpoint across requests and restarts)
    #[arg(long)]
    pub seed: Option<u64>,

    /// Parse spec + config, print the route table, then exit
    #[arg(long)]
    pub validate: bool,

    /// Maximum accepted request body size in bytes
    #[arg(long, default_value_t = 1_048_576)]
    pub max_body_bytes: usize,

    /// Grace period in seconds for in-flight requests on shutdown
    #[arg(long, default_value_t = 10)]
    pub shutdown_grace_secs: u64,

    /// Log filter, e.g. "info" or "mock_mesh=debug" (RUST_LOG also works)
    #[arg(long, default_value = "info")]
    pub log: String,
}

#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Install or manage the bundled Claude Code skill.
    Skill {
        #[command(subcommand)]
        action: SkillAction,
    },
}

#[derive(Debug, clap::Subcommand)]
pub enum SkillAction {
    /// Write the mock-mesh skill into <dir>/.claude/skills/mock-mesh/, so an
    /// AI coding agent can drive mock-mesh on its own.
    Install(SkillInstallArgs),
}

#[derive(Debug, clap::Args)]
pub struct SkillInstallArgs {
    /// Repository root to install into (default: current directory)
    #[arg(long, value_name = "PATH")]
    pub dir: Option<PathBuf>,

    /// Overwrite existing skill files
    #[arg(long)]
    pub force: bool,

    /// Print the skill (SKILL.md) to stdout instead of writing any file
    #[arg(long)]
    pub print: bool,
}
