//! Workspace filesystem tools and the confined I/O helpers they share.

mod delete;
mod list;
mod patch;
mod read;
mod search;
mod unified;
mod write;

use std::{
    fs::{self, File, OpenOptions},
    io::{self, ErrorKind, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::UNIX_EPOCH,
};

use aidex_core::WorkspacePath;
use aidex_policy::{Action, PathClass, PathScope};
use aidex_workspace::{Access, ConfinedRoot, parse_workspace_path};
use sha2::{Digest, Sha256};

pub(crate) use delete::FsDelete;
pub(crate) use list::{FsGlob, FsList};
pub(crate) use patch::FsPatch;
pub(crate) use read::FsRead;
pub(crate) use search::FsSearch;
pub(crate) use write::FsWrite;

use crate::{EntryKind, InputError, PathAccess, PlanPath, ToolContext, ToolError, walk};

/// Files larger than this are not hashed (their plan facts carry no SHA-256).
pub(crate) const MAX_HASH_BYTES: u64 = 256 * 1024 * 1024;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

/// Lowercase hexadecimal SHA-256.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// Streaming SHA-256 of a file, or `None` when it is larger than [`MAX_HASH_BYTES`].
pub(crate) fn hash_file(path: &Path) -> io::Result<Option<String>> {
    let mut file = File::open(path)?;
    if file.metadata()?.len() > MAX_HASH_BYTES {
        return Ok(None);
    }
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        hasher.update(&buffer[..read]);
    }
    Ok(Some(hex(&hasher.finalize())))
}

/// Parse a caller-supplied workspace-relative path for `field`.
pub(crate) fn parse_path(field: &str, raw: &str) -> Result<WorkspacePath, ToolError> {
    parse_workspace_path(raw).map_err(|error| ToolError::from_workspace(field, error))
}

/// Parse an optional directory; `None`, `""`, and `"."` mean the workspace root.
pub(crate) fn parse_dir(
    field: &str,
    raw: Option<&str>,
) -> Result<Option<WorkspacePath>, ToolError> {
    match raw {
        None | Some("" | ".") => Ok(None),
        Some(raw) => parse_path(field, raw.trim_end_matches('/')).map(Some),
    }
}

/// Resolve `path` inside the workspace for `access`.
pub(crate) fn resolve(
    ctx: &ToolContext,
    field: &str,
    path: &WorkspacePath,
    access: Access,
) -> Result<PathBuf, ToolError> {
    ctx.root
        .resolve(path, access)
        .map_err(|error| ToolError::from_workspace(field, error))
}

/// The policy scope of the workspace path `raw` (a file tool's `path`
/// argument) as a plan of it would report it ([`crate::PlanFacts::intent_attributes`]):
/// resolved like a read (an in-workspace link followed), or like a create
/// when nothing is there yet, and classed [`PathClass::Sensitive`] when its
/// name looks credential-bearing ([`crate::looks_sensitive`]); `""` and `"."`
/// are the workspace root, as for a listing. For a broker that checks policy
/// on a path whose call could not be planned, before the planning error
/// (which may depend on the file's content) reaches a model. Malformed paths,
/// paths outside the workspace, and paths that cannot be resolved either way
/// are errors.
pub fn path_scope(root: &ConfinedRoot, raw: &str) -> Result<PathScope, ToolError> {
    let Some(path) = parse_dir("path", Some(raw))? else {
        return Ok(PathScope::inside(
            root.display_root().display().to_string(),
            "",
        ));
    };
    let absolute = root
        .resolve(&path, Access::Read)
        .or_else(|_| root.resolve(&path, Access::Create))
        .map_err(|error| ToolError::from_workspace("path", error))?;
    let scope = PathScope::inside(
        aidex_workspace::display_path(&absolute)
            .display()
            .to_string(),
        path.as_str(),
    );
    Ok(if crate::sensitive::looks_sensitive(path.as_str()) {
        scope.with_class(PathClass::Sensitive)
    } else {
        scope
    })
}

