//! `aidex stop --emergency` and `aidex resume` (spec §48).

use crate::{
    commands::daemon::connect,
    exit::{CliError, Exit},
    output::Output,
};
use aidexd::methods::emergency::{EmergencyReport, RESUME, ResumeResult, STOP};
use clap::Args;
use serde_json::json;
use std::path::PathBuf;

#[derive(Debug, Args)]
pub struct StopArgs {
    /// Required: freeze tool execution, cancel approvals, kill managed processes,
    /// and revoke secret handles.
    #[arg(long, required = true)]
    pub emergency: bool,
    /// Also lock the vault.
    #[arg(long)]
    pub lock_vault: bool,
    /// Reason recorded in the audit log.
    #[arg(long)]
    pub reason: Option<String>,
    /// Drive root; defaults to AIDEX_DRIVE_ROOT from the portable launcher.
    #[arg(long, value_name = "DRIVE_PATH")]
    pub drive: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct ResumeArgs {
    /// Drive root; defaults to AIDEX_DRIVE_ROOT from the portable launcher.
    #[arg(long, value_name = "DRIVE_PATH")]
    pub drive: Option<PathBuf>,
}

pub fn stop(args: StopArgs, output: Output) -> Result<Exit, CliError> {
    let mut params = json!({"lock_vault": args.lock_vault});
    if let Some(reason) = &args.reason {
        params["reason"] = json!(reason);
    }
    let report: EmergencyReport = connect(args.drive.as_deref())?.call_as(STOP, params)?;
    output.emit(&report, || {
        println!("EMERGENCY STOP in effect: {}", report.reason);
        println!(
            "  cancelled {} pending approval(s), revoked {} approval(s)",
            report.cancelled_approvals.len(),
            report.revoked_approvals.len()
        );
        println!(
            "  terminated {} process tree(s){}, cancelled {} running call(s)",
            report.terminated_processes,
            if report.failed_terminations > 0 {
                format!(" ({} NOT confirmed)", report.failed_terminations)
            } else {
                String::new()
            },
            report.cancelled_calls
        );
        println!("  revoked {} secret handle(s)", report.revoked_handles);
        if report.vault_locked {
            println!("  the vault is locked");
        }
        println!("Resume tool execution with `aidex resume`.");
    })?;
    Ok(if report.failed_terminations > 0 {
        Exit::Failure
    } else {
        Exit::Success
    })
}

pub fn resume(args: ResumeArgs, output: Output) -> Result<Exit, CliError> {
    let result: ResumeResult = connect(args.drive.as_deref())?.call_as(RESUME, json!({}))?;
    output.emit(&result, || {
        if result.resumed {
            println!("Tool execution resumed. Cancelled approvals must be requested again.");
        } else {
            println!("No emergency stop was in effect.");
        }
    })?;
    Ok(Exit::Success)
}
