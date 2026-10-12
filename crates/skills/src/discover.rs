//! Finding skills: roots in precedence order, link refusal, size caps.
//!
//! A root is a directory whose immediate subdirectories each hold one skill
//! (`<root>/<skill-dir>/SKILL.md`, or `skill.md` in any letter case). Hidden
//! entries are skipped. Symbolic links and junctions are never followed: a
//! linked skill directory or file is listed with an error, and a linked root (or
//! root ancestor below the workspace or drive root) is reported as an issue.

use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use aidex_core::EntityId;
use aidex_drive::DrivePaths;
use serde::{Deserialize, Serialize};

use crate::{
    SkillDocument, SkillError, SkillManifest, SkillScope,
    fsio::{self, Entry, ReadFailure},
    manifest::{MAX_BODY_BYTES, MAX_SKILL_FILE_BYTES, declared_name},
    name::{MAX_SKILL_NAME_BYTES, is_skill_name},
};

/// The workspace's own skills, scope `project`.
pub const PROJECT_SKILLS_DIR: [&str; 2] = [".aidex", "skills"];
/// Skills other agents use in the workspace, scope `repo`, in precedence order.
pub const REPO_SKILLS_DIRS: [[&str; 2]; 3] = [
    [".agents", "skills"],
    [".claude", "skills"],
    [".codex", "skills"],
];
/// Directory entries read from one root.
pub const MAX_ROOT_ENTRIES: usize = 4096;
/// Skills taken from one root (in name order).
pub const MAX_SKILLS_PER_ROOT: usize = 256;
/// Skills in one catalog.
pub const MAX_SKILLS: usize = 512;
/// Skills taken from all of a workspace's roots together (project and repo,
/// disabled by default), so they can never crowd the user's drive and plugin
/// skills out of the catalog.
pub const MAX_WORKSPACE_SKILLS: usize = MAX_SKILLS / 2;
/// Skill files read and parsed from one root, counting those without a usable
/// name, so a root of unusable entries cannot make discovery parse thousands.
const MAX_PARSED_PER_ROOT: usize = 2 * MAX_SKILLS_PER_ROOT;
/// Entries read from one skill directory while looking for `SKILL.md`.
const MAX_SKILL_DIR_ENTRIES: usize = 1024;

/// The workspace whose project and repo skills are discovered.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillWorkspace {
    /// Registry id; workspace skill enablement is kept per workspace.
    pub id: EntityId,
    /// The workspace root (trusted: resolved through the workspace registry).
    pub root: PathBuf,
}

/// A further root, such as an enabled plugin's `skills/` directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtraSkillRoot {
    /// Must be [`SkillScope::Plugin`].
    pub scope: SkillScope,
    pub root: PathBuf,
    /// The plugin name that contributes the root.
    pub owner: String,
}

/// A root or entry that could not be considered at all.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillIssue {
    pub scope: SkillScope,
    /// Drive- or workspace-relative path.
    pub location: String,
    pub error: String,
}

/// One skill directory as found on disk.
#[derive(Clone, Debug)]
pub(crate) struct Found {
    pub scope: SkillScope,
    pub owner: Option<String>,
    pub name: String,
    pub dir: PathBuf,
    /// The skill file, when one was found and is a regular file.
    pub file: Option<PathBuf>,
    /// The skill directory itself is a link (never followed).
    pub linked: bool,
    pub location: String,
    pub size_bytes: u64,
    pub manifest: Result<SkillManifest, String>,
}

pub(crate) struct Discovery {
    pub found: Vec<Found>,
    pub issues: Vec<SkillIssue>,
}

struct Root {
    scope: SkillScope,
    dir: PathBuf,
    /// Location prefix, e.g. `.claude/skills`.
    label: String,
    owner: Option<String>,
}

