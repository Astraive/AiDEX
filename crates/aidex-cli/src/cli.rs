//! Command-line definitions. Subcommand argument types live next to their
//! handlers in `commands/<domain>.rs`; add one variant here per domain.

use crate::commands::{
    agent::AgentCommand, daemon::DaemonCommand, events::EventsCommand, omp::SessionOptions,
    session::SessionArgs, update::UpdateArgs, vault::VaultCommand, workspace::WorkspaceArgs,
};
use crate::commands::{
    approvals::{ApprovalsArgs, ApprovalsCommand, ApproveArgs, DenyArgs},
    context::{
        index::IndexArgs, memory::MemoryArgs, pack::ContextArgs, rules::RulesArgs,
        search::SearchArgs,
    },
    emergency::{ResumeArgs, StopArgs},
    inference::InferenceArgs,
    mcp::McpArgs,
    model::ModelArgs,
    offline::OfflineArgs,
    plugins::PluginsArgs,
    policy::PolicyArgs,
    provider::ProviderArgs,
    skills::SkillsArgs,
    tools::ToolsArgs,
};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "aidex", version, about = "Portable AI development environment")]
pub struct Cli {
    /// Print machine-readable JSON on stdout; errors become JSON on stderr.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Open an interactive agent session with approval prompts.
    Chat {
        #[command(flatten)]
        options: SessionOptions,
        /// Optional first message.
        #[arg(value_name = "PROMPT", num_args = 1.., trailing_var_arg = true)]
        prompt: Vec<String>,
    },
    /// Run a read-only, non-interactive agent prompt.
    Run {
        #[command(flatten)]
        options: SessionOptions,
        /// Prompt to send to the agent.
        #[arg(value_name = "PROMPT", required = true, num_args = 1.., trailing_var_arg = true)]
        prompt: Vec<String>,
    },
    /// Inspect configured agent runtimes.
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },
    /// Initialize AiDEX-owned directories on a removable drive.
    Init {
        /// Removable drive root (must already exist).
        #[arg(long, value_name = "DRIVE_PATH")]
        drive: PathBuf,
    },
    /// Report active drive health without changing it.
    Status {
        /// Drive root; defaults to AIDEX_DRIVE_ROOT from the portable launcher.
        #[arg(long, value_name = "DRIVE_PATH")]
        drive: Option<PathBuf>,
    },
    /// Control or inspect the local daemon.
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    /// Print or follow the drive's event log, resuming from the last printed cursor.
    Events {
        #[command(subcommand)]
        command: EventsCommand,
    },
    /// Check host information, drive health, and Oh My Pi availability.
    Doctor,
    /// Manage the drive's encrypted vault and its secrets.
    Vault {
        #[command(subcommand)]
        command: VaultCommand,
    },
    /// Manage host, drive, and ephemeral workspaces.
    #[command(alias = "project")]
    Workspace(WorkspaceArgs),
    /// Inspect models, choose the default, and explain routing.
    Model(ModelArgs),
    /// Import GGUF models and run them on the drive's verified llama.cpp runtime.
    Inference(InferenceArgs),
    /// Configure model providers (credentials come from the vault).
    Provider(ProviderArgs),
    /// Refuse or allow remote provider traffic.
    Offline(OfflineArgs),
    /// Inspect durable sessions, checkpoints, and resume plans.
    #[command(alias = "sessions")]
    Session(SessionArgs),
    /// List, inspect, enable, and disable skills (drive, workspace, and plugin).
    #[command(alias = "skill")]
    Skills(SkillsArgs),
    /// List and inspect the broker's approval requests (default: list).
    #[command(alias = "approval")]
    Approvals(ApprovalsArgs),
    /// Approve a pending request after reviewing its card (asks unless --yes).
    Approve(ApproveArgs),
    /// Deny a pending request.
    Deny(DenyArgs),
    /// Show or change the active policy, or explain a tool call without running it.
    Policy(PolicyArgs),
    /// List the tools the broker mediates.
    #[command(alias = "tool")]
    Tools(ToolsArgs),
    /// Install, review, trust, and enable plugin bundles of skills and MCP servers.
    #[command(alias = "plugin")]
    Plugins(PluginsArgs),
    /// List, start, and stop MCP servers and inspect their tools and logs.
    Mcp(McpArgs),
    /// Emergency stop: freeze tools, cancel approvals, kill managed processes.
    Stop(StopArgs),
    /// Resume tool execution after an emergency stop.
    Resume(ResumeArgs),
    /// Build, inspect, or cancel the workspace's local search index.
    Index(IndexArgs),
    /// Search the workspace index offline (full text, file names, or symbols).
    Search(SearchArgs),
    /// Inspect the context packs the native agent receives.
    Context(ContextArgs),
    /// Inspect and edit session, workspace, and user memory.
    Memory(MemoryArgs),
    /// Show the workspace's project rules and instruction files.
    Rules(RulesArgs),
    /// Check, apply, verify, and roll back signed AiDEX updates on this drive.
    Update(UpdateArgs),
}