/// Refuse modifications inside git metadata (hooks there would run code later).
pub(crate) fn reject_git_internal(path: &WorkspacePath) -> Result<(), ToolError> {
    if path.as_str().split('/').any(walk::is_git_dir) {
        return Err(ToolError::Forbidden {
            path: path.as_str().to_owned(),
            reason: "git metadata is never modified by file tools".into(),
        });
    }
    Ok(())
}

/// Actions a write to `path` implies besides the tool's own: anything beneath
/// `.aidex/` (project rules, AiDEX project state) is a `policy.change`.
pub(crate) fn implied_actions(path: &WorkspacePath) -> Vec<Action> {
    let first = path.as_str().split('/').next().unwrap_or_default();
    if first.eq_ignore_ascii_case(".aidex") {
        vec![Action::PolicyChange]
    } else {
        Vec::new()
    }
}

/// What is at a path, without following a leaf link.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Stat {
    pub kind: Option<EntryKind>,
    pub size: u64,
    /// Modification time in nanoseconds since the Unix epoch (0 when unknown).
    pub modified_ns: u128,
}

impl Stat {
    pub fn exists(&self) -> bool {
        self.kind.is_some()
    }
}

pub(crate) fn stat(path: &Path) -> Result<Stat, ToolError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            let file_type = metadata.file_type();
            let kind = if file_type.is_symlink() {
                EntryKind::Symlink
            } else if file_type.is_dir() {
                EntryKind::Directory
            } else if file_type.is_file() {
                EntryKind::File
            } else {
                EntryKind::Other
            };
            let modified_ns = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |duration| duration.as_nanos());
            Ok(Stat {
                kind: Some(kind),
                size: if kind == EntryKind::File {
                    metadata.len()
                } else {
                    0
                },
                modified_ns,
            })
        }
        Err(error) if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
            Ok(Stat {
                kind: None,
                size: 0,
                modified_ns: 0,
            })
        }
        Err(error) => Err(ToolError::io("cannot inspect the target", error)),
    }
}

/// A plan path fact for `path` (`None` = the workspace root).
pub(crate) fn plan_path(
    path: Option<&WorkspacePath>,
    absolute: &Path,
    access: PathAccess,
    stat: &Stat,
    sha256: Option<String>,
) -> PlanPath {
    let relative = path.map(WorkspacePath::as_str).unwrap_or_default();
    PlanPath {
        path: relative.to_owned(),
        display: aidex_workspace::display_path(absolute)
            .display()
            .to_string(),
        access,
        inside_workspace: true,
        exists: stat.exists(),
        kind: stat.kind,
        size: stat.exists().then_some(stat.size),
        sha256,
        sensitive: crate::sensitive::looks_sensitive(relative),
    }
}

/// Require a regular file at `path` (links inside the workspace are resolved by
/// `resolve` before this is called).
pub(crate) fn require_file(path: &WorkspacePath, stat: &Stat) -> Result<(), ToolError> {
    match stat.kind {
        Some(EntryKind::File) => Ok(()),
        None => Err(ToolError::NotFound {
            path: path.as_str().to_owned(),
        }),
        Some(kind) => Err(ToolError::WrongKind {
            path: path.as_str().to_owned(),
            expected: "file",
            found: kind.as_str(),
        }),
    }
}

/// Read a whole file of at most `limit` bytes.
pub(crate) fn read_capped(path: &Path, limit: u64, what: &str) -> Result<Vec<u8>, ToolError> {
    let file =
        File::open(path).map_err(|error| ToolError::io(format!("cannot open {what}"), error))?;
    let size = file
        .metadata()
        .map_err(|error| ToolError::io(format!("cannot inspect {what}"), error))?
        .len();
    if size > limit {
        return Err(ToolError::TooLarge {
            what: what.to_owned(),
            size,
            limit,
        });
    }
    let mut bytes = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| ToolError::io(format!("cannot read {what}"), error))?;
    if bytes.len() as u64 > limit {
        return Err(ToolError::TooLarge {
            what: what.to_owned(),
            size: bytes.len() as u64,
            limit,
        });
    }
    Ok(bytes)
}

