//! `aidex update status|check|apply|rollback|verify`.
//!
//! `status`, `check`, `apply`, and `rollback` go through the daemon's `update.*`
//! methods (the daemon is the drive's only updater and recovers interrupted updates
//! when it starts). `verify` checks the installed bundle locally, exactly as the
//! launcher does, plus every file and the SBOM. A switched version takes effect on
//! the next launch: running processes keep their binaries.

use crate::{
    commands::daemon::connect,
    exit::{CliError, Exit},
    output::{Output, display_path},
};
use aidex_bundle::{SignatureState, Target, TrustStore, VerifyScope};
use aidex_drive::{DrivePaths, resolve_root};
use aidex_updater::{IntegrityReport, integrity_report};
use aidexd::{
    ClientError,
    client::UPDATE_CALL_TIMEOUT,
    methods::update::{
        APPLY, ApplyReport, CHECK, CheckReport, ROLLBACK, RollbackReport, STATUS, StatusResult,
    },
};
use clap::{Args, Subcommand};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Host-side opt-in shared with the launcher: accept a bundle without `bundle.sig`.
pub const ALLOW_UNSIGNED_ENV: &str = "AIDEX_ALLOW_UNSIGNED";
const RESTART_HINT: &str = "restart AiDEX (close it, then start AiDEX.exe or ./aidex again; run `aidex daemon stop` first if the daemon is running) to use it";

#[derive(Debug, Args)]
pub struct UpdateArgs {
    /// Drive root; defaults to AIDEX_DRIVE_ROOT from the portable launcher.
    #[arg(long, value_name = "DRIVE_PATH", global = true)]
    pub drive: Option<PathBuf>,
    #[command(subcommand)]
    pub command: UpdateCommand,
}

#[derive(Debug, Subcommand)]
pub enum UpdateCommand {
    /// Show the active and previous versions, integrity, and the last operation.
    Status,
    /// Verify an update package (archive, bundle directory, or release folder)
    /// without applying it.
    Check(PackageArgs),
    /// Verify, self-test, and switch to a signed update package; it takes effect on
    /// the next launch.
    Apply(PackageArgs),
    /// Switch back to the previous version after re-verifying it.
    Rollback {
        /// Development only: accept a previous bundle without a signature.
        #[arg(long)]
        allow_unsigned: bool,
    },
    /// Verify the installed version: signature, trusted key, every file, and SBOM.
    Verify {
        /// Development only: accept an unsigned installation (also AIDEX_ALLOW_UNSIGNED=1).
        #[arg(long)]
        allow_unsigned: bool,
    },
}

#[derive(Debug, Args)]
pub struct PackageArgs {
    /// Update package: a .zip/.tar.gz archive, an unpacked bundle, or a folder of packages.
    #[arg(value_name = "SOURCE")]
    pub source: PathBuf,
    /// Permit returning to the recorded previous version with an older package.
    #[arg(long)]
    pub allow_downgrade: bool,
    /// Development only: accept a package without bundle.sig (a warning is recorded).
    #[arg(long)]
    pub allow_unsigned: bool,
}

impl UpdateCommand {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Status => "aidex update status",
            Self::Check(_) => "aidex update check",
            Self::Apply(_) => "aidex update apply",
            Self::Rollback { .. } => "aidex update rollback",
            Self::Verify { .. } => "aidex update verify",
        }
    }
}

pub fn run(args: UpdateArgs, output: Output) -> Result<Exit, CliError> {
    let drive = args.drive.as_deref();
    match args.command {
        UpdateCommand::Status => status(drive, output),
        UpdateCommand::Check(package) => check(drive, &package, output),
        UpdateCommand::Apply(package) => apply(drive, &package, output),
        UpdateCommand::Rollback { allow_unsigned } => rollback(drive, allow_unsigned, output),
        UpdateCommand::Verify { allow_unsigned } => verify(drive, allow_unsigned, output),
    }
}

/// Update refusals: integrity and authenticity failures exit 11 (as the launcher
/// does); version and platform refusals exit 10 without the IPC-version hint.
fn update_error(error: ClientError) -> CliError {
    match &error {
        ClientError::Rpc { data, .. }
            if data
                .as_ref()
                .and_then(|data| data["integrity"].as_bool())
                .unwrap_or(false) =>
        {
            CliError::new(Exit::Integrity, error.to_string()).with_hint(
                "nothing was changed; use a package signed by a key this AiDEX trusts, from a source you trust",
            )
        }
        ClientError::Incompatible { message, .. } => CliError::new(Exit::Incompatible, message.clone())
            .with_hint("nothing was changed; check `aidex update status` for the active and previous versions"),
        _ => CliError::from(error),
    }
}

