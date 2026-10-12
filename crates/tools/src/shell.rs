//! `shell.exec`: run a program (argument vector) or, only with an explicit
//! `shell: true`, a shell command line, through the process launcher.

use std::time::Duration;

use aidex_events::RiskLevel;
use aidex_policy::{Action, ShellCommand, ShellPlatform, ShellRisk};
use aidex_sandbox::{KillReason, MountAccess};
use aidex_workspace::Access;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    EntryKind, ExecutionPermit, InputError, LaunchRequest, NetworkFacts, PathAccess, PlanFacts,
    PlanParts, RiskProfile, Tool, ToolContext, ToolDescriptor, ToolError, ToolOutput, ToolPlan,
    builtin::descriptor,
    fs::{parse_dir, plan_path, resolve, stat},
    launcher::{self, command_line},
    schema::{normalized, parse_input},
    text,
};

/// Longest command line or argument list, in bytes.
const MAX_COMMAND_BYTES: usize = 32 * 1024;
/// Most arguments.
const MAX_ARGS: usize = 1_000;
/// Longest stdin, in bytes.
const MAX_STDIN_BYTES: usize = 1024 * 1024;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ShellInput {
    /// Program to run (bare name on PATH or path). Required unless `shell` is true.
    #[serde(default)]
    program: Option<String>,
    /// Arguments passed verbatim (never interpreted by a shell).
    #[serde(default)]
    args: Vec<String>,
    /// A shell command line; only with `shell: true` (higher risk).
    #[serde(default)]
    command: Option<String>,
    /// Run `command` through the platform shell (`sh -c`, or PowerShell on Windows).
    #[serde(default)]
    shell: bool,
    /// Working directory, workspace-relative (default the workspace root).
    #[serde(default)]
    cwd: Option<String>,
    /// Wall-clock limit in milliseconds (capped by the configured process timeout).
    #[serde(default)]
    timeout_ms: Option<u64>,
    /// Text delivered on stdin.
    #[serde(default)]
    stdin: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ShellOutput {
    exit_code: Option<i32>,
    signal: Option<i32>,
    /// The process was terminated after `timeout_ms`.
    timed_out: bool,
    /// Lossy UTF-8 with likely credentials redacted (bounded).
    stdout: String,
    stderr: String,
    stdout_truncated: bool,
    stderr_truncated: bool,
    duration_ms: u64,
    /// Isolation level the launcher enforced (`direct`, `brokered`, `restricted`).
    sandbox_level: String,
}

/// The program and arguments to launch, and the line policy classifies.
struct Launch {
    program: String,
    args: Vec<String>,
    line: String,
    platform: ShellPlatform,
}

fn launch_spec(input: &ShellInput) -> Result<Launch, ToolError> {
    let platform = ShellPlatform::current();
    if input.shell {
        if input.program.is_some() || !input.args.is_empty() {
            return Err(ToolError::invalid(
                "program",
                InputError::ConflictingFields,
                "with `shell: true` give only `command`",
            ));
        }
        let Some(command) = input.command.clone() else {
            return Err(ToolError::invalid(
                "command",
                InputError::MissingField,
                "`shell: true` requires `command`",
            ));
        };
        if command.trim().is_empty() || command.len() > MAX_COMMAND_BYTES {
            return Err(ToolError::invalid(
                "command",
                InputError::InvalidValue,
                "the command must be non-blank and at most 32 KiB",
            ));
        }
        let (program, args) = if cfg!(windows) {
            (
                "powershell".to_owned(),
                vec![
                    "-NoLogo".to_owned(),
                    "-NoProfile".to_owned(),
                    "-NonInteractive".to_owned(),
                    "-Command".to_owned(),
                    command.clone(),
                ],
            )
        } else {
            ("sh".to_owned(), vec!["-c".to_owned(), command.clone()])
        };
        return Ok(Launch {
            program,
            args,
            line: command,
            platform,
        });
    }
    if input.command.is_some() {
        return Err(ToolError::invalid(
            "command",
            InputError::ConflictingFields,
            "`command` requires `shell: true`; use `program` and `args` otherwise",
        ));
    }
    let Some(program) = input.program.clone() else {
        return Err(ToolError::invalid(
            "program",
            InputError::MissingField,
            "`program` is required unless `shell` is true",
        ));
    };
    let size: usize = program.len() + input.args.iter().map(String::len).sum::<usize>();
    if program.trim().is_empty()
        || program.contains('\0')
        || input.args.iter().any(|arg| arg.contains('\0'))
        || input.args.len() > MAX_ARGS
        || size > MAX_COMMAND_BYTES
    {
        return Err(ToolError::invalid(
            "program",
            InputError::InvalidValue,
            "a non-blank program, at most 1000 arguments, 32 KiB in total, no NUL bytes",
        ));
    }
    let line = command_line(&program, &input.args, platform);
    Ok(Launch {
        program,
        args: input.args.clone(),
        line,
        platform,
    })
}

fn risk_level(risk: ShellRisk) -> RiskLevel {
    match risk {
        ShellRisk::Low => RiskLevel::Low,
        ShellRisk::Medium => RiskLevel::Medium,
        ShellRisk::High => RiskLevel::High,
        ShellRisk::Destructive => RiskLevel::Critical,
    }
}

pub(crate) struct ShellExec {
    descriptor: ToolDescriptor,
}

impl ShellExec {
    pub(crate) fn new() -> Result<Self, ToolError> {
        Ok(Self {
            descriptor: descriptor::<ShellInput, ShellOutput>(
                "shell.exec",
                1,
                "Run a program with arguments in the workspace (or, with `shell: true`, a shell \
                 command line). The command is classified for policy; output is bounded and \
                 redacted. A non-zero exit is reported, not treated as a tool failure.",
                RiskProfile {
                    action: Action::ShellExec,
                    base_risk: RiskLevel::Medium,
                    reads_workspace: true,
                    writes_workspace: true,
                    spawns_process: true,
                    network: true,
                    destructive: true,
                    remote_write: true,
                },
                &["workspace:read", "workspace:write", "process:any"],
            )?,
        })
    }
}

impl Tool for ShellExec {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn plan(&self, ctx: &ToolContext, input: Value) -> Result<ToolPlan, ToolError> {
        let input: ShellInput = parse_input(input)?;
        let launch = launch_spec(&input)?;
        if input.timeout_ms == Some(0) {
            return Err(ToolError::invalid(
                "timeout_ms",
                InputError::OutOfRange,
                "must be at least 1",
            ));
        }
        if input
            .stdin
            .as_ref()
            .is_some_and(|stdin| stdin.len() > MAX_STDIN_BYTES)
        {
            return Err(ToolError::invalid(
                "stdin",
                InputError::OutOfRange,
                "at most 1 MiB of stdin",
            ));
        }
        let cwd = parse_dir("cwd", input.cwd.as_deref())?;
        let absolute = match &cwd {
            Some(path) => resolve(ctx, "cwd", path, Access::Read)?,
            None => ctx.root.root().to_path_buf(),
        };
        let target = stat(&absolute)?;
        if target.kind != Some(EntryKind::Directory) {
            return Err(ToolError::WrongKind {
                path: input.cwd.clone().unwrap_or_default(),
                expected: "directory",
                found: target.kind.map_or("missing entry", EntryKind::as_str),
            });
        }
        let command = ShellCommand::classify(launch.line, launch.platform);
        let classification = command.classification();
        let mut risk = risk_level(classification.risk);
        if input.shell {
            // Shell syntax is harder to classify than an argument vector.
            risk = risk.max(RiskLevel::High);
        }
        let network = classification.network.then(|| NetworkFacts {
            targets: classification.network_targets.clone(),
            unresolved: if classification.network_targets.is_empty() {
                vec!["destination named by the command".to_owned()]
            } else {
                Vec::new()
            },
        });
        let facts = PlanFacts {
            paths: vec![plan_path(
                cwd.as_ref(),
                &absolute,
                PathAccess::Cwd,
                &target,
                None,
            )],
            destructive: classification.risk == ShellRisk::Destructive,
            remote_write: classification.implied_actions.contains(&Action::GitPush),
            network,
            implied_actions: Vec::new(),
            command: Some(command.clone()),
        };
        let summary = format!("run {}", command.redacted_line());
        ToolPlan::new(
            ctx,
            &self.descriptor,
            PlanParts::new(normalized(&input)?, summary)
                .with_facts(facts)
                .with_risk(risk),
        )
    }

    fn execute(
        &self,
        ctx: &ToolContext,
        plan: &ToolPlan,
        permit: &ExecutionPermit,
    ) -> Result<ToolOutput, ToolError> {
        let input: ShellInput = parse_input(plan.input().clone())?;
        let launch = launch_spec(&input)?;
        let cwd = parse_dir("cwd", input.cwd.as_deref())?;
        let directory = match &cwd {
            Some(path) => aidex_workspace::display_path(&resolve(ctx, "cwd", path, Access::Read)?),
            None => ctx.root.display_root(),
        };
        let timeout = input
            .timeout_ms
            .map_or(ctx.limits.process_timeout, Duration::from_millis)
            .min(ctx.limits.process_timeout);
        let network = plan.facts().network.is_some();
        let mut request = LaunchRequest::new(ctx, plan, permit, launch.program, launch.args)?
            .with_cwd(directory)
            .with_workspace_access(MountAccess::ReadWrite)
            .with_network(network)
            .with_timeout(timeout);
        if let Some(stdin) = &input.stdin {
            request = request.with_stdin(stdin.clone().into_bytes());
        }
        let outcome = launcher::launch(ctx, &request)?;
        let status = &outcome.status;
        let output = ShellOutput {
            exit_code: status.code,
            signal: status.signal,
            timed_out: status.killed_by == Some(KillReason::WallTimeout),
            stdout: text::process_text(&outcome.output.stdout.bytes),
            stderr: text::process_text(&outcome.output.stderr.bytes),
            stdout_truncated: outcome.output.stdout.truncated,
            stderr_truncated: outcome.output.stderr.truncated,
            duration_ms: u64::try_from(status.elapsed.as_millis()).unwrap_or(u64::MAX),
            sandbox_level: outcome.sandbox_level.as_str().to_owned(),
        };
        let summary = match (output.timed_out, output.exit_code) {
            (true, _) => format!("{} timed out", request.display()),
            (false, Some(code)) => format!("{} exited with {code}", request.display()),
            (false, None) => format!("{} was terminated", request.display()),
        };
        ToolOutput::typed(summary, &output)
    }
}
