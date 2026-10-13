//! Path confinement: resolve workspace-relative paths to absolute paths that are
//! guaranteed (at resolution time) to stay inside a canonical workspace root.

use std::{
    ffi::OsString,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use aidex_core::WorkspacePath;

use crate::{
    WorkspaceError,
    platform::{self, ENFORCE_WINDOWS_RULES, windows_component_issue},
};

/// Why a root whose path now names another directory is unavailable.
pub(crate) const RELOCATED_ROOT: &str = "the root path now resolves to a different location";

/// What the caller intends to do with a resolved path.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Access {
    /// Read an existing file or directory; it must exist.
    Read,
    /// Modify an existing file or directory in place; it must exist.
    Write,
    /// Create or replace the leaf. Missing parents are allowed (the caller creates
    /// them); the deepest existing ancestor must resolve inside the root and an
    /// existing leaf must not be a symbolic link or junction.
    Create,
}

/// A canonical workspace root that every tool path is resolved against.
///
/// Resolution is a point-in-time check: a process that can modify the workspace
/// concurrently could replace a checked directory with a link before the caller
/// opens the returned path. Callers resolve immediately before each operation and
/// re-resolve after creating intermediate directories; OS-level confinement belongs
/// to the sandbox.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfinedRoot {
    root: PathBuf,
}

