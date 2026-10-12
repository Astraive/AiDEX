//! `aidex approvals`, `aidex approve`, `aidex deny`: review and decide the
//! execution broker's approval requests through the daemon's `approval.*` methods.
//!
//! Approving always shows the card first and asks for confirmation unless `--yes`
//! is given; the default answer denies the request. Without a terminal (or with
//! `--json`) approval refuses unless `--yes` is given, and changes nothing.

mod render;

use crate::{
    commands::daemon::connect,
    exit::{CliError, Exit},
    output::Output,
};
use aidexd::methods::approvals::{ApprovalList, ApprovalRecord, LIST, RESOLVE, REVOKE, SHOW};
use clap::{Args, Subcommand};
use serde_json::{Value, json};
use std::{
    io::{self, BufRead, IsTerminal, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Args)]
pub struct ApprovalsArgs {
    /// Drive root; defaults to AIDEX_DRIVE_ROOT from the portable launcher.
    #[arg(long, value_name = "DRIVE_PATH", global = true)]
    pub drive: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Option<ApprovalsCommand>,
    #[command(flatten)]
    pub list: ListArgs,
}

#[derive(Debug, Default, Args)]
pub struct ListArgs {
    /// Only requests waiting for a decision.
    #[arg(long, conflicts_with = "state")]
    pub pending: bool,
    /// Only approvals in this state (pending, approved, denied, expired, revoked,
    /// cancelled).
    #[arg(long)]
    pub state: Option<String>,
    /// Only approvals of this session id.
    #[arg(long, value_name = "ID")]
    pub session: Option<String>,
    /// 1-500 (default 500).
    #[arg(long)]
    pub limit: Option<usize>,
}

#[derive(Debug, Subcommand)]
pub enum ApprovalsCommand {
    /// List approvals, newest first.
    List(ListArgs),
    /// Show an approval card: effect, workspace, scope options, expiry, and diff.
    Show { id: String },
    /// Revoke a pending request, an approved call that has not run, or a session
    /// grant.
    Revoke {
        id: String,
        #[arg(long)]
        reason: Option<String>,
    },
}

impl ApprovalsCommand {
    pub fn label(command: Option<&Self>) -> &'static str {
        match command {
            None | Some(Self::List(_)) => "aidex approvals list",
            Some(Self::Show { .. }) => "aidex approvals show",
            Some(Self::Revoke { .. }) => "aidex approvals revoke",
        }
    }
}

