//! `aidex workspace ephemeral create|review|apply|discard`.
//!
//! `apply` always fetches a fresh review, shows what will be written, asks for
//! confirmation (or `--yes`), and sends the review digest so the daemon refuses
//! the apply if the workspace changed in between. Without a terminal and without
//! `--yes`, destructive commands refuse (exit 2) and change nothing.

use super::render;
use crate::{
    commands::daemon::connect,
    exit::{CliError, Exit},
    output::Output,
};
use aidex_core::WorkspacePath;
use aidex_workspace::{ApplyReport, EphemeralCreated, Workspace};
use aidexd::methods::workspace::{
    ApplyParams, Discarded, EPHEMERAL_APPLY, EPHEMERAL_CREATE, EPHEMERAL_DISCARD, EPHEMERAL_REVIEW,
    EphemeralReview, GET,
};
use clap::Subcommand;
use serde_json::json;
use std::{
    io::{self, BufRead, IsTerminal, Write},
    path::Path,
};

#[derive(Debug, Subcommand)]
pub enum EphemeralCommand {
    /// Copy a workspace into a new ephemeral workspace (honours .gitignore).
    Create {
        /// Source workspace id or name.
        source: String,
    },
    /// Show the changes made in an ephemeral workspace, with conflicts.
    Review {
        /// Exact ephemeral workspace id.
        id: String,
    },
    /// Apply reviewed changes to the source workspace.
    Apply {
        /// Exact ephemeral workspace id.
        id: String,
        /// Apply only this workspace-relative path (repeatable; default: all).
        #[arg(long = "path", value_name = "PATH")]
        paths: Vec<String>,
        /// Apply without an interactive confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Delete an ephemeral workspace and its AiDEX-created files.
    Discard {
        /// Exact ephemeral workspace id.
        id: String,
        /// Discard without an interactive confirmation.
        #[arg(long)]
        yes: bool,
    },
}

impl EphemeralCommand {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Create { .. } => "aidex workspace ephemeral create",
            Self::Review { .. } => "aidex workspace ephemeral review",
            Self::Apply { .. } => "aidex workspace ephemeral apply",
            Self::Discard { .. } => "aidex workspace ephemeral discard",
        }
    }
}

pub fn run(
    drive: Option<&Path>,
    command: EphemeralCommand,
    output: Output,
) -> Result<Exit, CliError> {
    match command {
        EphemeralCommand::Create { source } => create(drive, &source, output),
        EphemeralCommand::Review { id } => review(drive, &id, output),
        EphemeralCommand::Apply { id, paths, yes } => apply(drive, &id, &paths, yes, output),
        EphemeralCommand::Discard { id, yes } => discard(drive, &id, yes, output),
    }
}