fn absolute(source: &Path) -> Result<PathBuf, CliError> {
    std::path::absolute(source).map_err(|error| {
        CliError::new(
            Exit::Usage,
            format!("cannot resolve `{}`: {error}", source.display()),
        )
    })
}

fn package_params(package: &PackageArgs) -> Result<Value, CliError> {
    Ok(json!({
        "source": absolute(&package.source)?,
        "allow_downgrade": package.allow_downgrade,
        "allow_unsigned": package.allow_unsigned,
    }))
}

fn signer(signature: &SignatureState) -> String {
    match signature {
        SignatureState::Verified {
            key_id, key_name, ..
        } => format!("signed by {key_name} ({key_id})"),
        SignatureState::UnsignedAllowed => "UNSIGNED (development opt-in)".into(),
    }
}

fn status(drive: Option<&Path>, output: Output) -> Result<Exit, CliError> {
    let result: StatusResult = connect(drive)?
        .call_as(STATUS, Value::Null)
        .map_err(update_error)?;
    output.emit(&result, || {
        let status = &result.status;
        println!("Target: {}", status.target);
        match &status.current {
            Some(current) => println!("Active: {} ({})", current.version, current.dir),
            None => println!("Active: none installed"),
        }
        if let Some(previous) = &status.previous {
            println!("Previous: {} ({})", previous.version, previous.dir);
        }
        print_integrity(&status.integrity);
        if let Some(journal) = &status.journal {
            println!(
                "Last operation: {} {} at {}{}",
                journal.operation.as_str(),
                journal.state.as_str(),
                journal.updated_at.as_str(),
                journal
                    .outcome
                    .as_deref()
                    .map(|outcome| format!(" ({outcome})"))
                    .unwrap_or_default()
            );
        }
        if status.pending {
            println!("Pending: an interrupted operation is recovered when the daemon starts");
        }
        if let Some(recovery) = &result.recovery {
            println!("Startup recovery: {:?}", recovery.outcome);
        }
        if let Some(error) = &result.recovery_error {
            println!("Startup recovery failed: {error}");
        }
        if status.restart_required {
            println!("Restart required: {RESTART_HINT}");
        }
    })?;
    Ok(Exit::Success)
}

fn check(drive: Option<&Path>, package: &PackageArgs, output: Output) -> Result<Exit, CliError> {
    let mut client = connect(drive)?;
    let report: CheckReport = client
        .call_with_timeout(CHECK, package_params(package)?, UPDATE_CALL_TIMEOUT)
        .and_then(|value| {
            serde_json::from_value(value).map_err(|error| ClientError::Protocol(error.to_string()))
        })
        .map_err(update_error)?;
    output.emit(&report, || {
        println!(
            "Package: AiDEX {} for {} ({})",
            report.candidate.version,
            report.candidate.target,
            signer(&report.candidate.signature)
        );
        println!("Checksum: {:?}", report.checksum);
        match &report.current {
            Some(current) => println!("Active: {}", current.version),
            None => println!("Active: none installed"),
        }
        println!("Applying it would: {:?}", report.action);
    })?;
    Ok(Exit::Success)
}

fn apply(drive: Option<&Path>, package: &PackageArgs, output: Output) -> Result<Exit, CliError> {
    if package.allow_unsigned && !output.is_json() {
        eprintln!(
            "warning: --allow-unsigned accepts a package whose origin cannot be verified (development only)"
        );
    }
    let mut client = connect(drive)?;
    let report: ApplyReport = client
        .call_with_timeout(APPLY, package_params(package)?, UPDATE_CALL_TIMEOUT)
        .and_then(|value| {
            serde_json::from_value(value).map_err(|error| ClientError::Protocol(error.to_string()))
        })
        .map_err(update_error)?;
    output.emit(&report, || {
        let from = report
            .from
            .as_ref()
            .map(|from| format!(" (was {})", from.version))
            .unwrap_or_default();
        println!(
            "Update {:?}: AiDEX {} is active{from}, {}",
            report.action,
            report.to.version,
            signer(&report.candidate.signature)
        );
        if !report.pruned.is_empty() {
            println!("Removed old versions: {}", report.pruned.join(", "));
        }
        for warning in &report.warnings {
            eprintln!("warning: {warning}");
        }
        if report.restart_required {
            println!("Next: {RESTART_HINT}");
        }
    })?;
    Ok(Exit::Success)
}