#[derive(Debug, Args)]
pub struct ApproveArgs {
    /// Approval id (see `aidex approvals --pending`).
    pub id: String,
    /// Also allow identical calls for the rest of the session (when offered).
    #[arg(long)]
    pub session: bool,
    /// Approve without the confirmation prompt (after reviewing the card).
    #[arg(long, short = 'y')]
    pub yes: bool,
    /// Drive root; defaults to AIDEX_DRIVE_ROOT from the portable launcher.
    #[arg(long, value_name = "DRIVE_PATH")]
    pub drive: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct DenyArgs {
    /// Approval id.
    pub id: String,
    /// Drive root; defaults to AIDEX_DRIVE_ROOT from the portable launcher.
    #[arg(long, value_name = "DRIVE_PATH")]
    pub drive: Option<PathBuf>,
}

fn call<T: serde::de::DeserializeOwned>(
    drive: Option<&Path>,
    method: &str,
    params: Value,
) -> Result<T, CliError> {
    Ok(connect(drive)?.call_as(method, params)?)
}

pub fn run(args: ApprovalsArgs, output: Output) -> Result<Exit, CliError> {
    let drive = args.drive.as_deref();
    match args.command {
        None => list(drive, &args.list, output),
        Some(ApprovalsCommand::List(list_args)) => list(drive, &list_args, output),
        Some(ApprovalsCommand::Show { id }) => {
            let record: ApprovalRecord = call(drive, SHOW, json!({"id": id}))?;
            output.emit(&record, || render::card(&record))?;
            Ok(Exit::Success)
        }
        Some(ApprovalsCommand::Revoke { id, reason }) => {
            let record: ApprovalRecord = call(drive, REVOKE, json!({"id": id, "reason": reason}))?;
            output.emit(&record, || {
                println!("Revoked {} ({})", record.approval_id, record.card.summary);
            })?;
            Ok(Exit::Success)
        }
    }
}

fn list(drive: Option<&Path>, args: &ListArgs, output: Output) -> Result<Exit, CliError> {
    let mut params = json!({});
    if args.pending {
        params["state"] = json!("pending");
    }
    if let Some(state) = &args.state {
        params["state"] = json!(state);
    }
    if let Some(session) = &args.session {
        params["session"] = json!(session);
    }
    if let Some(limit) = args.limit {
        params["limit"] = json!(limit);
    }
    let listed: ApprovalList = call(drive, LIST, params)?;
    output.emit(&listed, || render::list(&listed.approvals))?;
    Ok(Exit::Success)
}

/// Show the card and ask; anything but `y`/`yes` denies. `--yes` skips the prompt.
fn confirm_approval(record: &ApprovalRecord, session: bool) -> Result<bool, CliError> {
    let stdin = io::stdin();
    if !stdin.is_terminal() || !io::stderr().is_terminal() {
        return Err(CliError::new(
            Exit::Usage,
            "refusing to approve without confirmation; nothing was changed",
        )
        .with_hint("review it with `aidex approvals show <ID>`, then re-run with `--yes`"));
    }
    render::card_to_stderr(record);
    let scope = if session { "for this session" } else { "once" };
    eprint!("Approve {scope}? [y/N] (N denies the request) ");
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

pub fn approve(args: ApproveArgs, output: Output) -> Result<Exit, CliError> {
    let drive = args.drive.as_deref();
    let mut client = connect(drive)?;
    let record: ApprovalRecord = client.call_as(SHOW, json!({"id": args.id}))?;
    if !args.yes && output.is_json() {
        return Err(CliError::new(
            Exit::Usage,
            "refusing to approve without confirmation in --json mode; nothing was changed",
        )
        .with_hint("re-run with `--yes` after reviewing the card"));
    }
    let approved = args.yes || confirm_approval(&record, args.session)?;
    let decision = match (approved, args.session) {
        (false, _) => "deny",
        (true, false) => "approve_once",
        (true, true) => "approve_session",
    };
    let record: ApprovalRecord =
        client.call_as(RESOLVE, json!({"id": args.id, "decision": decision}))?;
    output.emit(&record, || {
        if approved {
            println!(
                "Approved {} {} ({}); the requester can now continue it",
                record.approval_id,
                if args.session {
                    "for this session"
                } else {
                    "once"
                },
                record.card.summary
            );
        } else {
            println!("Denied {} ({})", record.approval_id, record.card.summary);
        }
    })?;
    Ok(Exit::Success)
}

pub fn deny(args: DenyArgs, output: Output) -> Result<Exit, CliError> {
    let record: ApprovalRecord = call(
        args.drive.as_deref(),
        RESOLVE,
        json!({"id": args.id, "decision": "deny"}),
    )?;
    output.emit(&record, || {
        println!("Denied {} ({})", record.approval_id, record.card.summary);
    })?;
    Ok(Exit::Success)
}

#[cfg(test)]
mod tests {
    use crate::cli::{Cli, Commands};
    use clap::Parser;

    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("aidex").chain(args.iter().copied()))
    }

    #[test]
    fn approval_commands_parse() {
        let Some(Commands::Approvals(args)) = parse(&["approvals", "--pending"]).unwrap().command
        else {
            panic!("expected approvals");
        };
        assert!(args.command.is_none() && args.list.pending);
        let Some(Commands::Approvals(args)) = parse(&["approvals", "list", "--state", "denied"])
            .unwrap()
            .command
        else {
            panic!("expected approvals list");
        };
        assert!(
            matches!(args.command, Some(ApprovalsCommand::List(ListArgs { state: Some(ref state), .. })) if state == "denied")
        );
        let Some(Commands::Approve(args)) = parse(&["approve", "apr_1", "--session", "--yes"])
            .unwrap()
            .command
        else {
            panic!("expected approve");
        };
        assert!(args.session && args.yes && args.id == "apr_1");
        assert!(matches!(
            parse(&["deny", "apr_1"]).unwrap().command,
            Some(Commands::Deny(_))
        ));
        assert!(parse(&["approvals", "--pending", "--state", "pending"]).is_err());
        assert!(parse(&["stop"]).is_err());
        let Some(Commands::Stop(stop)) = parse(&["stop", "--emergency", "--lock-vault"])
            .unwrap()
            .command
        else {
            panic!("expected stop");
        };
        assert!(stop.emergency && stop.lock_vault);
        assert!(parse(&["policy", "explain", "--tool", "fs.read"]).is_ok());
        assert!(parse(&["tools", "list"]).is_ok());
    }
}