/// Ask `question` on stderr; `--yes` skips it. JSON mode and non-terminals refuse.
pub fn confirm(question: &str, yes: bool, output: Output) -> Result<bool, CliError> {
    if yes {
        return Ok(true);
    }
    let stdin = io::stdin();
    if output.is_json() || !stdin.is_terminal() || !io::stderr().is_terminal() {
        return Err(CliError::new(
            Exit::Usage,
            "refusing to continue without confirmation; nothing was changed",
        )
        .with_hint("re-run with `--yes` after reviewing the changes"));
    }
    eprint!("{question} [y/N] ");
    let _ = io::stderr().flush();
    let mut answer = String::new();
    stdin.lock().read_line(&mut answer).map_err(|error| {
        CliError::new(Exit::Failure, format!("cannot read the answer: {error}"))
    })?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

/// Report a declined confirmation.
pub fn cancelled(output: Output) -> Exit {
    if !output.is_json() {
        eprintln!("Cancelled; nothing was changed.");
    }
    Exit::Cancelled
}

fn create(drive: Option<&Path>, source: &str, output: Output) -> Result<Exit, CliError> {
    let created: EphemeralCreated =
        connect(drive)?.call_as(EPHEMERAL_CREATE, json!({ "source": source }))?;
    output.emit(&created, || {
        let workspace = &created.workspace;
        println!(
            "Created ephemeral workspace {} ({}) with {} files ({} bytes)",
            workspace.name, workspace.id, created.files, created.bytes
        );
        println!("Path: {}", workspace.path);
        for skipped in &created.skipped {
            println!(
                "Skipped {} ({})",
                skipped.path,
                render::skip_reason(skipped.reason)
            );
        }
    })?;
    Ok(Exit::Success)
}

fn review(drive: Option<&Path>, id: &str, output: Output) -> Result<Exit, CliError> {
    let review: EphemeralReview = connect(drive)?.call_as(EPHEMERAL_REVIEW, json!({ "id": id }))?;
    output.emit(&review, || render::review(&review, true))?;
    Ok(Exit::Success)
}

fn parse_paths(paths: &[String]) -> Result<Vec<WorkspacePath>, CliError> {
    paths
        .iter()
        .map(|path| {
            let normalized = path.replace('\\', "/");
            WorkspacePath::new(normalized.trim_start_matches("./")).map_err(|error| {
                CliError::new(Exit::Usage, format!("invalid --path `{path}`: {error}"))
            })
        })
        .collect()
}

fn apply(
    drive: Option<&Path>,
    id: &str,
    paths: &[String],
    yes: bool,
    output: Output,
) -> Result<Exit, CliError> {
    let selected = parse_paths(paths)?;
    let mut client = connect(drive)?;
    let review: EphemeralReview = client.call_as(EPHEMERAL_REVIEW, json!({ "id": id }))?;
    let listed = |path: &WorkspacePath| review.changes.iter().any(|change| change.path == *path);
    if let Some(missing) = selected.iter().find(|path| !listed(path)) {
        return Err(CliError::new(
            Exit::Usage,
            if review.truncated {
                format!(
                    "`{}` is absent from the truncated review; refusing to apply an unreviewed path",
                    missing.as_str()
                )
            } else {
                format!(
                    "`{}` is not a change in this ephemeral workspace",
                    missing.as_str()
                )
            },
        ));
    }
    let source: Workspace = client.call_as(GET, json!({ "id": review.source_id.as_str() }))?;
    let to_apply: Vec<_> = review
        .changes
        .iter()
        .filter(|change| {
            change.conflict.is_none() && (selected.is_empty() || selected.contains(&change.path))
        })
        .collect();
    if !output.is_json() {
        render::review(&review, false);
    }
    if to_apply.is_empty() && !review.truncated {
        output.emit(&ApplyReport::default(), || println!("Nothing to apply."))?;
        return Ok(Exit::Success);
    }
    let count = if review.truncated && selected.is_empty() {
        format!(
            "all non-conflicting changes ({} total)",
            review.total_changes
        )
    } else {
        format!("{} change(s)", to_apply.len())
    };
    let question = format!("Apply {count} to {} at {}?", source.name, source.path);
    if !confirm(&question, yes, output)? {
        return Ok(cancelled(output));
    }
    let params = ApplyParams {
        id: id.trim().to_owned(),
        paths: (!selected.is_empty()).then_some(selected),
        review_digest: Some(review.digest.clone()),
    };
    let params = serde_json::to_value(&params)
        .map_err(|error| CliError::new(Exit::Failure, format!("cannot encode request: {error}")))?;
    let report: ApplyReport = client.call_as(EPHEMERAL_APPLY, params)?;
    output.emit(&report, || render::apply_report(&report))?;
    Ok(
        if report.skipped_conflicts.is_empty() && report.failed.is_empty() {
            Exit::Success
        } else {
            Exit::Failure
        },
    )
}

fn discard(drive: Option<&Path>, id: &str, yes: bool, output: Output) -> Result<Exit, CliError> {
    let mut client = connect(drive)?;
    let workspace: Workspace = client.call_as(GET, json!({ "id": id }))?;
    if workspace.id.as_str() != id.trim() {
        return Err(CliError::new(
            Exit::Usage,
            "discard requires the exact ephemeral workspace id",
        ));
    }
    let question = format!(
        "Discard ephemeral workspace {} ({}) and delete {}?",
        workspace.name, workspace.id, workspace.path
    );
    if !confirm(&question, yes, output)? {
        return Ok(cancelled(output));
    }
    let discarded: Discarded = client.call_as(EPHEMERAL_DISCARD, json!({ "id": id }))?;
    output.emit(&discarded, || {
        println!(
            "Discarded ephemeral workspace {} ({}); the source was not changed",
            discarded.workspace.name, discarded.workspace.id
        );
    })?;
    Ok(Exit::Success)
}
