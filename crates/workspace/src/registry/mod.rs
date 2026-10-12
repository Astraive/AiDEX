//! The durable workspace registry and per-workspace confined handles.

mod db;
mod host;

use std::{
    fmt, fs,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard, PoisonError},
};

use aidex_core::{EntityId, Timestamp, WorkspacePath};
use aidex_drive::{DrivePaths, RelativePath};
use rusqlite::Connection;

pub(crate) use db::Record;

use crate::{
    Access, ConfinedRoot, EphemeralLimits, Workspace, WorkspaceAvailability, WorkspaceError,
    WorkspaceKind, WorkspaceRoot,
    discover::repository_info,
    fsutil, marker,
    model::{WORKSPACE_ID_PREFIX, name_key, validate_name},
    platform::{self, display_path},
};

/// Registry database file inside the drive's `private_config` directory.
pub const REGISTRY_FILE: &str = "workspaces.sqlite3";
/// Directory (inside an AiDEX-created workspace directory) holding the workspace files.
pub const TREE_DIR: &str = "tree";
/// Directory under `workspaces/` holding ephemeral workspaces.
pub const EPHEMERAL_DIR: &str = "ephemeral";

/// Durable registry of host, drive, and ephemeral workspaces for one drive.
///
/// Thread-safe (`Send + Sync`): database access is serialized internally, and
/// ephemeral creation/apply/discard and data deletion are serialized against each other.
pub struct WorkspaceRegistry {
    paths: DrivePaths,
    database: PathBuf,
    connection: Mutex<Connection>,
    pub(crate) ephemeral_ops: Mutex<()>,
    pub(crate) limits: EphemeralLimits,
}

impl fmt::Debug for WorkspaceRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkspaceRegistry")
            .field("database", &self.database)
            .finish_non_exhaustive()
    }
}

/// A resolved, confined view of one registered workspace for tool requests.
#[derive(Clone, Debug)]
pub struct WorkspaceHandle {
    workspace: Workspace,
    root: ConfinedRoot,
}

impl WorkspaceHandle {
    /// The stable workspace identity to attach to tool requests and events.
    pub fn id(&self) -> &EntityId {
        &self.workspace.id
    }

    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    pub fn root(&self) -> &ConfinedRoot {
        &self.root
    }

    /// See [`ConfinedRoot::resolve`].
    pub fn resolve(&self, path: &WorkspacePath, access: Access) -> Result<PathBuf, WorkspaceError> {
        self.root.resolve(path, access)
    }

    /// See [`ConfinedRoot::relativize`].
    pub fn relativize(&self, absolute: &Path) -> Result<WorkspacePath, WorkspaceError> {
        self.root.relativize(absolute)
    }
}

impl WorkspaceRegistry {
    /// Open (or create) the registry at `<private_config>/workspaces.sqlite3`.
    ///
    /// The config directory is created only through [`DrivePaths::ensure_dir`]; a
    /// symlinked database file is refused; a newer schema is reported as
    /// [`WorkspaceError::Incompatible`].
    pub fn open(paths: &DrivePaths) -> Result<Self, WorkspaceError> {
        let config = paths.ensure_dir(&paths.config())?;
        let database = config.join(REGISTRY_FILE);
        if fsutil::is_link(&database) {
            return Err(WorkspaceError::Forbidden(format!(
                "refusing to open `{}`: it is a symbolic link",
                database.display()
            )));
        }
        let connection = db::open(&database)?;
        Ok(Self {
            paths: paths.clone(),
            database,
            connection: Mutex::new(connection),
            ephemeral_ops: Mutex::new(()),
            limits: EphemeralLimits::default(),
        })
    }

    /// Replace the ephemeral copy and review limits (tests and embedders).
    #[must_use]
    pub fn with_limits(mut self, limits: EphemeralLimits) -> Self {
        self.limits = limits;
        self
    }

    pub fn paths(&self) -> &DrivePaths {
        &self.paths
    }

