//! CLI argument parsing for flok.

use clap::{Parser, Subcommand};

/// An AI coding agent for the terminal.
#[derive(Debug, Parser)]
#[command(name = "flok", version, about)]
pub(crate) struct Args {
    #[command(subcommand)]
    pub command: Option<Command>,

    // Top-level flags that work without a subcommand (for backwards compat)
    /// Continue a previous session by ID.
    #[arg(short, long, alias = "resume", global = true)]
    pub session: Option<String>,

    /// Send a single prompt and exit (non-interactive mode).
    #[arg(short, long, global = true)]
    pub prompt: Option<String>,

    /// Override the model to use.
    #[arg(short, long, global = true)]
    pub model: Option<String>,

    /// Working directory (defaults to current directory).
    #[arg(short = 'd', long, global = true)]
    pub workdir: Option<std::path::PathBuf>,

    /// Start in plan mode (read-only, no file modifications).
    #[arg(long, global = true)]
    pub plan: bool,

    /// Enable debug logging to /tmp/flok.log.
    #[arg(long, global = true)]
    pub debug: bool,

    /// Disable alternate screen mode for the TUI.
    ///
    /// Runs inline in the normal terminal buffer, which preserves scrollback.
    #[arg(long, global = true)]
    pub no_alt_screen: bool,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// List available models and their pricing.
    Models,

    /// List past sessions.
    Sessions {
        /// Show sessions for a specific project path.
        #[arg(long)]
        project: Option<String>,

        /// Maximum number of sessions to show.
        #[arg(short = 'n', long, default_value = "20")]
        limit: usize,
    },

    /// Show version and build info.
    Version,

    /// Manage authentication for LLM providers.
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },

    /// Manage MCP server configuration.
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum AuthCommand {
    /// Login to a provider — save an API key to your config file.
    Login {
        /// Provider to authenticate with. If omitted, you'll be prompted to choose.
        #[arg(long)]
        provider: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum McpCommand {
    /// Add or update an MCP server entry in user config.
    Add {
        /// Logical MCP server name, e.g. `github`.
        name: String,

        /// Remote MCP endpoint URL.
        #[arg(long, conflicts_with = "command", required_unless_present = "command")]
        url: Option<String>,

        /// Stdio command for a local MCP server.
        #[arg(long, conflicts_with = "url", required_unless_present = "url")]
        command: Option<String>,

        /// Repeated argument for `--command` stdio servers.
        #[arg(long = "arg")]
        args: Vec<String>,

        /// Working directory for stdio MCP servers.
        #[arg(long)]
        cwd: Option<std::path::PathBuf>,

        /// Env var holding a bearer token for remote MCP servers.
        #[arg(long)]
        bearer_token_env_var: Option<String>,

        /// Per-server tool timeout in seconds.
        #[arg(long)]
        timeout_seconds: Option<u64>,

        /// Add the entry in disabled state.
        #[arg(long)]
        disabled: bool,
    },
}

impl Args {
    /// Parse CLI arguments.
    pub(crate) fn parse_args() -> Self {
        Self::parse()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_args_parse() {
        let args = Args::try_parse_from(["flok"]).unwrap();
        assert!(args.session.is_none());
        assert!(args.prompt.is_none());
        assert!(args.model.is_none());
        assert!(args.command.is_none());
        assert!(!args.no_alt_screen);
    }

    #[test]
    fn parse_prompt_flag() {
        let args = Args::try_parse_from(["flok", "--prompt", "hello world"]).unwrap();
        assert_eq!(args.prompt.as_deref(), Some("hello world"));
    }

    #[test]
    fn parse_models_subcommand() {
        let args = Args::try_parse_from(["flok", "models"]).unwrap();
        assert!(matches!(args.command, Some(Command::Models)));
    }

    #[test]
    fn parse_sessions_subcommand() {
        let args = Args::try_parse_from(["flok", "sessions", "--limit", "10"]).unwrap();
        assert!(matches!(args.command, Some(Command::Sessions { limit: 10, .. })));
    }

    #[test]
    fn parse_resume_flag() {
        let args = Args::try_parse_from(["flok", "--resume", "abc123"]).unwrap();
        assert_eq!(args.session.as_deref(), Some("abc123"));
    }

    #[test]
    fn parse_session_flag() {
        let args = Args::try_parse_from(["flok", "--session", "abc123"]).unwrap();
        assert_eq!(args.session.as_deref(), Some("abc123"));
    }

    #[test]
    fn parse_no_alt_screen_flag() {
        let args = Args::try_parse_from(["flok", "--no-alt-screen"]).unwrap();
        assert!(args.no_alt_screen);
    }

    #[test]
    fn parse_mcp_add_remote_subcommand() {
        let args = Args::try_parse_from([
            "flok",
            "mcp",
            "add",
            "github",
            "--url",
            "https://api.githubcopilot.com/mcp/",
            "--bearer-token-env-var",
            "GITHUB_PAT_TOKEN",
        ])
        .unwrap();

        match args.command {
            Some(Command::Mcp {
                command: McpCommand::Add { name, url, bearer_token_env_var, .. },
            }) => {
                assert_eq!(name, "github");
                assert_eq!(url.as_deref(), Some("https://api.githubcopilot.com/mcp/"));
                assert_eq!(bearer_token_env_var.as_deref(), Some("GITHUB_PAT_TOKEN"));
            }
            other => panic!("unexpected command parse: {other:?}"),
        }
    }
}
