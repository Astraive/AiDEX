//! `aidex workspace` (alias `aidex project`): register host directories, create
//! drive workspaces, select the active workspace, discover repositories, and
//! review/apply/discard ephemeral copies. Every command goes through the daemon's
//! `workspace.*` methods.

pub(crate) mod ephemeral;
mod render;

use crate::{
    commands::daemon::connect,
    exit::{CliError, Exit},
    output::Output,
};
use aidex_workspace::{Discovery, Workspace};
use aidexd::methods::workspace::{
    ADD, CREATE_DRIVE, DISCOVER, GET, LIST, REMOVE, Removed, SELECT, SELECTED, SelectedWorkspace,
    WorkspaceList,
};
use clap::{Args, Subcommand};
use serde_json::json;
use std::path::{Path, PathBuf};

pub use ephemeral::EphemeralCommand;

#[derive(Debug, Args)]
pub struct WorkspaceArgs {
    /// Drive root; defaults to AIDEX_DRIVE_ROOT from the portable launcher.
    #[arg(long, value_name = "DRIVE_PATH", global = true)]
    pub drive: Option<PathBuf>,
    #[command(subcommand)]
    pub command: WorkspaceCommand,
}

#[derive(Debug, Subcommand)]
pub enum WorkspaceCommand {
    /// Register an existing host directory exactly as selected.
    Add {
        /// Directory to register (relative paths resolve against the current directory).
        path: PathBuf,
        /// Workspace name (default: the directory name).
        #[arg(long)]
        name: Option<String>,
    },
    /// Create an empty workspace stored on the drive.
    Create {
        /// Unique workspace name.
        name: String,
    },
    /// List registered workspaces and whether their roots are available here.
    List,
    /// Show one workspace (default: the selected workspace).
    Show {
        /// Workspace id or name.
        id: Option<String>,
    },
    /// Select the active workspace; the selection is stored on the drive.
    Select {
        /// Workspace id or name.
        id_or_name: String,
    },
    /// Unregister a workspace. Host files are never deleted.
    Remove {
        /// Exact workspace id.
        id: String,
        /// Also delete the directory of an AiDEX-created (drive or ephemeral) workspace.
        #[arg(long)]
        delete_data: bool,
        /// Confirm `--delete-data` without prompting.
        #[arg(long)]
        yes: bool,
    },
    /// Report the enclosing and nested repositories of a directory.
    Discover {
        /// Directory to inspect.
        path: PathBuf,
    },
    /// Reviewable ephemeral copies of a workspace.
    Ephemeral {
        #[command(subcommand)]
        command: EphemeralCommand,
    },
}

impl WorkspaceCommand {
    /// Prefix for human error messages.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Add { .. } => "aidex workspace add",
            Self::Create { .. } => "aidex workspace create",
            Self::List => "aidex workspace list",
            Self::Show { .. } => "aidex workspace show",
            Self::Select { .. } => "aidex workspace select",
            Self::Remove { .. } => "aidex workspace remove",
            Self::Discover { .. } => "aidex workspace discover",
            Self::Ephemeral { command } => command.label(),
        }
    }
}

pub fn run(args: WorkspaceArgs, output: Output) -> Result<Exit, CliError> {
    let drive = args.drive.as_deref();
    match args.command {
        WorkspaceCommand::Add { path, name } => add(drive, &path, name, output),
        WorkspaceCommand::Create { name } => create(drive, &name, output),
        WorkspaceCommand::List => list(drive, output),
        WorkspaceCommand::Show { id } => show(drive, id.as_deref(), output),
        WorkspaceCommand::Select { id_or_name } => select(drive, &id_or_name, output),
        WorkspaceCommand::Remove {
            id,
            delete_data,
            yes,
        } => remove(drive, &id, delete_data, yes, output),
        WorkspaceCommand::Discover { path } => discover(drive, &path, output),
        WorkspaceCommand::Ephemeral { command } => ephemeral::run(drive, command, output),
    }
}

/// An absolute, UTF-8 spelling of a user path; the daemon's working directory is
/// unrelated to the caller's, so relative paths are resolved here.
fn absolute(path: &Path) -> Result<String, CliError> {
    let absolute = std::path::absolute(path).map_err(|error| {
        CliError::new(
            Exit::Usage,
            format!("cannot resolve `{}`: {error}", path.display()),
        )
    })?;
    absolute.to_str().map(str::to_owned).ok_or_else(|| {
        CliError::new(
            Exit::Usage,
            format!("`{}` is not valid Unicode", absolute.display()),
        )
    })
}

fn emit_workspace(output: Output, workspace: &Workspace, verb: &str) -> Result<Exit, CliError> {
    output.emit(workspace, || {
        println!("{verb} workspace {} ({})", workspace.name, workspace.id);
        render::workspace_details(workspace, false);
    })?;
    Ok(Exit::Success)
}

fn add(
    drive: Option<&Path>,
    path: &Path,
    name: Option<String>,
    output: Output,
) -> Result<Exit, CliError> {
    let path = absolute(path)?;
    let mut params = json!({ "path": path });
    if let Some(name) = name {
        params["name"] = json!(name);
    }
    let workspace: Workspace = connect(drive)?.call_as(ADD, params)?;
    emit_workspace(output, &workspace, "Added")
}