    fn connection(&self) -> MutexGuard<'_, Connection> {
        // SQLite transactions keep the database consistent even if a holder panicked.
        self.connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Register a directory the user explicitly selected on the host.
    ///
    /// The directory must exist; it is canonicalized and registered exactly as
    /// selected (discovery never moves the root to a repository root). The drive
    /// root, any directory containing the drive, and AiDEX-managed directories are
    /// refused. A root that is already registered is a conflict. `name` defaults to
    /// the directory name.
    pub fn add_host(&self, path: &Path, name: Option<&str>) -> Result<Workspace, WorkspaceError> {
        let canonical = host::validate_host_root(&self.paths, path)?;
        let shown = display_path(&canonical);
        let root = shown
            .to_str()
            .ok_or_else(|| {
                WorkspaceError::invalid_path(
                    shown.display().to_string(),
                    "the path is not valid Unicode",
                )
            })?
            .to_owned();
        let name = match name {
            Some(name) => validate_name(name)?,
            None => validate_name(
                &canonical
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| root.clone()),
            )?,
        };
        let record = Record {
            id: generate_id()?,
            name,
            kind: WorkspaceKind::Host,
            root: WorkspaceRoot::Host(root),
            created_at: Timestamp::now(),
            last_opened_at: None,
        };
        self.insert_record(&record)?;
        Ok(self.view(record))
    }

    /// Create an empty drive workspace at `workspaces/<id>/tree` with an ownership
    /// marker in `workspaces/<id>/`.
    pub fn create_drive(&self, name: &str) -> Result<Workspace, WorkspaceError> {
        let name = validate_name(name)?;
        if db::by_name(&self.connection(), &name_key(&name))?.is_some() {
            return Err(WorkspaceError::Conflict(format!(
                "a workspace named `{name}` is already registered; choose another name"
            )));
        }
        let id = generate_id()?;
        let parent = self.paths.ensure_dir(&self.paths.workspaces())?;
        let base = parent.join(id.as_str());
        self.create_owned_dir(&base, &id, "drive")?;
        let relative = RelativePath::new(format!("{id}/{TREE_DIR}"))
            .map_err(|error| WorkspaceError::Corrupt(format!("drive root: {error}")))?;
        let record = Record {
            id: id.clone(),
            name,
            kind: WorkspaceKind::Drive,
            root: WorkspaceRoot::Drive(relative),
            created_at: Timestamp::now(),
            last_opened_at: None,
        };
        if let Err(error) = self.insert_record(&record) {
            let _ = fs::remove_dir_all(&base);
            return Err(error);
        }
        Ok(self.view(record))
    }

    /// Every registered workspace in creation order, with availability computed now.
    pub fn list(&self) -> Result<Vec<Workspace>, WorkspaceError> {
        let records = db::list(&self.connection())?;
        Ok(records
            .into_iter()
            .map(|record| self.view(record))
            .collect())
    }

    pub fn get(&self, id: &EntityId) -> Result<Workspace, WorkspaceError> {
        self.record(id).map(|record| self.view(record))
    }

    /// Look up by exact id, else by case-insensitive name.
    pub fn find(&self, reference: &str) -> Result<Workspace, WorkspaceError> {
        let reference = reference.trim();
        let connection = self.connection();
        if let Ok(id) = EntityId::new(reference)
            && let Some(record) = db::get(&connection, &id)?
        {
            drop(connection);
            return Ok(self.view(record));
        }
        let record = db::by_name(&connection, &name_key(reference))?;
        drop(connection);
        record
            .map(|record| self.view(record))
            .ok_or_else(|| WorkspaceError::NotFound(reference.to_owned()))
    }

    /// Persist `id` as the selected workspace (survives restarts) and record the open.
    pub fn select(&self, id: &EntityId) -> Result<Workspace, WorkspaceError> {
        if !db::select(&mut self.connection(), id, &Timestamp::now())? {
            return Err(WorkspaceError::NotFound(id.to_string()));
        }
        self.get(id)
    }

    /// The persisted selection, if any.
    pub fn selected(&self) -> Result<Option<Workspace>, WorkspaceError> {
        let id = db::selected(&self.connection())?;
        id.map(|id| self.get(&id)).transpose()
    }

    /// Record that the workspace was opened now.
    pub fn touch_opened(&self, id: &EntityId) -> Result<Workspace, WorkspaceError> {
        if !db::touch(&self.connection(), id, &Timestamp::now())? {
            return Err(WorkspaceError::NotFound(id.to_string()));
        }
        self.get(id)
    }

    /// Unregister a workspace. No files are deleted (host data never is); use
    /// [`Self::delete_data`] or [`Self::discard`] to delete AiDEX-created data.
    /// A workspace with ephemeral copies must have them discarded first.
    pub fn remove(&self, id: &EntityId) -> Result<Workspace, WorkspaceError> {
        let record = self.record(id)?;
        let connection = self.connection();
        let children = db::children(&connection, id)?;
        if !children.is_empty() {
            return Err(WorkspaceError::Conflict(format!(
                "workspace {id} has ephemeral copies ({}); discard them first",
                children.join(", ")
            )));
        }
        db::delete(&connection, id)?;
        drop(connection);
        Ok(self.view(record))
    }

    /// Delete an AiDEX-created (drive or ephemeral) workspace's directory, then
    /// unregister it. Host workspaces are refused: AiDEX never deletes host data.
    /// The directory is deleted only if its ownership marker names this workspace.
    pub fn delete_data(&self, id: &EntityId) -> Result<Workspace, WorkspaceError> {
        let _guard = self
            .ephemeral_ops
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let record = self.record(id)?;
        if record.kind == WorkspaceKind::Host {
            return Err(WorkspaceError::Forbidden(format!(
                "AiDEX never deletes host workspace data; `remove` unregisters {id} and leaves its files"
            )));
        }
        self.delete_owned(record)
    }

    /// A confined root for the workspace on this host.
    ///
    /// Fails with [`WorkspaceError::RootUnavailable`] when the root is missing
    /// (never recreated), and re-applies the host-root drive checks because the
    /// drive may have moved since the workspace was added.
    pub fn confined_root(&self, id: &EntityId) -> Result<ConfinedRoot, WorkspaceError> {
        let record = self.record(id)?;
        self.confine(&record)
    }

    /// The workspace record together with its confined root.
    pub fn handle(&self, id: &EntityId) -> Result<WorkspaceHandle, WorkspaceError> {
        let record = self.record(id)?;
        let root = self.confine(&record)?;
        Ok(WorkspaceHandle {
            workspace: self.view(record),
            root,
        })
    }

    pub(crate) fn record(&self, id: &EntityId) -> Result<Record, WorkspaceError> {
        db::get(&self.connection(), id)?.ok_or_else(|| WorkspaceError::NotFound(id.to_string()))
    }

    pub(crate) fn insert_record(&self, record: &Record) -> Result<(), WorkspaceError> {
        db::insert(
            &mut self.connection(),
            record,
            &name_key(&record.name),
            &root_key_for(&record.root),
        )
    }

    pub(crate) fn name_taken(&self, name: &str) -> Result<bool, WorkspaceError> {
        Ok(db::by_name(&self.connection(), &name_key(name))?.is_some())
    }

    pub(crate) fn confine(&self, record: &Record) -> Result<ConfinedRoot, WorkspaceError> {
        let unavailable = |reason: String| WorkspaceError::RootUnavailable {
            workspace: record.id.to_string(),
            reason,
        };
        if let WorkspaceAvailability::Unavailable { reason } = self.availability(record) {
            return Err(unavailable(reason));
        }
        let location = self.location(&record.root);
        let root = ConfinedRoot::new(&location).map_err(|error| match error {
            WorkspaceError::RootUnavailable { reason, .. } => unavailable(reason),
            other => other,
        })?;
        match &record.root {
            WorkspaceRoot::Host(_) => host::check_not_drive_state(&self.paths, root.root())?,
            WorkspaceRoot::Drive(relative) => {
                // Every directory from the drive's `workspaces/` down to the root must be
                // a real directory on the drive, so the root cannot be redirected.
                let escapes = || WorkspaceError::EscapesRoot(record.root.as_str().to_owned());
                let workspaces = self.paths.workspaces();
                let mut current = workspaces.clone();
                if fsutil::is_link(&current) {
                    return Err(escapes());
                }
                for segment in relative.as_str().split('/') {
                    current.push(segment);
                    if fsutil::is_link(&current) {
                        return Err(escapes());
                    }
                }
                let canonical = |path: &Path| {
                    fs::canonicalize(path).map_err(|error| {
                        WorkspaceError::io(format!("cannot resolve `{}`", path.display()), error)
                    })
                };
                let drive_root = canonical(self.paths.root())?;
                let workspaces = canonical(&workspaces)?;
                if !platform::is_within(&workspaces, &drive_root)
                    || !platform::is_within(root.root(), &workspaces)
                {
                    return Err(escapes());
                }
            }
        }
        Ok(root)
    }

    /// The root's absolute location on this host (lexical; no I/O).
    pub(crate) fn location(&self, root: &WorkspaceRoot) -> PathBuf {
        match root {
            WorkspaceRoot::Host(path) => PathBuf::from(path),
            WorkspaceRoot::Drive(relative) => relative.resolve_under(self.paths.workspaces()),
        }
    }

    fn availability(&self, record: &Record) -> WorkspaceAvailability {
        let location = self.location(&record.root);
        let unavailable = |reason: &str| WorkspaceAvailability::Unavailable {
            reason: reason.to_owned(),
        };
        if !location.is_absolute() {
            return unavailable("the root was added on a different operating system");
        }
        let metadata = fs::symlink_metadata(&location);
        match metadata {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                unavailable("the workspace root directory was replaced by a link")
            }
            Ok(metadata) if metadata.is_dir() => WorkspaceAvailability::Available,
            Ok(_) => unavailable("the root is not a directory"),
            Err(error) if error.kind() == ErrorKind::NotFound => match record.root {
                WorkspaceRoot::Host(_) => {
                    unavailable("the root directory does not exist on this host")
                }
                WorkspaceRoot::Drive(_) => unavailable("the drive workspace directory is missing"),
            },
            Err(error) => WorkspaceAvailability::Unavailable {
                reason: format!("the root cannot be accessed: {error}"),
            },
        }
    }

    pub(crate) fn view(&self, record: Record) -> Workspace {
        let location = self.location(&record.root);
        let availability = self.availability(&record);
        let repository = (availability == WorkspaceAvailability::Available)
            .then(|| fs::canonicalize(&location).ok())
            .flatten()
            .and_then(|canonical| {
                repository_info(&canonical, matches!(record.kind, WorkspaceKind::Host))
            });
        Workspace {
            id: record.id,
            name: record.name,
            kind: record.kind,
            root: record.root,
            created_at: record.created_at,
            last_opened_at: record.last_opened_at,
            path: display_path(&location).to_string_lossy().into_owned(),
            availability,
            repository,
        }
    }

    /// The directory AiDEX owns for a drive or ephemeral workspace.
    pub(crate) fn owned_dir(
        &self,
        record: &Record,
    ) -> Result<(PathBuf, &'static str), WorkspaceError> {
        let workspaces = self.paths.workspaces();
        let (base, kind, expected) = match record.kind {
            WorkspaceKind::Drive => (
                workspaces.join(record.id.as_str()),
                "drive",
                format!("{}/{TREE_DIR}", record.id),
            ),
            WorkspaceKind::Ephemeral { .. } => (
                workspaces.join(EPHEMERAL_DIR).join(record.id.as_str()),
                "ephemeral",
                format!("{EPHEMERAL_DIR}/{}/{TREE_DIR}", record.id),
            ),
            WorkspaceKind::Host => {
                return Err(WorkspaceError::Forbidden(
                    "host workspaces have no AiDEX-owned directory".into(),
                ));
            }
        };
        if record.root.as_str() != expected {
            return Err(WorkspaceError::Corrupt(format!(
                "workspace {} has unexpected root `{}`",
                record.id,
                record.root.as_str()
            )));
        }
        Ok((base, kind))
    }

    /// Create `base` (which must not exist), its ownership marker, and `tree/`.
    pub(crate) fn create_owned_dir(
        &self,
        base: &Path,
        id: &EntityId,
        kind: &str,
    ) -> Result<PathBuf, WorkspaceError> {
        fs::create_dir(base).map_err(|error| {
            if error.kind() == ErrorKind::AlreadyExists {
                WorkspaceError::Conflict(format!("`{}` already exists", base.display()))
            } else {
                WorkspaceError::io(format!("cannot create `{}`", base.display()), error)
            }
        })?;
        let tree = base.join(TREE_DIR);
        let created = marker::write(base, id, kind).and_then(|()| {
            fs::create_dir(&tree).map_err(|error| {
                WorkspaceError::io(format!("cannot create `{}`", tree.display()), error)
            })
        });
        if let Err(error) = created {
            let _ = fs::remove_dir_all(base);
            return Err(error);
        }
        Ok(tree)
    }

    /// Verify ownership, delete the owned directory, then unregister the record.
    ///
    /// The tree is removed before the marker so an interrupted delete can be retried.
    /// Links inside the tree are removed, never followed. An already-missing directory
    /// only unregisters the record.
    pub(crate) fn delete_owned(&self, record: Record) -> Result<Workspace, WorkspaceError> {
        let (base, kind) = self.owned_dir(&record)?;
        let connection = self.connection();
        let children = db::children(&connection, &record.id)?;
        drop(connection);
        if !children.is_empty() {
            return Err(WorkspaceError::Conflict(format!(
                "workspace {} has ephemeral copies ({}); discard them first",
                record.id,
                children.join(", ")
            )));
        }
        let workspaces = self.paths.workspaces();
        for ancestor in [workspaces.clone(), workspaces.join(EPHEMERAL_DIR)] {
            if base.starts_with(&ancestor) && fsutil::is_link(&ancestor) {
                return Err(WorkspaceError::OwnershipMarker(base.clone()));
            }
        }
        match fs::symlink_metadata(&base) {
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(WorkspaceError::io(
                    format!("cannot inspect `{}`", base.display()),
                    error,
                ));
            }
            Ok(_) => {
                marker::verify(&base, &record.id, kind)?;
                remove_tree(&base.join(TREE_DIR))?;
                fs::remove_dir_all(&base).map_err(|error| {
                    WorkspaceError::io(format!("cannot delete `{}`", base.display()), error)
                })?;
            }
        }
        let view = self.view(record.clone());
        db::delete(&self.connection(), &record.id)?;
        Ok(view)
    }
}