/// Every root in precedence order; for each name only the first skill is kept.
pub(crate) fn discover(
    paths: &DrivePaths,
    workspace: Option<&SkillWorkspace>,
    extra_roots: &[ExtraSkillRoot],
) -> Result<Discovery, SkillError> {
    for extra in extra_roots {
        if extra.scope != SkillScope::Plugin {
            return Err(SkillError::Invalid(
                "extra skill roots must have the plugin scope".into(),
            ));
        }
        let owner = extra.owner.trim();
        if owner.is_empty()
            || owner.len() > 128
            || owner
                .chars()
                .any(|ch| ch.is_control() || ch == '/' || ch == '\\')
        {
            return Err(SkillError::Invalid(
                "extra skill roots need a plugin name".into(),
            ));
        }
    }
    let mut issues = Vec::new();
    let mut roots = Vec::new();
    if let Some(workspace) = workspace {
        let mut add = |scope, segments: &[&str]| {
            if let Some(dir) = resolve(&workspace.root, segments, scope, &mut issues) {
                roots.push(Root {
                    scope,
                    dir,
                    label: segments.join("/"),
                    owner: None,
                });
            }
        };
        add(SkillScope::Project, &PROJECT_SKILLS_DIR);
        for segments in &REPO_SKILLS_DIRS {
            add(SkillScope::Repo, segments);
        }
    }
    drive_root(paths, &mut roots, &mut issues);
    for extra in extra_roots {
        plugin_root(paths, extra, &mut roots, &mut issues);
    }

    let mut found: Vec<Found> = Vec::new();
    let mut workspace_kept = 0;
    for root in &roots {
        let workspace = root.scope.is_workspace();
        if workspace && workspace_kept >= MAX_WORKSPACE_SKILLS {
            issues.push(workspace_budget_issue(root));
            continue;
        }
        for candidate in scan_root(root, &mut issues) {
            if let Some(existing) = found
                .iter()
                .find(|existing| existing.name == candidate.name)
            {
                // A higher-precedence root shadows the skill silently; two
                // directories of one root declaring the same name is a mistake.
                if existing.dir.parent() == candidate.dir.parent() {
                    issues.push(SkillIssue {
                        scope: root.scope,
                        error: format!(
                            "`{}` declares the skill `{}` that `{}` already defines; it is ignored",
                            candidate.location, candidate.name, existing.location
                        ),
                        location: candidate.location,
                    });
                }
                continue;
            }
            if workspace && workspace_kept >= MAX_WORKSPACE_SKILLS {
                issues.push(workspace_budget_issue(root));
                break;
            }
            if found.len() >= MAX_SKILLS {
                issues.push(SkillIssue {
                    scope: root.scope,
                    location: root.label.clone(),
                    error: format!("more than {MAX_SKILLS} skills; the rest are ignored"),
                });
                return Ok(Discovery { found, issues });
            }
            if workspace {
                workspace_kept += 1;
            }
            found.push(candidate);
        }
    }
    Ok(Discovery { found, issues })
}

fn workspace_budget_issue(root: &Root) -> SkillIssue {
    SkillIssue {
        scope: root.scope,
        location: root.label.clone(),
        error: format!(
            "the workspace has more than {MAX_WORKSPACE_SKILLS} skills; the rest are ignored"
        ),
    }
}

/// Resolve `segments` below a trusted `base`, recording an issue for links.
fn resolve(
    base: &Path,
    segments: &[&str],
    scope: SkillScope,
    issues: &mut Vec<SkillIssue>,
) -> Option<PathBuf> {
    match fsio::real_dir_beneath(base, segments) {
        Ok(dir) => dir,
        Err((path, error)) => {
            let location = fsio::relative_slash(base, &path).unwrap_or_else(|| segments.join("/"));
            issues.push(SkillIssue {
                scope,
                error: match error {
                    Some(error) => format!("`{location}` cannot be read: {error}"),
                    None => format!(
                        "`{location}` is a symbolic link, junction, or not a directory; skills there are ignored"
                    ),
                },
                location,
            });
            None
        }
    }
}

fn drive_root(paths: &DrivePaths, roots: &mut Vec<Root>, issues: &mut Vec<SkillIssue>) {
    let skills = paths.skills();
    let Some(label) = fsio::relative_slash(paths.root(), &skills) else {
        return;
    };
    let segments: Vec<&str> = label.split('/').filter(|s| !s.is_empty()).collect();
    if let Some(dir) = resolve(paths.root(), &segments, SkillScope::Drive, issues) {
        roots.push(Root {
            scope: SkillScope::Drive,
            dir,
            label,
            owner: None,
        });
    }
}

