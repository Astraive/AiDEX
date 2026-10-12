//! A deterministic, link-refusing directory walk that honours `.gitignore` files.
//!
//! Ignore rules follow Git: `.gitignore` files from the walk root downwards, plus
//! (for a root inside a repository) the `.gitignore` files between the repository
//! root and the walk root and the repository's `.git/info/exclude`. The user's
//! global excludes file is host-specific and deliberately not consulted. Rules are
//! honoured even outside a Git repository. `.git` and `.aidex` directories are
//! never entered. Symbolic links and junctions are reported, never followed.

use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use aidex_core::WorkspacePath;
use ignore::{
    Match,
    gitignore::{Gitignore, GitignoreBuilder},
};
use serde::{Deserialize, Serialize};

use crate::{WorkspaceError, confine::validate_components, discover::enclosing_repository, fsutil};

/// Directory names that hold version-control or AiDEX metadata; never copied,
/// reviewed, or applied.
const RESERVED_DIRS: [&str; 2] = [".git", ".aidex"];

/// Why an entry was left out of a copy or review.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// A symbolic link or junction; never followed or reproduced.
    Link,
    /// The name is not valid Unicode or not a portable workspace path.
    UnsupportedName,
    /// Not a regular file or directory (device, socket, FIFO), or a directory where
    /// a file was expected.
    NotAFile,
    /// The directory could not be read.
    Unreadable,
}

/// An entry that was not copied or is not reviewable, with the reason.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkippedEntry {
    /// Root-relative path (`/`-separated; lossy when the name is not Unicode).
    pub path: String,
    pub reason: SkipReason,
}

/// One item produced by [`walk`].
pub(crate) enum Item {
    Dir {
        absolute: PathBuf,
    },
    File {
        path: WorkspacePath,
        absolute: PathBuf,
        len: u64,
    },
    Skipped(SkippedEntry),
}

/// Whether a path component names a reserved metadata directory.
pub(crate) fn is_reserved_name(name: &str) -> bool {
    RESERVED_DIRS
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(name))
}

/// Whether any component of `path` is a reserved metadata directory.
pub(crate) fn is_reserved_path(path: &WorkspacePath) -> bool {
    path.as_str().split('/').any(is_reserved_name)
}

fn matcher(root: &Path, file: &Path) -> Option<Arc<Gitignore>> {
    let metadata = fs::symlink_metadata(file).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let mut builder = GitignoreBuilder::new(root);
    builder.case_insensitive(cfg!(windows)).ok()?;
    // Invalid globs are skipped individually, as Git does.
    let _ = builder.add(file);
    builder.build().ok().map(Arc::new)
}

/// Resolve Git's metadata directory, including separate git dirs and the shared
/// directory used by linked worktrees. Relative paths in each metadata file are
/// relative to that file's containing directory, not the selected source root.
fn git_common_dir(repository: &Path) -> Option<PathBuf> {
    let git = repository.join(".git");
    let metadata = fs::symlink_metadata(&git).ok()?;
    let git = if metadata.is_dir() {
        git
    } else if metadata.is_file() {
        let contents = fs::read_to_string(&git).ok()?;
        let target = contents.strip_prefix("gitdir:")?.trim();
        if target.is_empty() || target.contains(['\r', '\n']) {
            return None;
        }
        repository.join(target)
    } else {
        return None;
    };
    if !fs::symlink_metadata(&git).is_ok_and(|metadata| metadata.is_dir()) {
        return None;
    }
    let commondir = git.join("commondir");
    match fs::symlink_metadata(&commondir) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(git),
        Ok(metadata) if metadata.is_file() => {
            let contents = fs::read_to_string(commondir).ok()?;
            let target = contents.trim();
            if target.is_empty() || target.contains(['\r', '\n']) {
                return None;
            }
            let common = git.join(target);
            fs::symlink_metadata(&common)
                .is_ok_and(|metadata| metadata.is_dir())
                .then_some(common)
        }
        _ => None,
    }
}

/// Ignore matchers that apply to `root` from above it: the enclosing repository's
/// `info/exclude` and every `.gitignore` between the repository root and `root`.
pub(crate) fn inherited_matchers(root: &Path) -> Vec<Arc<Gitignore>> {
    let Some(repository) = enclosing_repository(root, true) else {
        return Vec::new();
    };
    let mut matchers = Vec::new();
    if let Some(git) = git_common_dir(&repository) {
        matchers.extend(matcher(&repository, &git.join("info").join("exclude")));
    }
    let mut between: Vec<&Path> = root
        .ancestors()
        .skip(1)
        .take_while(|dir| dir.starts_with(&repository))
        .collect();
    between.reverse();
    for dir in between {
        matchers.extend(matcher(dir, &dir.join(".gitignore")));
    }
    matchers
}

