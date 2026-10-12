//! A deterministic, confined, link-refusing workspace walk honouring `.gitignore`.
//!
//! Ignore rules: `.gitignore` files from the workspace root down to (and beneath)
//! the walk start, plus `.git/info/exclude` when the root is a repository. Files
//! above the workspace root (and the user's global excludes file) are never read,
//! so results depend only on workspace content. `.git` is never entered. Symbolic
//! links and junctions are reported as entries but never followed.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use aidex_core::WorkspacePath;
use aidex_workspace::{Access, parse_workspace_path};
use ignore::{
    Match,
    gitignore::{Gitignore, GitignoreBuilder},
};

use crate::{EntryKind, ToolContext, ToolError};

/// Options for one walk.
#[derive(Clone, Copy, Debug)]
pub(crate) struct WalkOptions {
    pub include_ignored: bool,
    /// Depth limit below the start (1 = direct children only).
    pub max_depth: Option<usize>,
    /// Stop after visiting this many entries.
    pub max_entries: usize,
}

/// One visited entry.
#[derive(Clone, Debug)]
pub(crate) struct WalkEntry {
    pub path: WorkspacePath,
    pub absolute: PathBuf,
    pub kind: EntryKind,
    pub len: u64,
}

/// Whether to keep walking.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Flow {
    Continue,
    Stop,
}

/// What the walk could not report.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct WalkSummary {
    pub visited: usize,
    /// The entry limit stopped the walk.
    pub truncated: bool,
    /// Names that are not Unicode or not portable workspace paths.
    pub unsupported_names: usize,
    pub unreadable_dirs: usize,
}

struct Matcher {
    /// Workspace-relative components of the directory holding the ignore file.
    dir: Vec<String>,
    gitignore: Arc<Gitignore>,
}

fn load(dir_abs: &Path, file: &Path, dir: &[String]) -> Option<Matcher> {
    let metadata = fs::symlink_metadata(file).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let mut builder = GitignoreBuilder::new(dir_abs);
    builder.case_insensitive(cfg!(windows)).ok()?;
    // Invalid globs are skipped individually, as git does.
    let _ = builder.add(file);
    let gitignore = builder.build().ok()?;
    Some(Matcher {
        dir: dir.to_vec(),
        gitignore: Arc::new(gitignore),
    })
}

fn is_ignored(matchers: &[Arc<Matcher>], components: &[String], is_dir: bool) -> bool {
    for matcher in matchers.iter().rev() {
        let Some(relative) = components.get(matcher.dir.len()..) else {
            continue;
        };
        let relative = relative.join("/");
        match matcher.gitignore.matched(Path::new(&relative), is_dir) {
            Match::None => continue,
            Match::Ignore(_) => return true,
            Match::Whitelist(_) => return false,
        }
    }
    false
}

/// Whether a component names git metadata (never listed, searched, or modified).
pub(crate) fn is_git_dir(name: &str) -> bool {
    name.eq_ignore_ascii_case(".git")
}

fn entry_kind(file_type: fs::FileType) -> EntryKind {
    if file_type.is_symlink() {
        EntryKind::Symlink
    } else if file_type.is_dir() {
        EntryKind::Directory
    } else if file_type.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    }
}

/// Walk beneath `start` (the root when `None`), visiting entries in name order.
pub(crate) fn walk(
    ctx: &ToolContext,
    start: Option<&WorkspacePath>,
    options: WalkOptions,
    mut visit: impl FnMut(&WalkEntry) -> Result<Flow, ToolError>,
) -> Result<WalkSummary, ToolError> {
    let root = ctx.root.root().to_path_buf();
    let mut matchers: Vec<Arc<Matcher>> = Vec::new();
    if !options.include_ignored {
        let git = root.join(".git");
        if fs::symlink_metadata(&git).is_ok_and(|metadata| metadata.is_dir()) {
            matchers.extend(load(&root, &git.join("info").join("exclude"), &[]).map(Arc::new));
        }
        matchers.extend(load(&root, &root.join(".gitignore"), &[]).map(Arc::new));
    }
    let (start_abs, start_components) = match start {
        None => (root.clone(), Vec::new()),
        Some(path) => {
            let components: Vec<String> = path.as_str().split('/').map(str::to_owned).collect();
            if !options.include_ignored {
                for depth in 1..=components.len() {
                    let prefix =
                        WorkspacePath::new(components[..depth].join("/")).map_err(|_| {
                            ToolError::NotFound {
                                path: path.as_str().to_owned(),
                            }
                        })?;
                    let dir = ctx
                        .root
                        .resolve(&prefix, Access::Read)
                        .map_err(|error| ToolError::from_workspace("path", error))?;
                    matchers.extend(
                        load(&dir, &dir.join(".gitignore"), &components[..depth]).map(Arc::new),
                    );
                }
            }
            let resolved = ctx
                .root
                .resolve(path, Access::Read)
                .map_err(|error| ToolError::from_workspace("path", error))?;
            if !resolved.is_dir() {
                return Err(ToolError::WrongKind {
                    path: path.as_str().to_owned(),
                    expected: "directory",
                    found: "file",
                });
            }
            (resolved, components)
        }
    };
    let mut summary = WalkSummary::default();
    let mut stack = vec![(start_abs, start_components, 1usize, matchers)];
    while let Some((dir, components, depth, matchers)) = stack.pop() {
        crate::context::check_cancel(&ctx.cancel)?;
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if depth == 1 => {
                return Err(ToolError::io(
                    format!("cannot list `{}`", components.join("/")),
                    error,
                ));
            }
            Err(_) => {
                summary.unreadable_dirs += 1;
                continue;
            }
        };
        let mut children: Vec<fs::DirEntry> = entries.filter_map(Result::ok).collect();
        children.sort_by_key(fs::DirEntry::file_name);
        let mut subdirectories = Vec::new();
        for child in children {
            let Some(name) = child.file_name().to_str().map(str::to_owned) else {
                summary.unsupported_names += 1;
                continue;
            };
            if is_git_dir(&name) {
                continue;
            }
            let Ok(file_type) = child.file_type() else {
                summary.unsupported_names += 1;
                continue;
            };
            let kind = entry_kind(file_type);
            let mut child_components = components.clone();
            child_components.push(name);
            let Ok(path) = parse_workspace_path(&child_components.join("/")) else {
                summary.unsupported_names += 1;
                continue;
            };
            if !options.include_ignored
                && is_ignored(&matchers, &child_components, kind == EntryKind::Directory)
            {
                continue;
            }
            if summary.visited >= options.max_entries {
                summary.truncated = true;
                return Ok(summary);
            }
            summary.visited += 1;
            let len = if kind == EntryKind::File {
                child.metadata().map(|metadata| metadata.len()).unwrap_or(0)
            } else {
                0
            };
            let absolute = child.path();
            let entry = WalkEntry {
                path,
                absolute: absolute.clone(),
                kind,
                len,
            };
            if visit(&entry)? == Flow::Stop {
                return Ok(summary);
            }
            let descend = options.max_depth.is_none_or(|max| depth < max);
            if kind == EntryKind::Directory && descend {
                let mut nested = matchers.clone();
                if !options.include_ignored {
                    nested.extend(
                        load(&absolute, &absolute.join(".gitignore"), &child_components)
                            .map(Arc::new),
                    );
                }
                subdirectories.push((absolute, child_components, depth + 1, nested));
            }
        }
        stack.extend(subdirectories.into_iter().rev());
    }
    Ok(summary)
}
