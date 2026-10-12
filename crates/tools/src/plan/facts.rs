//! Facts a plan computes for policy evaluation, convertible to policy intents.

use aidex_policy::{Action, IntentAttributes, NetworkTarget, PathClass, PathScope, ShellCommand};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How a planned call uses a path.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathAccess {
    /// Read file content.
    Read,
    /// List or search beneath a directory.
    List,
    /// Modify an existing file.
    Write,
    /// Create or replace a file.
    Create,
    /// Delete the entry.
    Delete,
    /// Working directory of a launched process.
    Cwd,
}

impl PathAccess {
    /// Whether the access modifies the workspace.
    pub fn is_write(self) -> bool {
        matches!(self, Self::Write | Self::Create | Self::Delete)
    }
}

/// What a filesystem entry is (links are never followed when reporting this).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    File,
    Directory,
    /// A symbolic link or junction.
    Symlink,
    /// A device, socket, FIFO, or other special file.
    Other,
}

impl EntryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Directory => "directory",
            Self::Symlink => "symbolic link",
            Self::Other => "special file",
        }
    }
}

/// One path a plan touches, resolved and confined at plan time.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlanPath {
    /// Workspace-relative path with `/` separators; `""` is the workspace root.
    pub path: String,
    /// The resolved absolute path, as displayed on approval cards.
    pub display: String,
    pub access: PathAccess,
    /// Always `true` for built-in tools: paths outside the workspace are refused at
    /// plan time. Kept for tools that operate on explicitly mounted host paths.
    pub inside_workspace: bool,
    pub exists: bool,
    pub kind: Option<EntryKind>,
    pub size: Option<u64>,
    /// SHA-256 of the content at plan time (files that will be modified or read
    /// for a preview); execution refuses with `Conflict` if it changed.
    pub sha256: Option<String>,
    /// The name looks credential-bearing (`.env`, private keys, cloud credentials).
    /// A hint: the broker decides the final [`PathClass`].
    pub sensitive: bool,
}

/// Network use of a planned call.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct NetworkFacts {
    /// Classified destinations known at plan time.
    pub targets: Vec<NetworkTarget>,
    /// Destinations known only by name (for example the git remote `origin`); the
    /// broker must treat them as unknown destinations.
    pub unresolved: Vec<String>,
}

/// Policy-relevant facts computed by `plan`. Every value is derived by the tool from
/// the validated input and the workspace, never copied from model output.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct PlanFacts {
    pub paths: Vec<PlanPath>,
    /// The classified command line for process-running tools.
    pub command: Option<ShellCommand>,
    pub network: Option<NetworkFacts>,
    /// The call has an irreversible destructive effect (recursive delete).
    pub destructive: bool,
    /// The call writes to a remote system (push).
    pub remote_write: bool,
    /// Further actions the call performs that policy must evaluate separately (for
    /// example `policy.change` for writes beneath `.aidex/`).
    pub implied_actions: Vec<Action>,
}

impl PlanFacts {
    /// Paths the call modifies.
    pub fn write_paths(&self) -> impl Iterator<Item = &PlanPath> {
        self.paths.iter().filter(|path| path.access.is_write())
    }

    /// Policy intent attributes. Paths flagged [`PlanPath::sensitive`] become
    /// [`PathClass::Sensitive`]; the broker may raise classes further (for example
    /// to `Protected`) but should never lower them. A single network target becomes
    /// the intent destination; otherwise the destination is left unknown.
    pub fn intent_attributes(&self) -> IntentAttributes {
        let paths = self
            .paths
            .iter()
            .map(|path| {
                let scope = if path.inside_workspace {
                    PathScope::inside(&path.display, &path.path)
                } else {
                    PathScope::outside(&path.display)
                };
                if path.sensitive {
                    scope.with_class(PathClass::Sensitive)
                } else {
                    scope
                }
            })
            .collect();
        let network = self
            .network
            .as_ref()
            .filter(|network| network.unresolved.is_empty() && network.targets.len() == 1)
            .and_then(|network| network.targets.first().cloned());
        IntentAttributes {
            paths,
            command: self.command.clone(),
            network,
            secret: None,
            model_locality: None,
            remote_write: self.remote_write,
            destructive: self.destructive,
        }
    }
}