fn is_ignored(matchers: &[Arc<Gitignore>], path: &Path, is_dir: bool) -> bool {
    for matcher in matchers.iter().rev() {
        match matcher.matched(path, is_dir) {
            Match::None => continue,
            Match::Ignore(_) => return true,
            Match::Whitelist(_) => return false,
        }
    }
    false
}

/// Walk `root` depth-first in name order, calling `visit` for every directory,
/// regular file, and skipped entry that is not ignored.
pub(crate) fn walk(
    root: &Path,
    inherited: Vec<Arc<Gitignore>>,
    mut visit: impl FnMut(Item) -> Result<(), WorkspaceError>,
) -> Result<(), WorkspaceError> {
    let mut root_matchers = inherited;
    root_matchers.extend(matcher(root, &root.join(".gitignore")));
    let mut stack: Vec<(PathBuf, Vec<String>, Vec<Arc<Gitignore>>)> =
        vec![(root.to_path_buf(), Vec::new(), root_matchers)];
    while let Some((dir, relative, matchers)) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if relative.is_empty() => {
                return Err(WorkspaceError::io(
                    format!("cannot read `{}`", dir.display()),
                    error,
                ));
            }
            Err(_) => {
                visit(Item::Skipped(SkippedEntry {
                    path: relative.join("/"),
                    reason: SkipReason::Unreadable,
                }))?;
                continue;
            }
        };
        let mut children = Vec::new();
        for entry in entries {
            match entry {
                Ok(entry) => children.push(entry),
                Err(_) => visit(Item::Skipped(SkippedEntry {
                    path: relative.join("/"),
                    reason: SkipReason::Unreadable,
                }))?,
            }
        }
        children.sort_by_key(fs::DirEntry::file_name);
        let mut subdirectories = Vec::new();
        for entry in children {
            let file_name = entry.file_name();
            let lossy = |relative: &[String], name: &OsStr| {
                let mut parts = relative.to_vec();
                parts.push(name.to_string_lossy().into_owned());
                parts.join("/")
            };
            let Some(name) = file_name.to_str().map(str::to_owned) else {
                visit(Item::Skipped(SkippedEntry {
                    path: lossy(&relative, &file_name),
                    reason: SkipReason::UnsupportedName,
                }))?;
                continue;
            };
            if is_reserved_name(&name) {
                continue;
            }
            let absolute = entry.path();
            let Ok(file_type) = entry.file_type() else {
                visit(Item::Skipped(SkippedEntry {
                    path: lossy(&relative, &file_name),
                    reason: SkipReason::Unreadable,
                }))?;
                continue;
            };
            let is_dir = file_type.is_dir() && !file_type.is_symlink();
            if is_ignored(&matchers, &absolute, is_dir) {
                continue;
            }
            let mut child = relative.clone();
            child.push(name);
            let joined = child.join("/");
            let Some(path) = WorkspacePath::new(joined.clone())
                .ok()
                .filter(|path| validate_components(path).is_ok())
            else {
                visit(Item::Skipped(SkippedEntry {
                    path: joined,
                    reason: SkipReason::UnsupportedName,
                }))?;
                continue;
            };
            if file_type.is_symlink() || fsutil::is_link(&absolute) {
                visit(Item::Skipped(SkippedEntry {
                    path: joined,
                    reason: SkipReason::Link,
                }))?;
            } else if is_dir {
                visit(Item::Dir {
                    absolute: absolute.clone(),
                })?;
                let mut nested = matchers.clone();
                nested.extend(matcher(&absolute, &absolute.join(".gitignore")));
                subdirectories.push((absolute, child, nested));
            } else if file_type.is_file() {
                let len = entry.metadata().map(|metadata| metadata.len()).unwrap_or(0);
                visit(Item::File {
                    path,
                    absolute,
                    len,
                })?;
            } else {
                visit(Item::Skipped(SkippedEntry {
                    path: joined,
                    reason: SkipReason::NotAFile,
                }))?;
            }
        }
        // Pop in name order.
        stack.extend(subdirectories.into_iter().rev());
    }
    Ok(())
}