impl ConfinedRoot {
    /// Canonicalize `root`, which must be an existing directory.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, WorkspaceError> {
        let requested = root.into();
        let canonical = fs::canonicalize(&requested).map_err(|error| {
            if error.kind() == ErrorKind::NotFound {
                WorkspaceError::RootUnavailable {
                    workspace: requested.display().to_string(),
                    reason: "the root directory does not exist on this host".into(),
                }
            } else {
                WorkspaceError::io(
                    format!("cannot resolve workspace root `{}`", requested.display()),
                    error,
                )
            }
        })?;
        let metadata = fs::metadata(&canonical).map_err(|error| {
            WorkspaceError::io(
                format!("cannot inspect workspace root `{}`", canonical.display()),
                error,
            )
        })?;
        if !metadata.is_dir() {
            return Err(WorkspaceError::RootUnavailable {
                workspace: requested.display().to_string(),
                reason: "the root is not a directory".into(),
            });
        }
        Ok(Self { root: canonical })
    }

    /// The canonical root (on Windows usually with a `\\?\` prefix).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The root without a Windows verbatim prefix, for display.
    pub fn display_root(&self) -> PathBuf {
        platform::display_path(&self.root)
    }

    /// Whether `absolute` is the root or lies lexically beneath it.
    pub fn contains(&self, absolute: &Path) -> bool {
        platform::is_within(absolute, &self.root)
    }

    /// Resolve `path` for `access`, returning an absolute path inside the root.
    ///
    /// Components are checked one at a time from the root: a real directory or file
    /// is a physical child of the (canonical) directory above it, and a symbolic
    /// link or junction is followed only when its fully resolved target is inside
    /// the root. Unresolvable links (dangling or looping) fail closed with
    /// [`WorkspaceError::EscapesRoot`]. Read/Write require the path to exist; Create
    /// allows missing components (the caller creates them) but refuses a leaf that
    /// is itself a link. A root whose path no longer resolves to itself (a parent
    /// directory was replaced by a link, or a mount was remapped so the path resolves elsewhere) is
    /// [`WorkspaceError::RootUnavailable`].
    pub fn resolve(&self, path: &WorkspacePath, access: Access) -> Result<PathBuf, WorkspaceError> {
        validate_components(path)?;
        let escapes = || WorkspaceError::EscapesRoot(path.as_str().to_owned());
        let unavailable = |reason: &str| WorkspaceError::RootUnavailable {
            workspace: self.display_root().display().to_string(),
            reason: reason.to_owned(),
        };
        match fs::symlink_metadata(&self.root) {
            // The canonical root was replaced by a link after it was resolved.
            Ok(metadata) if metadata.file_type().is_symlink() => return Err(escapes()),
            Ok(metadata) if metadata.is_dir() => {}
            _ => return Err(unavailable("the root directory no longer exists")),
        }
        // The root is canonical, so any other resolution means one of its parent
        // directories now leads somewhere else.
        match fs::canonicalize(&self.root) {
            Ok(current) if platform::same_location(&current, &self.root) => {}
            Ok(_) => return Err(unavailable(RELOCATED_ROOT)),
            Err(_) => return Err(unavailable("the root directory can no longer be resolved")),
        }
        let segments: Vec<&str> = path.as_str().split('/').collect();
        let last = segments.len() - 1;
        let mut current = self.root.clone();
        let mut missing: &[&str] = &[];
        for (index, segment) in segments.iter().enumerate() {
            let candidate = current.join(segment);
            let metadata = match fs::symlink_metadata(&candidate) {
                Ok(metadata) => metadata,
                Err(error)
                    if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) =>
                {
                    missing = &segments[index..];
                    break;
                }
                Err(error) => {
                    return Err(WorkspaceError::io(
                        format!("cannot inspect `{}`", path.as_str()),
                        error,
                    ));
                }
            };
            if metadata.file_type().is_symlink() {
                let target = fs::canonicalize(&candidate).map_err(|_| escapes())?;
                if !self.contains(&target) {
                    return Err(escapes());
                }
                if index == last && access == Access::Create {
                    return Err(WorkspaceError::SymlinkLeaf(path.clone()));
                }
                current = target;
            } else {
                current = candidate;
            }
            if index < last && !current.is_dir() {
                return Err(match access {
                    Access::Create => WorkspaceError::invalid_path(
                        path.as_str(),
                        "a parent component is not a directory",
                    ),
                    Access::Read | Access::Write => WorkspaceError::PathNotFound(path.clone()),
                });
            }
        }
        if !missing.is_empty() {
            if access != Access::Create {
                return Err(WorkspaceError::PathNotFound(path.clone()));
            }
            current.extend(missing);
        }
        Ok(current)
    }

    /// The workspace-relative path of an absolute path inside the root.
    ///
    /// Existing paths are canonicalized first (so links are resolved and checked);
    /// a missing tail is validated lexically below its deepest existing ancestor.
    pub fn relativize(&self, absolute: &Path) -> Result<WorkspacePath, WorkspaceError> {
        let shown = || absolute.display().to_string();
        if !absolute.is_absolute() {
            return Err(WorkspaceError::invalid_path(
                shown(),
                "an absolute path is required",
            ));
        }
        let mut existing = absolute.to_path_buf();
        let mut missing: Vec<OsString> = Vec::new();
        let canonical = loop {
            match fs::canonicalize(&existing) {
                Ok(canonical) => break canonical,
                Err(error)
                    if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) =>
                {
                    if fs::symlink_metadata(&existing).is_ok() {
                        // Present but unresolvable: a dangling link cannot be verified.
                        return Err(WorkspaceError::EscapesRoot(shown()));
                    }
                    let Some(name) = existing.file_name() else {
                        return Err(WorkspaceError::EscapesRoot(shown()));
                    };
                    missing.push(name.to_os_string());
                    if !existing.pop() {
                        return Err(WorkspaceError::EscapesRoot(shown()));
                    }
                }
                Err(error) => {
                    return Err(WorkspaceError::io(
                        format!("cannot resolve `{}`", shown()),
                        error,
                    ));
                }
            }
        };
        let mut relative = platform::strip_root(&canonical, &self.root)
            .ok_or_else(|| WorkspaceError::EscapesRoot(shown()))?;
        relative.extend(missing.into_iter().rev());
        let mut parts = Vec::with_capacity(relative.len());
        for part in &relative {
            let part = part.to_str().ok_or_else(|| {
                WorkspaceError::invalid_path(shown(), "the path is not valid Unicode")
            })?;
            if part == ".." || part == "." {
                return Err(WorkspaceError::EscapesRoot(shown()));
            }
            parts.push(part);
        }
        if parts.is_empty() {
            return Err(WorkspaceError::invalid_path(
                shown(),
                "the workspace root itself has no relative path",
            ));
        }
        let path = WorkspacePath::new(parts.join("/"))
            .map_err(|error| WorkspaceError::invalid_path(shown(), error.to_string()))?;
        validate_components(&path)?;
        Ok(path)
    }
}

/// Apply the platform's component rules (Windows: reserved device names, trailing
/// dots and spaces, forbidden characters) to an already lexically valid path.
pub(crate) fn validate_components(path: &WorkspacePath) -> Result<(), WorkspaceError> {
    if !ENFORCE_WINDOWS_RULES {
        return Ok(());
    }
    for component in path.as_str().split('/') {
        if let Some(reason) = windows_component_issue(component) {
            return Err(WorkspaceError::invalid_path(path.as_str(), reason));
        }
    }
    Ok(())
}

/// Parse a caller-supplied workspace-relative path with the lexical core rules and
/// the platform rules. Rejects traversal, absolute paths, drive letters, UNC paths,
/// alternate data streams (`:`), and backslashes.
pub fn parse_workspace_path(path: &str) -> Result<WorkspacePath, WorkspaceError> {
    let parsed = WorkspacePath::new(path)
        .map_err(|error| WorkspaceError::invalid_path(path, error.to_string()))?;
    validate_components(&parsed)?;
    Ok(parsed)
}