/// Remove a directory tree without following links; a missing tree is fine.
fn remove_tree(path: &Path) -> Result<(), WorkspaceError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(WorkspaceError::io(
            format!("cannot inspect `{}`", path.display()),
            error,
        )),
        Ok(metadata) if metadata.file_type().is_symlink() => remove_link(path),
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path).map_err(|error| {
            WorkspaceError::io(format!("cannot delete `{}`", path.display()), error)
        }),
        Ok(_) => fs::remove_file(path).map_err(|error| {
            WorkspaceError::io(format!("cannot delete `{}`", path.display()), error)
        }),
    }
}

/// Remove a symbolic link or junction itself (never its target).
fn remove_link(path: &Path) -> Result<(), WorkspaceError> {
    fs::remove_file(path)
        .or_else(|error| {
            // Windows directory links and junctions are removed as directories.
            if cfg!(windows) {
                fs::remove_dir(path)
            } else {
                Err(error)
            }
        })
        .map_err(|error| {
            WorkspaceError::io(format!("cannot delete link `{}`", path.display()), error)
        })
}

pub(crate) fn generate_id() -> Result<EntityId, WorkspaceError> {
    EntityId::generate(WORKSPACE_ID_PREFIX).map_err(|error| {
        WorkspaceError::io(
            "cannot generate a workspace id",
            io::Error::other(error.to_string()),
        )
    })
}

fn root_key_for(root: &WorkspaceRoot) -> String {
    match root {
        WorkspaceRoot::Host(path) => format!("host:{}", platform::root_key(Path::new(path))),
        WorkspaceRoot::Drive(relative) => format!("drive:{}", relative.as_str().to_lowercase()),
    }
}