fn rollback(drive: Option<&Path>, allow_unsigned: bool, output: Output) -> Result<Exit, CliError> {
    let mut client = connect(drive)?;
    let report: RollbackReport = client
        .call_with_timeout(
            ROLLBACK,
            json!({ "allow_unsigned": allow_unsigned }),
            UPDATE_CALL_TIMEOUT,
        )
        .and_then(|value| {
            serde_json::from_value(value).map_err(|error| ClientError::Protocol(error.to_string()))
        })
        .map_err(update_error)?;
    output.emit(&report, || {
        println!(
            "Rolled back: AiDEX {} is active (was {}), {}",
            report.to.version,
            report.from.version,
            signer(&report.signature)
        );
        for warning in &report.warnings {
            eprintln!("warning: {warning}");
        }
        println!("Next: {RESTART_HINT}");
    })?;
    Ok(Exit::Success)
}

/// The integrity of the installed bundle for this host (doctor and `update verify`).
pub fn local_integrity(root: &Path, scope: VerifyScope, allow_unsigned: bool) -> IntegrityReport {
    let unavailable = |reason: &str, error: String| IntegrityReport {
        target: Target::host(),
        installed: false,
        verified: false,
        scope,
        version: None,
        dir: None,
        manifest_sha256: None,
        verified_files: None,
        signature: None,
        signer_key_id: None,
        sbom: Default::default(),
        previous: None,
        pending_update: None,
        reason: Some(reason.to_owned()),
        error: Some(error),
    };
    let Some(target) = Target::host() else {
        return unavailable("unsupported_host", "this host has no bundle target".into());
    };
    let paths = match DrivePaths::open(root) {
        Ok(paths) => paths,
        Err(error) => return unavailable("drive", error.to_string()),
    };
    let trust = match TrustStore::builtin() {
        Ok(trust) => trust,
        Err(error) => return unavailable("trust", error.to_string()),
    };
    let allow = allow_unsigned || std::env::var(ALLOW_UNSIGNED_ENV).is_ok_and(|value| value == "1");
    integrity_report(&paths, target, &trust, scope, allow)
}

pub fn print_integrity(report: &IntegrityReport) {
    if !report.installed {
        println!("Integrity: no AiDEX bundle installed for this host");
        return;
    }
    if report.verified {
        let signature = report
            .signature
            .as_ref()
            .map(signer)
            .unwrap_or_else(|| "signature not checked".into());
        println!(
            "Integrity: verified {} files ({:?}), {signature}",
            report.verified_files.unwrap_or_default(),
            report.scope
        );
    } else {
        println!(
            "Integrity: FAILED ({}): {}",
            report.reason.as_deref().unwrap_or("error"),
            report.error.as_deref().unwrap_or("unknown")
        );
    }
    match (&report.sbom.summary, &report.sbom.error) {
        (Some(summary), _) => println!(
            "SBOM: CycloneDX {} with {} components",
            summary.spec_version, summary.components
        ),
        (None, Some(error)) => println!("SBOM: invalid ({error})"),
        (None, None) => println!("SBOM: none"),
    }
}

fn verify(drive: Option<&Path>, allow_unsigned: bool, output: Output) -> Result<Exit, CliError> {
    let root = resolve_root(drive)?;
    let report = local_integrity(&root, VerifyScope::All, allow_unsigned);
    if !report.installed {
        return Err(CliError::new(
            Exit::NotFound,
            format!(
                "no AiDEX bundle for this host is installed on {}",
                display_path(&root)
            ),
        ));
    }
    if !report.verified {
        return Err(CliError::new(
            Exit::Integrity,
            format!(
                "the installed bundle failed verification ({}): {}",
                report.reason.as_deref().unwrap_or("error"),
                report.error.as_deref().unwrap_or("unknown")
            ),
        )
        .with_hint("roll back with `aidex update rollback`, or reinstall a signed package with `aidex update apply <archive>`"));
    }
    output.emit(&report, || {
        println!(
            "AiDEX {} ({})",
            report.version.as_deref().unwrap_or("?"),
            report.dir.as_deref().unwrap_or("?")
        );
        print_integrity(&report);
    })?;
    Ok(Exit::Success)
}
