//! `fs.delete`: delete a file, link, or (explicitly recursive) directory.

use std::{
    fs,
    path::{Path, PathBuf},
};

use aidex_core::WorkspacePath;
use aidex_events::{FileChange, RiskLevel};
use aidex_policy::Action;
use aidex_workspace::Access;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    hash_file, implied_actions, parse_path, plan_path, read_capped, reject_git_internal, resolve,
    stat,
};
use crate::{
    DiffKind, DiffPreview, EntryKind, ExecutionPermit, FileDiff, InputError, PathAccess,
    PathChange, PlanFacts, PlanParts, RiskProfile, Tool, ToolContext, ToolDescriptor, ToolError,
    ToolOutput, ToolPlan,
    builtin::descriptor,
    diff::file_diff,
    fs::hex,
    schema::{normalized, parse_input},
};

/// Files listed individually in a recursive-delete preview.
const MAX_PREVIEW_FILES: usize = 200;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DeleteInput {
    /// Workspace-relative path of the file, link, or directory to delete.
    path: String,
    /// Delete a non-empty directory and everything beneath it (destructive).
    #[serde(default)]
    recursive: bool,
    /// Only delete a regular file whose SHA-256 matches this exact expected content.
    #[serde(default)]
    expected_sha256: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DeleteOutput {
    path: String,
    kind: EntryKind,
    files_deleted: u64,
    directories_deleted: u64,
}

/// One entry of a directory tree, for previews and change detection.
struct TreeEntry {
    relative: String,
    kind: EntryKind,
    size: u64,
    modified_ns: u128,
}

/// Every entry beneath `dir` (links are entries, never followed), sorted.
fn enumerate(
    ctx: &ToolContext,
    dir: &Path,
    prefix: &str,
    out: &mut Vec<TreeEntry>,
) -> Result<(), ToolError> {
    crate::context::check_cancel(&ctx.cancel)?;
    let mut children: Vec<fs::DirEntry> = fs::read_dir(dir)
        .map_err(|error| ToolError::io(format!("cannot list `{prefix}`"), error))?
        .collect::<Result<_, _>>()
        .map_err(|error| ToolError::io(format!("cannot list `{prefix}`"), error))?;
    children.sort_by_key(fs::DirEntry::file_name);
    for child in children {
        let name = child.file_name().to_string_lossy().into_owned();
        let relative = format!("{prefix}/{name}");
        let path = child.path();
        let entry = stat(&path)?;
        let Some(kind) = entry.kind else { continue };
        out.push(TreeEntry {
            relative: relative.clone(),
            kind,
            size: entry.size,
            modified_ns: entry.modified_ns,
        });
        if out.len() > ctx.limits.max_walk_entries {
            return Err(ToolError::TooLarge {
                what: "the directory tree".into(),
                size: out.len() as u64,
                limit: ctx.limits.max_walk_entries as u64,
            });
        }
        if kind == EntryKind::Directory {
            enumerate(ctx, &path, &relative, out)?;
        }
    }
    Ok(())
}

fn tree_digest(entries: &[TreeEntry]) -> String {
    let mut hasher = Sha256::new();
    for entry in entries {
        hasher.update(entry.relative.as_bytes());
        hasher.update([0]);
        hasher.update(entry.kind.as_str().as_bytes());
        hasher.update(entry.size.to_le_bytes());
        hasher.update(entry.modified_ns.to_le_bytes());
    }
    hex(&hasher.finalize())
}

/// The target entry itself (a leaf link is not followed): its parent is resolved
/// inside the workspace and the leaf is joined without resolution.
fn locate(ctx: &ToolContext, path: &WorkspacePath) -> Result<PathBuf, ToolError> {
    let parent = match path.as_str().rsplit_once('/') {
        Some((parent, _)) => {
            let parent = parse_path("path", parent)?;
            resolve(ctx, "path", &parent, Access::Read)?
        }
        None => ctx.root.root().to_path_buf(),
    };
    if !parent.is_dir() {
        return Err(ToolError::NotFound {
            path: path.as_str().to_owned(),
        });
    }
    let name = path.as_str().rsplit('/').next().unwrap_or_default();
    Ok(parent.join(name))
}

pub(crate) struct FsDelete {
    descriptor: ToolDescriptor,
}

impl FsDelete {
    pub(crate) fn new() -> Result<Self, ToolError> {
        Ok(Self {
            descriptor: descriptor::<DeleteInput, DeleteOutput>(
                "fs.delete",
                1,
                "Delete a workspace file, symbolic link (the link only), or empty directory; \
                 deleting a non-empty directory requires `recursive` and is destructive.",
                RiskProfile {
                    action: Action::FsDelete,
                    base_risk: RiskLevel::Medium,
                    reads_workspace: true,
                    writes_workspace: true,
                    spawns_process: false,
                    network: false,
                    destructive: true,
                    remote_write: false,
                },
                &["workspace:write"],
            )?,
        })
    }
}

impl Tool for FsDelete {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    fn plan(&self, ctx: &ToolContext, input: Value) -> Result<ToolPlan, ToolError> {
        let input: DeleteInput = parse_input(input)?;
        let path = parse_path("path", &input.path)?;
        reject_git_internal(&path)?;
        let absolute = locate(ctx, &path)?;
        let target = stat(&absolute)?;
        let Some(kind) = target.kind else {
            return Err(ToolError::NotFound {
                path: path.as_str().to_owned(),
            });
        };
        if input.expected_sha256.is_some() && kind != EntryKind::File {
            return Err(ToolError::invalid(
                "expected_sha256",
                InputError::InvalidValue,
                "an expected content digest requires a regular file",
            ));
        }
        let mut sha256 = None;
        let mut preview = DiffPreview::default();
        let mut state = json!({ "kind": kind, "modified_ns": target.modified_ns.to_string() });
        let mut destructive = false;
        let mut risk = RiskLevel::Medium;
        match kind {
            EntryKind::File => {
                sha256 = hash_file(&absolute)
                    .map_err(|error| ToolError::io("cannot hash the file", error))?;
                if input.expected_sha256.as_ref().is_some_and(|expected| {
                    sha256
                        .as_ref()
                        .is_none_or(|actual| !expected.eq_ignore_ascii_case(actual))
                }) {
                    return Err(ToolError::conflict(
                        path.as_str(),
                        "its content does not match the expected digest",
                    ));
                }
                let mut budget = ctx.limits.max_diff_bytes;
                let file = if target.size <= ctx.limits.max_write_bytes {
                    let bytes = read_capped(&absolute, ctx.limits.max_write_bytes, "the file")?;
                    file_diff(&path, Some(&bytes), None, &mut budget)
                } else {
                    FileDiff {
                        path: path.clone(),
                        kind: DiffKind::Deleted,
                        additions: 0,
                        deletions: 0,
                        unified: None,
                        binary: false,
                        truncated: true,
                        before_sha256: sha256.clone(),
                        after_sha256: None,
                    }
                };
                preview = DiffPreview::single(file);
            }
            EntryKind::Directory => {
                let mut entries = Vec::new();
                enumerate(ctx, &absolute, path.as_str(), &mut entries)?;
                if !entries.is_empty() && !input.recursive {
                    return Err(ToolError::invalid(
                        "recursive",
                        InputError::InvalidValue,
                        format!(
                            "`{}` is not empty; set `recursive` to delete it and its {} entries",
                            path.as_str(),
                            entries.len()
                        ),
                    ));
                }
                destructive = !entries.is_empty();
                if destructive {
                    risk = RiskLevel::High;
                }
                let files: Vec<&TreeEntry> = entries
                    .iter()
                    .filter(|entry| entry.kind != EntryKind::Directory)
                    .collect();
                for entry in files.iter().take(MAX_PREVIEW_FILES) {
                    if let Ok(file_path) = WorkspacePath::new(entry.relative.clone()) {
                        preview.files.push(FileDiff {
                            path: file_path,
                            kind: DiffKind::Deleted,
                            additions: 0,
                            deletions: 0,
                            unified: None,
                            binary: false,
                            truncated: false,
                            before_sha256: None,
                            after_sha256: None,
                        });
                    }
                }
                preview.files_omitted = files.len().saturating_sub(MAX_PREVIEW_FILES) as u64;
                state["tree_digest"] = Value::String(tree_digest(&entries));
                state["entries"] = Value::from(entries.len() as u64);
            }
            EntryKind::Symlink | EntryKind::Other => {}
        }
        let facts = PlanFacts {
            paths: vec![plan_path(
                Some(&path),
                &absolute,
                PathAccess::Delete,
                &target,
                sha256,
            )],
            destructive,
            implied_actions: implied_actions(&path),
            ..PlanFacts::default()
        };
        let summary = match kind {
            EntryKind::Directory if destructive => format!(
                "delete directory {} recursively ({} files)",
                path.as_str(),
                preview.files.len() as u64 + preview.files_omitted
            ),
            _ => format!("delete {} {}", kind.as_str(), path.as_str()),
        };
        ToolPlan::new(
            ctx,
            &self.descriptor,
            PlanParts::new(normalized(&input)?, summary)
                .with_facts(facts)
                .with_risk(risk)
                .with_preview(preview)
                .with_state(state),
        )
    }

    fn execute(
        &self,
        ctx: &ToolContext,
        plan: &ToolPlan,
        _permit: &ExecutionPermit,
    ) -> Result<ToolOutput, ToolError> {
        let input: DeleteInput = parse_input(plan.input().clone())?;
        let path = parse_path("path", &input.path)?;
        let planned = plan
            .facts()
            .paths
            .first()
            .ok_or_else(|| ToolError::PlanMismatch("the plan names no path".into()))?;
        let absolute = locate(ctx, &path)?;
        let current = stat(&absolute)?;
        let shown = path.as_str();
        let Some(kind) = current.kind else {
            return Err(ToolError::conflict(shown, "it was deleted"));
        };
        if Some(kind) != planned.kind {
            return Err(ToolError::conflict(shown, "its type changed"));
        }
        let files: u64;
        let mut directories = 0u64;
        match kind {
            EntryKind::File => {
                let hash = hash_file(&absolute)
                    .map_err(|error| ToolError::io("cannot hash the file", error))?;
                if hash != planned.sha256 || Some(current.size) != planned.size {
                    return Err(ToolError::conflict(shown, "its content changed"));
                }
                fs::remove_file(&absolute)
                    .map_err(|error| ToolError::io("cannot delete the file", error))?;
                files = 1;
            }
            EntryKind::Directory => {
                let mut entries = Vec::new();
                enumerate(ctx, &absolute, shown, &mut entries)?;
                if plan.state().get("tree_digest").and_then(Value::as_str)
                    != Some(tree_digest(&entries).as_str())
                {
                    return Err(ToolError::conflict(shown, "the directory contents changed"));
                }
                files = entries
                    .iter()
                    .filter(|entry| entry.kind != EntryKind::Directory)
                    .count() as u64;
                directories = 1 + entries
                    .iter()
                    .filter(|entry| entry.kind == EntryKind::Directory)
                    .count() as u64;
                // `remove_dir_all` removes links themselves and never follows them.
                if entries.is_empty() {
                    fs::remove_dir(&absolute)
                } else {
                    fs::remove_dir_all(&absolute)
                }
                .map_err(|error| ToolError::io("cannot delete the directory", error))?;
            }
            EntryKind::Symlink | EntryKind::Other => {
                // A directory link (or junction) is removed as a directory entry on
                // Windows; neither call follows the link.
                fs::remove_file(&absolute)
                    .or_else(|_| fs::remove_dir(&absolute))
                    .map_err(|error| ToolError::io("cannot delete the link", error))?;
                files = 1;
            }
        }
        let output = DeleteOutput {
            path: shown.to_owned(),
            kind,
            files_deleted: files,
            directories_deleted: directories,
        };
        let summary = format!("deleted {} {shown}", kind.as_str());
        Ok(ToolOutput::typed(summary, &output)?
            .with_diff(plan.preview().cloned().unwrap_or_default())
            .with_changes(vec![PathChange {
                path,
                change: FileChange::Deleted,
            }]))
    }
}
