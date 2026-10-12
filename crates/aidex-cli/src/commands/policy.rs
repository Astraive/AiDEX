//! `aidex policy`: show the broker's active policy, change the profile, reload
//! policy documents, and explain what a tool call would be allowed to do.

use crate::{
    commands::daemon::connect,
    exit::{CliError, Exit},
    output::Output,
};
use aidexd::methods::policy::{EXPLAIN, PROFILE_SET, RELOAD, SHOW};
use clap::{Args, Subcommand};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Debug, Args)]
pub struct PolicyArgs {
    /// Drive root; defaults to AIDEX_DRIVE_ROOT from the portable launcher.
    #[arg(long, value_name = "DRIVE_PATH", global = true)]
    pub drive: Option<PathBuf>,
    #[command(subcommand)]
    pub command: PolicyCommand,
}

#[derive(Debug, Subcommand)]
pub enum PolicyCommand {
    /// Show the active profile, privacy, host trust, network mode, documents, and
    /// hard rules.
    Show,
    /// Switch the policy profile (safe, balanced, autonomous, offline-secure).
    Profile {
        #[arg(value_name = "PROFILE")]
        profile: String,
    },
    /// Re-read the policy documents in the drive's `policies/` directory.
    Reload,
    /// Dry run: plan a tool call and show the decision without running it.
    Explain {
        /// Tool id, e.g. `fs.write` (see `aidex tools list`).
        #[arg(long)]
        tool: String,
        /// Tool input as a JSON object.
        #[arg(long, default_value = "{}")]
        input: String,
        /// Workspace id or name (default: the selected workspace).
        #[arg(long)]
        workspace: Option<String>,
        /// Session id whose session-layer rules and grants apply.
        #[arg(long)]
        session: Option<String>,
        /// Evaluate as the local user instead of an agent.
        #[arg(long)]
        as_user: bool,
    },
}

impl PolicyCommand {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Show => "aidex policy show",
            Self::Profile { .. } => "aidex policy profile",
            Self::Reload => "aidex policy reload",
            Self::Explain { .. } => "aidex policy explain",
        }
    }
}

fn call(drive: Option<&Path>, method: &str, params: Value) -> Result<Value, CliError> {
    Ok(connect(drive)?.call(method, params)?)
}

fn text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => "-".into(),
        other => other.to_string(),
    }
}

fn print_status(status: &Value) {
    let settings = &status["settings"];
    println!(
        "Profile:      {} (effective {})",
        text(&settings["profile"]),
        text(&status["effective_profile"])
    );
    println!(
        "Privacy:      {} (effective {})",
        text(&settings["privacy"]),
        text(&status["effective_privacy"])
    );
    println!("Host trust:   {}", text(&settings["host_trust"]));
    println!(
        "Network:      {} (effective {})",
        text(&settings["network_mode"]),
        text(&status["effective_network_mode"])
    );
    println!(
        "Documents:    {} rule(s) from {}",
        text(&status["document_rules"]),
        text(&status["policies_dir"])
    );
    if let Some(error) = status["document_error"].as_str() {
        println!("              FAILED TO LOAD (every call is denied): {error}");
    }
    if let Some(reason) = status["emergency_stop"].as_str() {
        println!("Emergency:    STOPPED ({reason}); resume with `aidex resume`");
    }
    println!("Hard rules:");
    for rule in status["hard_rules"].as_array().into_iter().flatten() {
        println!("  {:<36} {}", text(&rule["id"]), text(&rule["verdict"]));
    }
}

fn verdict(verdict: &Value) -> String {
    match verdict["kind"].as_str() {
        Some("sandbox") => format!("sandbox ({})", text(&verdict["level"])),
        Some("require_approval") => {
            let scopes: Vec<String> = verdict["scope_options"]
                .as_array()
                .into_iter()
                .flatten()
                .map(text)
                .collect();
            format!(
                "require approval ({}; expires in {} s)",
                scopes.join(" or "),
                text(&verdict["expires_in_secs"])
            )
        }
        Some("rate_limit") => format!(
            "rate limit ({} per {} s)",
            text(&verdict["max"]),
            text(&verdict["per_secs"])
        ),
        Some(kind) => kind.replace('_', " "),
        None => text(verdict),
    }
}

fn print_explanation(explanation: &Value) {
    let decision = &explanation["decision"];
    println!("Tool:      {}", text(&explanation["tool_id"]));
    println!("Request:   {}", text(&explanation["summary"]));
    println!("Effect:    {}", text(&explanation["effect"]));
    println!("Risk:      {}", text(&explanation["risk"]));
    println!("Decision:  {}", verdict(&decision["verdict"]));
    println!(
        "Rule:      {} ({})",
        text(&decision["rule"]),
        text(&decision["reason"])
    );
    println!("Sandbox:   at least {}", text(&decision["sandbox"]));
    if let Some(grant) = explanation["session_grant"].as_str() {
        println!("Grant:     session grant {grant} would allow it");
    }
    if let Some(reason) = explanation["sandbox_unavailable"].as_str() {
        println!("Sandbox:   UNAVAILABLE here, so it would be denied: {reason}");
    }
    for intent in explanation["intents"].as_array().into_iter().flatten() {
        println!(
            "  {:<16} {:<28} {}",
            text(&intent["action"]),
            verdict(&intent["decision"]["verdict"]),
            text(&intent["decision"]["rule"])
        );
    }
    for warning in explanation["warnings"].as_array().into_iter().flatten() {
        println!("Warning:   {}", text(warning));
    }
}

pub fn run(args: PolicyArgs, output: Output) -> Result<Exit, CliError> {
    let drive = args.drive.as_deref();
    match args.command {
        PolicyCommand::Show => {
            let status = call(drive, SHOW, json!({}))?;
            output.emit(&status, || print_status(&status))?;
        }
        PolicyCommand::Profile { profile } => {
            let status = call(drive, PROFILE_SET, json!({"profile": profile}))?;
            output.emit(&status, || {
                println!(
                    "Policy profile is now {}",
                    text(&status["settings"]["profile"])
                );
            })?;
        }
        PolicyCommand::Reload => {
            let status = call(drive, RELOAD, json!({}))?;
            output.emit(&status, || print_status(&status))?;
        }
        PolicyCommand::Explain {
            tool,
            input,
            workspace,
            session,
            as_user,
        } => {
            let input: Value = serde_json::from_str(&input).map_err(|error| {
                CliError::new(Exit::Usage, format!("--input is not valid JSON: {error}"))
            })?;
            let mut params = json!({
                "tool": tool,
                "input": input,
                "origin": if as_user { "user" } else { "agent" },
            });
            if let Some(workspace) = workspace {
                params["workspace"] = json!(workspace);
            }
            if let Some(session) = session {
                params["session"] = json!(session);
            }
            let explanation = call(drive, EXPLAIN, params)?;
            output.emit(&explanation, || print_explanation(&explanation))?;
        }
    }
    Ok(Exit::Success)
}
