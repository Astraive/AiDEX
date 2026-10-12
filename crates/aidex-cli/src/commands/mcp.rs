//! `aidex mcp list|start|stop|tools|logs`: the drive's MCP servers through the
//! daemon's `mcp.*` methods. Servers are configured (and enabled) in the
//! desktop app or with `mcp.add`; their secrets live in the vault under
//! consumer `mcp:<id>`.

use crate::{
    commands::daemon::connect,
    exit::{CliError, Exit},
    output::Output,
};
use aidexd::methods::mcp::{LIST, LOGS, START, STOP, TOOLS};
use clap::{Args, Subcommand};
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};

/// Starting a server includes launching it, `initialize`, and the tool listing.
const START_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, Args)]
pub struct McpArgs {
    /// Drive root; defaults to AIDEX_DRIVE_ROOT from the portable launcher.
    #[arg(long, value_name = "DRIVE_PATH", global = true)]
    pub drive: Option<PathBuf>,
    #[command(subcommand)]
    pub command: McpCommand,
}

#[derive(Debug, Subcommand)]
pub enum McpCommand {
    /// List configured servers with their status and tool counts.
    List {
        /// Only servers offered in this workspace (id or name).
        #[arg(long, value_name = "WORKSPACE")]
        workspace: Option<String>,
    },
    /// Start an enabled server and list its tools.
    Start {
        /// Server id.
        id: String,
    },
    /// Stop a server.
    Stop {
        /// Server id.
        id: String,
    },
    /// List a server's tools (starts it when it is not running).
    Tools {
        /// Server id.
        id: String,
    },
    /// Show the newest lines of a server's log (stderr and lifecycle notes).
    Logs {
        /// Server id.
        id: String,
        /// Lines to show (1-400).
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
}

impl McpCommand {
    pub fn label(&self) -> &'static str {
        match self {
            Self::List { .. } => "aidex mcp list",
            Self::Start { .. } => "aidex mcp start",
            Self::Stop { .. } => "aidex mcp stop",
            Self::Tools { .. } => "aidex mcp tools",
            Self::Logs { .. } => "aidex mcp logs",
        }
    }
}

fn text<'a>(value: &'a Value, field: &str) -> &'a str {
    value[field].as_str().unwrap_or("")
}

fn print_server(server: &Value) {
    let enabled = if server["enabled"] == true {
        "enabled"
    } else {
        "disabled"
    };
    println!(
        "{:<24} {:<10} {:<8} {:<5} {:>3} tools  {}",
        text(server, "id"),
        text(server, "status"),
        enabled,
        text(server, "transport"),
        server["tool_count"].as_u64().unwrap_or(0),
        text(server, "endpoint"),
    );
    if let Some(error) = server.get("error").filter(|error| error.is_object()) {
        println!("    {}: {}", text(error, "reason"), text(error, "message"));
    }
}

/// The first line of a tool description, cut for one terminal row.
fn summary(description: &str) -> String {
    let line = description.lines().next().unwrap_or("");
    if line.chars().count() > 80 {
        format!("{}...", line.chars().take(77).collect::<String>())
    } else {
        line.to_owned()
    }
}

pub fn run(args: McpArgs, output: Output) -> Result<Exit, CliError> {
    let mut client = connect(args.drive.as_deref())?;
    match args.command {
        McpCommand::List { workspace } => {
            let params = match workspace {
                Some(workspace) => json!({ "workspace_id": workspace }),
                None => json!({}),
            };
            let listed = client.call(LIST, params)?;
            output.emit(&listed, || {
                let servers = listed["servers"].as_array().cloned().unwrap_or_default();
                if servers.is_empty() {
                    println!("No MCP servers are configured.");
                }
                for server in &servers {
                    print_server(server);
                }
                for invalid in listed["invalid"].as_array().into_iter().flatten() {
                    println!(
                        "{:<24} invalid    {}",
                        text(invalid, "id"),
                        text(invalid, "error")
                    );
                }
            })?;
        }
        McpCommand::Start { id } => {
            let server = client.call_with_timeout(START, json!({ "id": id }), START_TIMEOUT)?;
            output.emit(&server, || print_server(&server))?;
        }
        McpCommand::Stop { id } => {
            let server = client.call(STOP, json!({ "id": id }))?;
            output.emit(&server, || print_server(&server))?;
        }
        McpCommand::Tools { id } => {
            let listed = client.call_with_timeout(TOOLS, json!({ "id": id }), START_TIMEOUT)?;
            output.emit(&listed, || {
                for tool in listed["tools"].as_array().into_iter().flatten() {
                    println!(
                        "{:<32} {}",
                        text(tool, "name"),
                        summary(text(tool, "description"))
                    );
                }
                if listed["truncated"] == true {
                    println!(
                        "({} tools in total; use --json for the full list)",
                        listed["total"]
                    );
                }
                for note in listed["notes"].as_array().into_iter().flatten() {
                    println!("note: {}", note.as_str().unwrap_or(""));
                }
            })?;
        }
        McpCommand::Logs { id, limit } => {
            let logs = client.call(LOGS, json!({ "id": id, "limit": limit }))?;
            output.emit(&logs, || {
                for line in logs["lines"].as_array().into_iter().flatten() {
                    println!(
                        "{} {:<6} {}",
                        text(line, "at"),
                        text(line, "stream"),
                        text(line, "text")
                    );
                }
            })?;
        }
    }
    Ok(Exit::Success)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Commands};
    use clap::Parser;

    fn parse(args: &[&str]) -> McpCommand {
        let cli = Cli::try_parse_from(std::iter::once("aidex").chain(args.iter().copied()))
            .unwrap_or_else(|error| panic!("{args:?}: {error}"));
        let label = Commands::label(cli.command.as_ref());
        let Some(Commands::Mcp(args)) = cli.command else {
            panic!("expected mcp");
        };
        assert_eq!(label, args.command.label());
        args.command
    }

    #[test]
    fn subcommands_parse_and_label_themselves() {
        assert!(matches!(
            parse(&["mcp", "list", "--workspace", "proj"]),
            McpCommand::List { workspace: Some(w) } if w == "proj"
        ));
        assert!(
            matches!(parse(&["mcp", "start", "github"]), McpCommand::Start { id } if id == "github")
        );
        assert!(matches!(
            parse(&["mcp", "stop", "github"]),
            McpCommand::Stop { .. }
        ));
        assert!(matches!(
            parse(&["mcp", "tools", "github"]),
            McpCommand::Tools { .. }
        ));
        assert!(matches!(
            parse(&["mcp", "logs", "github", "--limit", "5"]),
            McpCommand::Logs { limit: 5, .. }
        ));
        assert_eq!(parse(&["mcp", "list"]).label(), "aidex mcp list");
        assert!(Cli::try_parse_from(["aidex", "mcp", "start"]).is_err());
        assert_eq!(summary(&"x".repeat(100)).chars().count(), 80);
        assert_eq!(summary("first\nsecond"), "first");
    }
}