fn create(drive: Option<&Path>, name: &str, output: Output) -> Result<Exit, CliError> {
    let workspace: Workspace = connect(drive)?.call_as(CREATE_DRIVE, json!({ "name": name }))?;
    emit_workspace(output, &workspace, "Created")
}

fn list(drive: Option<&Path>, output: Output) -> Result<Exit, CliError> {
    let list: WorkspaceList = connect(drive)?.call_as(LIST, json!({}))?;
    output.emit(&list, || render::workspace_table(&list))?;
    Ok(Exit::Success)
}

fn show(drive: Option<&Path>, id: Option<&str>, output: Output) -> Result<Exit, CliError> {
    let mut client = connect(drive)?;
    let workspace: Workspace = match id {
        Some(id) => client.call_as(GET, json!({ "id": id }))?,
        None => {
            let selected: SelectedWorkspace = client.call_as(SELECTED, json!({}))?;
            selected.workspace.ok_or_else(|| {
                CliError::new(Exit::NotFound, "no workspace is selected")
                    .with_hint("select one with `aidex workspace select <ID|NAME>`")
            })?
        }
    };
    output.emit(&workspace, || render::workspace_details(&workspace, true))?;
    Ok(Exit::Success)
}

fn select(drive: Option<&Path>, id_or_name: &str, output: Output) -> Result<Exit, CliError> {
    let workspace: Workspace =
        connect(drive)?.call_as(SELECT, json!({ "id_or_name": id_or_name }))?;
    emit_workspace(output, &workspace, "Selected")
}

fn remove(
    drive: Option<&Path>,
    id: &str,
    delete_data: bool,
    yes: bool,
    output: Output,
) -> Result<Exit, CliError> {
    let mut client = connect(drive)?;
    if delete_data {
        let workspace: Workspace = client.call_as(GET, json!({ "id": id }))?;
        if workspace.id.as_str() != id.trim() {
            return Err(CliError::new(
                Exit::Usage,
                "`--delete-data` requires the exact workspace id",
            ));
        }
        let question = format!(
            "Delete the AiDEX-created directory of workspace {} ({}) at {}?",
            workspace.name, workspace.id, workspace.path
        );
        if !ephemeral::confirm(&question, yes, output)? {
            return Ok(ephemeral::cancelled(output));
        }
    }
    let removed: Removed =
        client.call_as(REMOVE, json!({ "id": id, "delete_data": delete_data }))?;
    output.emit(&removed, || {
        let workspace = &removed.workspace;
        if removed.deleted_data {
            println!(
                "Removed workspace {} ({}) and deleted its AiDEX-created directory",
                workspace.name, workspace.id
            );
        } else {
            println!(
                "Unregistered workspace {} ({}); no files were deleted",
                workspace.name, workspace.id
            );
            if workspace.kind != aidex_workspace::WorkspaceKind::Host {
                println!(
                    "Its AiDEX-created directory remains at {}; registration is gone, so remove it manually. To delete automatically, use --delete-data in the original remove command.",
                    workspace.path
                );
            }
        }
    })?;
    Ok(Exit::Success)
}

fn discover(drive: Option<&Path>, path: &Path, output: Output) -> Result<Exit, CliError> {
    let path = absolute(path)?;
    let discovery: Discovery = connect(drive)?.call_as(DISCOVER, json!({ "path": path }))?;
    output.emit(&discovery, || render::discovery(&discovery))?;
    Ok(Exit::Success)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Commands};
    use clap::Parser;

    fn parse(args: &[&str]) -> WorkspaceArgs {
        let cli = Cli::try_parse_from(std::iter::once("aidex").chain(args.iter().copied()))
            .unwrap_or_else(|error| panic!("{args:?}: {error}"));
        match cli.command {
            Some(Commands::Workspace(args)) => args,
            other => panic!("expected workspace, got {other:?}"),
        }
    }

    #[test]
    fn workspace_commands_parse_with_the_project_alias_and_global_drive() {
        let args = parse(&["project", "add", "repo", "--name", "demo", "--drive", "d"]);
        assert_eq!(args.drive, Some(PathBuf::from("d")));
        assert!(matches!(
            args.command,
            WorkspaceCommand::Add { ref path, name: Some(ref name) } if path == Path::new("repo") && name == "demo"
        ));
        let args = parse(&[
            "workspace",
            "--drive",
            "d",
            "ephemeral",
            "apply",
            "wsp_1",
            "--path",
            "a",
            "--path",
            "b/c",
            "--yes",
        ]);
        let WorkspaceCommand::Ephemeral {
            command: EphemeralCommand::Apply { id, paths, yes },
        } = args.command
        else {
            panic!("expected ephemeral apply");
        };
        assert_eq!(
            (id.as_str(), paths, yes),
            ("wsp_1", vec!["a".into(), "b/c".into()], true)
        );
        assert!(matches!(
            parse(&["workspace", "show"]).command,
            WorkspaceCommand::Show { id: None }
        ));
        assert_eq!(
            parse(&["workspace", "ephemeral", "discard", "wsp_1"])
                .command
                .label(),
            "aidex workspace ephemeral discard"
        );
        for invalid in [
            &["workspace"][..],
            &["workspace", "add"],
            &["workspace", "select"],
        ] {
            assert!(
                Cli::try_parse_from(std::iter::once("aidex").chain(invalid.iter().copied()))
                    .is_err()
            );
        }
    }
}
