//! `aidex skills list|show|enable|disable [--workspace W]`: the skills agents
//! can use, from the drive, the workspace, and enabled plugins.

use crate::{
    commands::daemon::connect,
    exit::{CliError, Exit},
    output::Output,
};
use aidex_skills::{SkillDetail, SkillSummary};
use aidexd::methods::skills::{LIST, SET_ENABLED, SHOW, SkillList};
use clap::{Args, Subcommand};
use serde_json::{Value, json};
use std::path::PathBuf;

/// Longest description shown in `aidex skills list`, in characters.
const LIST_DESCRIPTION_CHARS: usize = 72;

#[derive(Debug, Args)]
pub struct SkillsArgs {
    /// Drive root; defaults to AIDEX_DRIVE_ROOT from the portable launcher.
    #[arg(long, value_name = "DRIVE_PATH", global = true)]
    pub drive: Option<PathBuf>,
    /// Workspace id or name whose project and repo skills are included
    /// (without it only drive and plugin skills are listed).
    #[arg(long, value_name = "ID|NAME", global = true)]
    pub workspace: Option<String>,
    #[command(subcommand)]
    pub command: SkillsCommand,
}

#[derive(Debug, Subcommand)]
pub enum SkillsCommand {
    /// List skills with their scope and whether they are enabled.
    List,
    /// Show a skill's details, front matter, and (redacted) instructions.
    Show {
        /// Skill id, `<scope>:<name>` (e.g. `drive:review`).
        id: String,
    },
    /// Enable a skill (project and repo skills need --workspace).
    Enable {
        /// Skill id, `<scope>:<name>`.
        id: String,
    },
    /// Disable a skill.
    Disable {
        /// Skill id, `<scope>:<name>`.
        id: String,
    },
}

impl SkillsCommand {
    pub fn label(&self) -> &'static str {
        match self {
            Self::List => "aidex skills list",
            Self::Show { .. } => "aidex skills show",
            Self::Enable { .. } => "aidex skills enable",
            Self::Disable { .. } => "aidex skills disable",
        }
    }
}

/// Skill text is untrusted: replace control characters (other than line breaks
/// and tabs) so a skill cannot drive the terminal with escape sequences.
fn terminal_text(text: &str) -> String {
    text.chars()
        .map(|ch| {
            if ch.is_control() && ch != '\n' && ch != '\t' {
                '\u{fffd}'
            } else {
                ch
            }
        })
        .collect()
}

/// One line of at most `max` characters.
fn one_line(text: &str, max: usize) -> String {
    let line = terminal_text(&text.replace(['\n', '\t'], " "));
    if line.chars().count() <= max {
        return line;
    }
    let mut cut: String = line.chars().take(max.saturating_sub(3)).collect();
    cut.push_str("...");
    cut
}

fn state(summary: &SkillSummary) -> &'static str {
    match (summary.error.is_some(), summary.enabled) {
        (true, _) => "error",
        (false, true) => "enabled",
        (false, false) => "disabled",
    }
}

fn params(workspace: Option<&str>, mut params: Value) -> Value {
    if let Some(workspace) = workspace {
        params["workspace_id"] = json!(workspace);
    }
    params
}

fn print_list(list: &SkillList) {
    if list.skills.is_empty() {
        println!("No skills found.");
    }
    for skill in &list.skills {
        let text = skill.error.as_deref().unwrap_or(&skill.description);
        println!(
            "{:<32} {:<8} {}",
            skill.id,
            state(skill),
            one_line(text, LIST_DESCRIPTION_CHARS)
        );
    }
    for issue in &list.issues {
        println!(
            "warning: {} ({}): {}",
            terminal_text(&issue.location),
            issue.scope,
            one_line(&issue.error, 160)
        );
    }
    if list.truncated {
        println!("(more skills exist than fit in one listing)");
    }
}

fn print_detail(detail: &SkillDetail) {
    let summary = &detail.summary;
    println!("{} ({})", terminal_text(&summary.display_name), summary.id);
    println!("  state:       {}", state(summary));
    if !summary.description.is_empty() {
        println!("  description: {}", one_line(&summary.description, 1024));
    }
    if let Some(version) = &summary.version {
        println!("  version:     {}", terminal_text(version));
    }
    println!("  location:    {}", terminal_text(&summary.location));
    if let Some(plugin) = &summary.plugin {
        println!("  plugin:      {}", terminal_text(plugin));
    }
    println!(
        "  invocable:   user {}, model {}",
        if summary.user_invocable { "yes" } else { "no" },
        if summary.model_invocable { "yes" } else { "no" }
    );
    if let Some(error) = &summary.error {
        println!("  error:       {}", one_line(error, 1024));
    }
    if detail.body_redacted {
        println!("  note:        text that looks like a credential is hidden below");
    }
    if !detail.front_matter.is_empty() {
        println!("front matter:");
        for entry in &detail.front_matter {
            let value = serde_json::to_string(&entry.value).unwrap_or_default();
            println!(
                "  {}: {}",
                terminal_text(&entry.key),
                one_line(&value, 1024)
            );
        }
    }
    if let Some(body) = &detail.body {
        println!("---");
        println!("{}", terminal_text(body));
    }
}

pub fn run(args: SkillsArgs, output: Output) -> Result<Exit, CliError> {
    let mut client = connect(args.drive.as_deref())?;
    let workspace = args.workspace.as_deref();
    match args.command {
        SkillsCommand::List => {
            let list: SkillList = client.call_as(LIST, params(workspace, json!({})))?;
            output.emit(&list, || print_list(&list))?;
        }
        SkillsCommand::Show { id } => {
            let detail: SkillDetail =
                client.call_as(SHOW, params(workspace, json!({ "id": id })))?;
            output.emit(&detail, || print_detail(&detail))?;
        }
        SkillsCommand::Enable { id } => set_enabled(&mut client, output, workspace, &id, true)?,
        SkillsCommand::Disable { id } => {
            set_enabled(&mut client, output, workspace, &id, false)?;
        }
    }
    Ok(Exit::Success)
}

fn set_enabled(
    client: &mut aidexd::Client,
    output: Output,
    workspace: Option<&str>,
    id: &str,
    enabled: bool,
) -> Result<(), CliError> {
    let summary: SkillSummary = client.call_as(
        SET_ENABLED,
        params(workspace, json!({ "id": id, "enabled": enabled })),
    )?;
    output.emit(&summary, || {
        println!(
            "{} {}.",
            if summary.enabled {
                "Enabled"
            } else {
                "Disabled"
            },
            summary.id
        );
    })
}