impl Commands {
    /// Prefix for human error messages, e.g. `aidex daemon status`.
    pub fn label(command: Option<&Self>) -> &'static str {
        match command {
            None | Some(Self::Chat { .. }) => "aidex",
            Some(Self::Run { .. }) => "aidex run",
            Some(Self::Agent { .. }) => "aidex agent",
            Some(Self::Init { .. }) => "aidex init",
            Some(Self::Status { .. }) => "aidex status",
            Some(Self::Daemon { command }) => match command {
                DaemonCommand::Start { .. } => "aidex daemon start",
                DaemonCommand::Status { .. } => "aidex daemon status",
                DaemonCommand::Stop { .. } => "aidex daemon stop",
            },
            Some(Self::Events { command }) => command.label(),
            Some(Self::Doctor) => "aidex doctor",
            Some(Self::Vault { command }) => command.label(),
            Some(Self::Workspace(args)) => args.command.label(),
            Some(Self::Model(args)) => args.command.label(),
            Some(Self::Inference(args)) => args.command.label(),
            Some(Self::Provider(args)) => args.command.label(),
            Some(Self::Offline(args)) => args.command.label(),
            Some(Self::Session(args)) => args.command.label(),
            Some(Self::Skills(args)) => args.command.label(),
            Some(Self::Approvals(args)) => ApprovalsCommand::label(args.command.as_ref()),
            Some(Self::Approve(_)) => "aidex approve",
            Some(Self::Deny(_)) => "aidex deny",
            Some(Self::Policy(args)) => args.command.label(),
            Some(Self::Tools(args)) => args.command.label(),
            Some(Self::Plugins(args)) => args.command.label(),
            Some(Self::Mcp(args)) => args.command.label(),
            Some(Self::Stop(_)) => "aidex stop",
            Some(Self::Resume(_)) => "aidex resume",
            Some(Self::Index(args)) => args.command.label(),
            Some(Self::Search(args)) => args.label(),
            Some(Self::Context(args)) => args.command.label(),
            Some(Self::Memory(args)) => args.command.label(),
            Some(Self::Rules(args)) => args.command.label(),
            Some(Self::Update(args)) => args.command.label(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, error::ErrorKind};

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("aidex").chain(args.iter().copied()))
    }

    #[test]
    fn command_definitions_are_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn json_is_a_global_flag() {
        for args in [
            &["--json", "daemon", "status"][..],
            &["daemon", "status", "--json"],
            &["daemon", "--json", "status", "--drive", "x"],
        ] {
            let cli = parse(args).unwrap();
            assert!(cli.json, "{args:?}");
            assert!(matches!(
                cli.command,
                Some(Commands::Daemon {
                    command: DaemonCommand::Status { .. }
                })
            ));
        }
        assert!(!parse(&["doctor"]).unwrap().json);
    }

    #[test]
    fn subcommands_parse_their_arguments() {
        let cli = parse(&["daemon", "start", "--drive", "/media/aidex"]).unwrap();
        let Some(Commands::Daemon {
            command: DaemonCommand::Start { drive },
        }) = cli.command
        else {
            panic!("expected daemon start");
        };
        assert_eq!(drive, Some(PathBuf::from("/media/aidex")));

        let cli = parse(&[
            "events",
            "tail",
            "--workspace",
            "ws",
            "--after",
            "7",
            "--kind",
            "tool",
            "--kind",
            "approval",
            "--follow",
        ])
        .unwrap();
        let Some(Commands::Events {
            command: EventsCommand::Tail(tail),
        }) = cli.command
        else {
            panic!("expected events tail");
        };
        assert_eq!(
            (tail.filter.workspace.as_deref(), tail.after, tail.follow),
            (Some("ws"), 7, true)
        );
        assert_eq!(tail.filter.kinds, ["tool", "approval"]);
        let cli = parse(&["events", "show", "--after", "3", "--limit", "9"]).unwrap();
        let Some(Commands::Events {
            command: EventsCommand::Show(show),
        }) = cli.command
        else {
            panic!("expected events show");
        };
        assert_eq!((show.after, show.limit), (3, 9));

        let cli = parse(&["run", "--cwd", ".", "explain", "this"]).unwrap();
        let Some(Commands::Run { prompt, options }) = cli.command else {
            panic!("expected run");
        };
        assert_eq!(prompt, ["explain", "this"]);
        assert_eq!(options.cwd, Some(PathBuf::from(".")));
        assert!(parse(&[]).unwrap().command.is_none());
        assert!(matches!(
            parse(&["agent", "list"]).unwrap().command,
            Some(Commands::Agent {
                command: AgentCommand::List
            })
        ));
    }

    #[test]
    fn usage_errors_are_reported_by_clap() {
        for args in [
            &["init"][..],
            &["run"],
            &["events"],
            &["events", "tail", "--after", "-1"],
            &["daemon", "restart"],
            &["--bogus"],
        ] {
            let error = parse(args).unwrap_err();
            assert!(error.use_stderr(), "{args:?}");
            assert_ne!(error.kind(), ErrorKind::DisplayHelp);
        }
        assert_eq!(
            parse(&["--help"]).unwrap_err().kind(),
            ErrorKind::DisplayHelp
        );
    }

    #[test]
    fn labels_name_the_failing_command() {
        let cli = parse(&["daemon", "stop"]).unwrap();
        assert_eq!(Commands::label(cli.command.as_ref()), "aidex daemon stop");
        assert_eq!(Commands::label(None), "aidex");
    }
}
