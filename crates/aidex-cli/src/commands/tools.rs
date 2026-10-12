//! `aidex tools list`: the tools the broker mediates, with their risk metadata.

use crate::{
    commands::daemon::connect,
    exit::{CliError, Exit},
    output::Output,
};
use aidexd::methods::tools::LIST;
use clap::{Args, Subcommand};
use serde_json::{Value, json};
use std::path::PathBuf;

#[derive(Debug, Args)]
pub struct ToolsArgs {
    /// Drive root; defaults to AIDEX_DRIVE_ROOT from the portable launcher.
    #[arg(long, value_name = "DRIVE_PATH", global = true)]
    pub drive: Option<PathBuf>,
    #[command(subcommand)]
    pub command: ToolsCommand,
}

#[derive(Debug, Subcommand)]
pub enum ToolsCommand {
    /// List registered tools: action, base risk, and effects.
    List,
}

impl ToolsCommand {
    pub fn label(&self) -> &'static str {
        match self {
            Self::List => "aidex tools list",
        }
    }
}

fn effects(risk: &Value) -> String {
    let flags = [
        ("reads_workspace", "read"),
        ("writes_workspace", "write"),
        ("spawns_process", "process"),
        ("network", "network"),
        ("destructive", "destructive"),
        ("remote_write", "remote-write"),
    ];
    let shown: Vec<&str> = flags
        .iter()
        .filter(|(field, _)| risk[field] == true)
        .map(|(_, label)| *label)
        .collect();
    shown.join(",")
}

pub fn run(args: ToolsArgs, output: Output) -> Result<Exit, CliError> {
    match args.command {
        ToolsCommand::List => {
            let listed = connect(args.drive.as_deref())?.call(LIST, json!({}))?;
            output.emit(&listed, || {
                for tool in listed["tools"].as_array().into_iter().flatten() {
                    let risk = &tool["risk"];
                    println!(
                        "{:<12} {:<14} {:<7} {}",
                        tool["id"].as_str().unwrap_or("?"),
                        risk["action"].as_str().unwrap_or("?"),
                        risk["base_risk"].as_str().unwrap_or("?"),
                        effects(risk)
                    );
                }
            })?;
        }
    }
    Ok(Exit::Success)
}