fn plugin_root(
    paths: &DrivePaths,
    extra: &ExtraSkillRoot,
    roots: &mut Vec<Root>,
    issues: &mut Vec<SkillIssue>,
) {
    let owner = extra.owner.trim().to_owned();
    let dir = match fsio::relative_slash(paths.root(), &extra.root) {
        Some(label) => {
            let segments: Vec<&str> = label.split('/').filter(|s| !s.is_empty()).collect();
            resolve(paths.root(), &segments, SkillScope::Plugin, issues).map(|dir| (dir, label))
        }
        None => {
            let label = format!("{owner}/skills");
            match fsio::inspect(&extra.root) {
                Ok(Entry::Dir) => Some((extra.root.clone(), label)),
                Ok(Entry::Missing) => None,
                Ok(_) | Err(_) => {
                    issues.push(SkillIssue {
                        scope: SkillScope::Plugin,
                        error: format!(
                            "`{label}` is a symbolic link, junction, or not a readable directory; skills there are ignored"
                        ),
                        location: label,
                    });
                    None
                }
            }
        }
    };
    if let Some((dir, label)) = dir {
        roots.push(Root {
            scope: SkillScope::Plugin,
            dir,
            label,
            owner: Some(owner),
        });
    }
}

/// The skills directly beneath one root, in name order.
fn scan_root(root: &Root, issues: &mut Vec<SkillIssue>) -> Vec<Found> {
    let entries = match fs::read_dir(&root.dir) {
        Ok(entries) => entries,
        Err(error) => {
            issues.push(SkillIssue {
                scope: root.scope,
                location: root.label.clone(),
                error: format!("`{}` cannot be read: {error}", root.label),
            });
            return Vec::new();
        }
    };
    let mut names: Vec<OsString> = Vec::new();
    for entry in entries.flatten() {
        if names.len() >= MAX_ROOT_ENTRIES {
            issues.push(SkillIssue {
                scope: root.scope,
                location: root.label.clone(),
                error: format!(
                    "`{}` has more than {MAX_ROOT_ENTRIES} entries; only some were scanned",
                    root.label
                ),
            });
            break;
        }
        names.push(entry.file_name());
    }
    names.sort();
    let mut found = Vec::new();
    let mut parsed = 0;
    for name in names {
        let lossy = name.to_string_lossy().into_owned();
        if lossy.starts_with('.') {
            continue;
        }
        let dir = root.dir.join(&name);
        let location = format!("{}/{lossy}", root.label);
        let candidate = match fsio::inspect(&dir) {
            Ok(Entry::Dir) => match find_skill_file(&dir) {
                None => continue,
                Some(Ok((file_name, file, size))) => {
                    if found.len() >= MAX_SKILLS_PER_ROOT || parsed >= MAX_PARSED_PER_ROOT {
                        issues.push(root_full_issue(root, found.len()));
                        break;
                    }
                    parsed += 1;
                    let manifest = load_manifest(&file, size, name.to_str());
                    Candidate {
                        location: format!("{location}/{file_name}"),
                        file: Some(file),
                        linked: false,
                        size_bytes: size,
                        declared: manifest.0,
                        manifest: manifest.1,
                    }
                }
                Some(Err(error)) => Candidate::failed(location, error),
            },
            Ok(Entry::Link) => {
                let mut candidate = Candidate::failed(
                    location,
                    "symbolic links and junctions are refused; copy the skill instead".into(),
                );
                candidate.linked = true;
                candidate
            }
            Ok(_) => continue,
            Err(error) => Candidate::failed(location, format!("cannot be read: {error}")),
        };
        let Some(skill_name) = candidate
            .declared
            .clone()
            .or_else(|| name.to_str().and_then(fallback_name))
        else {
            issues.push(SkillIssue {
                scope: root.scope,
                location: candidate.location,
                error: match candidate.manifest {
                    Err(error) => error,
                    Ok(_) => "the skill has no usable name".into(),
                },
            });
            continue;
        };
        if found.len() >= MAX_SKILLS_PER_ROOT {
            issues.push(root_full_issue(root, found.len()));
            break;
        }
        found.push(Found {
            scope: root.scope,
            owner: root.owner.clone(),
            name: skill_name,
            dir,
            file: candidate.file,
            linked: candidate.linked,
            location: candidate.location,
            size_bytes: candidate.size_bytes,
            manifest: candidate.manifest,
        });
    }
    found
}

/// The issue for a root whose further skills are ignored: it already gave
/// [`MAX_SKILLS_PER_ROOT`] skills, or [`MAX_PARSED_PER_ROOT`] skill files were read.
fn root_full_issue(root: &Root, kept: usize) -> SkillIssue {
    SkillIssue {
        scope: root.scope,
        location: root.label.clone(),
        error: if kept >= MAX_SKILLS_PER_ROOT {
            format!(
                "`{}` has more than {MAX_SKILLS_PER_ROOT} skills; the rest are ignored",
                root.label
            )
        } else {
            format!(
                "`{}` has more than {MAX_PARSED_PER_ROOT} skill files; the rest are ignored",
                root.label
            )
        },
    }
}