/// Hash check: the current content of a planned file still matches the plan.
pub(crate) fn verify_unchanged(
    path: &WorkspacePath,
    planned: &PlanPath,
    current: &Stat,
    current_bytes: Option<&[u8]>,
) -> Result<(), ToolError> {
    let shown = path.as_str();
    match (planned.exists, current.exists()) {
        (false, true) => return Err(ToolError::conflict(shown, "it was created")),
        (true, false) => return Err(ToolError::conflict(shown, "it was deleted")),
        (false, false) => return Ok(()),
        (true, true) => {}
    }
    if planned.kind != current.kind {
        return Err(ToolError::conflict(shown, "its type changed"));
    }
    if let (Some(expected), Some(bytes)) = (&planned.sha256, current_bytes)
        && *expected != sha256_hex(bytes)
    {
        return Err(ToolError::conflict(shown, "its content changed"));
    }
    Ok(())
}

/// Create the missing parent directories of `path` inside the workspace, checking
/// confinement after each one, and return the re-resolved target.
pub(crate) fn ensure_parents(
    ctx: &ToolContext,
    path: &WorkspacePath,
) -> Result<PathBuf, ToolError> {
    let components: Vec<&str> = path.as_str().split('/').collect();
    for depth in 1..components.len() {
        let prefix = WorkspacePath::new(components[..depth].join("/"))
            .map_err(|_| ToolError::invalid("path", InputError::InvalidPath, "invalid parent"))?;
        let directory = resolve(ctx, "path", &prefix, Access::Create)?;
        match fs::symlink_metadata(&directory) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(ToolError::WrongKind {
                    path: prefix.as_str().to_owned(),
                    expected: "directory",
                    found: "file",
                });
            }
            Err(error) if error.kind() == ErrorKind::NotFound => match fs::create_dir(&directory) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(ToolError::io(
                        format!("cannot create directory `{}`", prefix.as_str()),
                        error,
                    ));
                }
            },
            Err(error) => {
                return Err(ToolError::io(
                    format!("cannot inspect `{}`", prefix.as_str()),
                    error,
                ));
            }
        }
        // Re-check: the directory (as created or found) must still resolve inside.
        let checked = resolve(ctx, "path", &prefix, Access::Read)?;
        if !checked.is_dir() {
            return Err(ToolError::EscapesRoot {
                path: prefix.as_str().to_owned(),
            });
        }
    }
    resolve(ctx, "path", path, Access::Create)
}

/// Write `bytes` to `destination` atomically: exclusive temporary sibling, fsync,
/// permissions copied from the file being replaced (Unix mode bits), rename.
pub(crate) fn atomic_write(destination: &Path, bytes: &[u8]) -> Result<(), ToolError> {
    let parent = destination.parent().ok_or_else(|| {
        ToolError::io(
            "cannot write",
            io::Error::new(ErrorKind::InvalidInput, "no parent"),
        )
    })?;
    let (temporary, mut file) = loop {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".aidex-tmp-{}-{sequence}", std::process::id()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => break (candidate, file),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(ToolError::io("cannot create a temporary file", error)),
        }
    };
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        copy_mode(destination, &temporary)?;
        fs::rename(&temporary, destination)?;
        sync_parent(parent);
        Ok(())
    })();
    result.map_err(|error: io::Error| {
        let _ = fs::remove_file(&temporary);
        ToolError::io("cannot write the file", error)
    })
}

fn copy_mode(existing: &Path, temporary: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = fs::metadata(existing) {
            let mode = metadata.permissions().mode() & 0o7777;
            fs::set_permissions(temporary, fs::Permissions::from_mode(mode))?;
        }
    }
    #[cfg(not(unix))]
    let _ = (existing, temporary);
    Ok(())
}

/// Best-effort directory fsync so a completed rename survives power loss (Unix).
fn sync_parent(parent: &Path) {
    #[cfg(unix)]
    if let Ok(directory) = File::open(parent) {
        let _ = directory.sync_all();
    }
    #[cfg(not(unix))]
    let _ = parent;
}