struct Candidate {
    location: String,
    file: Option<PathBuf>,
    linked: bool,
    size_bytes: u64,
    declared: Option<String>,
    manifest: Result<SkillManifest, String>,
}

impl Candidate {
    fn failed(location: String, error: String) -> Self {
        Self {
            location,
            file: None,
            linked: false,
            size_bytes: 0,
            declared: None,
            manifest: Err(error),
        }
    }
}

/// The skill file in `dir`: `SKILL.md`, else `skill.md`, else another letter
/// case of it; `None` when the directory holds none (not a skill).
fn find_skill_file(dir: &Path) -> Option<Result<(String, PathBuf, u64), String>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => return Some(Err(format!("cannot be read: {error}"))),
    };
    let mut matches: Vec<String> = entries
        .flatten()
        .take(MAX_SKILL_DIR_ENTRIES)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.eq_ignore_ascii_case("skill.md"))
        .collect();
    matches.sort_by_key(|name| match name.as_str() {
        "SKILL.md" => 0,
        "skill.md" => 1,
        _ => 2,
    });
    let file_name = matches.into_iter().next()?;
    let file = dir.join(&file_name);
    Some(match fsio::inspect(&file) {
        Ok(Entry::File(size)) => Ok((file_name, file, size)),
        Ok(Entry::Link) => Err(format!(
            "`{file_name}` is a symbolic link or junction; links are refused"
        )),
        Ok(_) => Err(format!("`{file_name}` is not a regular file")),
        Err(error) => Err(format!("`{file_name}` cannot be read: {error}")),
    })
}

/// Read and validate a skill file: `(declared name, manifest or error)`.
pub(crate) fn load_manifest(
    file: &Path,
    size: u64,
    directory: Option<&str>,
) -> (Option<String>, Result<SkillManifest, String>) {
    match read_document(file, size, directory) {
        Ok((manifest, _)) => (Some(manifest.name.clone()), Ok(manifest)),
        Err((declared, error)) => (declared, Err(error)),
    }
}

/// Read, parse, and validate a skill file. Errors carry the name the file
/// declares (when it parsed that far).
pub(crate) fn read_document(
    file: &Path,
    size: u64,
    directory: Option<&str>,
) -> Result<(SkillManifest, SkillDocument), (Option<String>, String)> {
    let too_large = || {
        format!(
            "the skill file is larger than {} KiB",
            MAX_SKILL_FILE_BYTES / 1024
        )
    };
    if size > MAX_SKILL_FILE_BYTES {
        return Err((None, too_large()));
    }
    let text = match fsio::read_text(file, MAX_SKILL_FILE_BYTES) {
        Ok(text) => text,
        Err(ReadFailure::TooLarge) => return Err((None, too_large())),
        Err(ReadFailure::NotUtf8) => return Err((None, "the skill file is not UTF-8 text".into())),
        Err(ReadFailure::Io(error)) => {
            return Err((None, format!("the skill file cannot be read: {error}")));
        }
    };
    let document = SkillDocument::parse(&text).map_err(|error| (None, error.to_string()))?;
    let declared = declared_name(document.front_matter.as_ref(), directory);
    if document.body.len() > MAX_BODY_BYTES {
        return Err((
            declared,
            format!(
                "the instructions are larger than {} KiB",
                MAX_BODY_BYTES / 1024
            ),
        ));
    }
    match SkillManifest::from_front_matter(document.front_matter.as_ref(), directory) {
        Ok(manifest) => Ok((manifest, document)),
        Err(error) => Err((declared, error)),
    }
}

/// A listing name for a directory that is not a valid slug: lowercase, other
/// characters replaced by `-`.
fn fallback_name(directory: &str) -> Option<String> {
    let mut name = String::with_capacity(directory.len());
    for ch in directory.chars() {
        let ch = ch.to_ascii_lowercase();
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            name.push(ch);
        } else if !name.ends_with('-') {
            name.push('-');
        }
    }
    let mut name = name.trim_matches('-').to_owned();
    name.truncate(MAX_SKILL_NAME_BYTES);
    let name = name.trim_end_matches('-').to_owned();
    is_skill_name(&name).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_names_are_slugs() {
        assert_eq!(fallback_name("Code_Review").as_deref(), Some("code-review"));
        assert_eq!(fallback_name("--x--").as_deref(), Some("x"));
        assert_eq!(fallback_name("éü"), None);
        assert_eq!(fallback_name(&"a".repeat(80)).unwrap().len(), 64);
    }
}
