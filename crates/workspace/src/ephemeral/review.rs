//! Computing the reviewable diff of an ephemeral workspace against its baseline,
//! with conflict detection against the current source.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
};

use aidex_core::{EntityId, WorkspacePath};
use sha2::{Digest, Sha256};
use similar::TextDiff;

use super::{
    baseline::{Baseline, FileState},
    types::{ChangeKind, ConflictReason, EphemeralDiff, EphemeralLimits, FileChange},
};
use crate::{
    Access, ConfinedRoot, SkipReason, SkippedEntry, WorkspaceError, fsutil,
    platform::component_keys,
    walk::{self, Item},
};

/// What a confined root holds at a workspace path, without following links.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Entry {
    Missing,
    File(PathBuf),
    /// The path or one of its components is a link (inside or outside the root).
    Link,
    /// A directory, device, or other non-file.
    Other,
}

/// Inspect `path` under `root`, refusing to look through any link.
pub(crate) fn entry(root: &ConfinedRoot, path: &WorkspacePath) -> Result<Entry, WorkspaceError> {
    let resolved = match root.resolve(path, Access::Read) {
        Ok(resolved) => resolved,
        Err(WorkspaceError::PathNotFound(_)) => return Ok(Entry::Missing),
        Err(WorkspaceError::EscapesRoot(_) | WorkspaceError::SymlinkLeaf(_)) => {
            return Ok(Entry::Link);
        }
        Err(WorkspaceError::InvalidPath { .. }) => return Ok(Entry::Other),
        Err(error) => return Err(error),
    };
    let mut lexical = root.root().to_path_buf();
    lexical.extend(path.as_str().split('/'));
    if component_keys(&resolved) != component_keys(&lexical) || fsutil::is_link(&lexical) {
        return Ok(Entry::Link);
    }
    let metadata = fs::symlink_metadata(&resolved).map_err(|error| {
        WorkspaceError::io(format!("cannot inspect `{}`", path.as_str()), error)
    })?;
    Ok(if metadata.is_file() {
        Entry::File(resolved)
    } else {
        Entry::Other
    })
}

/// The source's state at a path, for conflict decisions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SourceState {
    Missing,
    File { path: PathBuf, state: FileState },
    Unsafe,
    Other,
}

impl SourceState {
    /// Whether the source matches `expected` (`None` = absent).
    pub(crate) fn matches(&self, expected: Option<&FileState>) -> bool {
        match (self, expected) {
            (Self::Missing, None) => true,
            (Self::File { state, .. }, Some(expected)) => state == expected,
            _ => false,
        }
    }
}

pub(crate) fn source_state(
    source: &ConfinedRoot,
    path: &WorkspacePath,
) -> Result<SourceState, WorkspaceError> {
    source_state_with(source, path, |resolved| {
        let (size, sha256) = fsutil::hash_file(resolved).map_err(|error| {
            WorkspaceError::io(format!("cannot read source `{}`", path.as_str()), error)
        })?;
        Ok(FileState { size, sha256 })
    })
}

fn source_state_with(
    source: &ConfinedRoot,
    path: &WorkspacePath,
    read: impl FnOnce(&Path) -> Result<FileState, WorkspaceError>,
) -> Result<SourceState, WorkspaceError> {
    Ok(match entry(source, path)? {
        Entry::Missing => SourceState::Missing,
        Entry::Link => SourceState::Unsafe,
        Entry::Other => SourceState::Other,
        Entry::File(resolved) => {
            let state = read(&resolved)?;
            SourceState::File {
                path: resolved,
                state,
            }
        }
    })
}

/// Conflict rule: the source may be applied over only when it still matches the
/// baseline, or already matches the result.
pub(crate) fn conflict(
    base: Option<&FileState>,
    new: Option<&FileState>,
    source: &SourceState,
) -> Option<ConflictReason> {
    if *source == SourceState::Unsafe {
        return Some(ConflictReason::UnsafePath);
    }
    if source.matches(new) || source.matches(base) {
        return None;
    }
    Some(match (source, base) {
        (SourceState::Missing, _) => ConflictReason::SourceRemoved,
        (_, None) => ConflictReason::SourceAdded,
        (SourceState::File { .. }, Some(_)) => ConflictReason::SourceChanged,
        _ => ConflictReason::SourceRemoved,
    })
}

/// A detected change before conflict and diff computation.
struct Pending {
    path: WorkspacePath,
    kind: ChangeKind,
    base: Option<FileState>,
    new: Option<FileState>,
}

pub(crate) struct ReviewInput<'a> {
    pub workspace_id: &'a EntityId,
    pub source_id: &'a EntityId,
    pub source: &'a ConfinedRoot,
    pub tree: &'a ConfinedRoot,
    pub baseline: &'a Baseline,
    pub limits: &'a EphemeralLimits,
}

#[derive(Default)]
struct TreeCount {
    files: u64,
    bytes: u64,
}

impl TreeCount {
    fn account_file(&mut self, len: u64, limits: &EphemeralLimits) -> Result<(), WorkspaceError> {
        self.files = self.files.saturating_add(1);
        self.bytes = self.bytes.saturating_add(len);
        if self.files > limits.max_files {
            return Err(WorkspaceError::TooLarge(format!(
                "the ephemeral tree has more than {} files",
                limits.max_files
            )));
        }
        if self.bytes > limits.max_bytes {
            return Err(WorkspaceError::TooLarge(format!(
                "the ephemeral tree exceeds {} bytes",
                limits.max_bytes
            )));
        }
        Ok(())
    }
}

#[derive(Default)]
struct ScannedTree {
    walked: BTreeMap<String, WorkspacePath>,
    unsupported: Vec<SkippedEntry>,
    count: TreeCount,
}

fn scan(input: &ReviewInput<'_>) -> Result<ScannedTree, WorkspaceError> {
    let mut scanned = ScannedTree::default();
    walk::walk(input.tree.root(), Vec::new(), |item| {
        match item {
            Item::File { path, len, .. } => {
                scanned.count.account_file(len, input.limits)?;
                scanned.walked.insert(path.as_str().to_owned(), path);
            }
            Item::Skipped(skipped) => scanned.unsupported.push(skipped),
            Item::Dir { .. } => {}
        }
        Ok(())
    })?;
    Ok(scanned)
}

pub(crate) fn review(input: &ReviewInput<'_>) -> Result<EphemeralDiff, WorkspaceError> {
    review_scanned(input, scan(input)?)
}

fn review_scanned(
    input: &ReviewInput<'_>,
    scanned: ScannedTree,
) -> Result<EphemeralDiff, WorkspaceError> {
    let ScannedTree {
        mut walked,
        mut unsupported,
        mut count,
    } = scanned;
    // Metadata is only a preflight: files may grow after the walk. Charge the
    // bytes actually hashed once per file against a separate aggregate budget.
    let mut tree_bytes = input.limits.max_bytes;
    let mut flagged: BTreeSet<String> =
        unsupported.iter().map(|entry| entry.path.clone()).collect();
    let mut flag =
        |path: &WorkspacePath, reason: SkipReason, unsupported: &mut Vec<SkippedEntry>| {
            if flagged.insert(path.as_str().to_owned()) {
                unsupported.push(SkippedEntry {
                    path: path.as_str().to_owned(),
                    reason,
                });
            }
        };

    let mut pending: Vec<Pending> = Vec::new();
    for (key, base) in &input.baseline.files {
        let path = WorkspacePath::new(key.as_str())
            .map_err(|_| WorkspaceError::Corrupt(format!("baseline path `{key}`")))?;
        let was_walked = walked.remove(key).is_some();
        match entry(input.tree, &path)? {
            Entry::Missing => pending.push(Pending {
                path,
                kind: ChangeKind::Deleted,
                base: Some(base.clone()),
                new: None,
            }),
            Entry::Link => flag(&path, SkipReason::Link, &mut unsupported),
            Entry::Other => flag(&path, SkipReason::NotAFile, &mut unsupported),
            Entry::File(absolute) => {
                // Baseline files remain reviewable even when newly ignored;
                // they must still count toward the review's resource limits.
                if !was_walked {
                    let metadata = fs::symlink_metadata(&absolute).map_err(|error| {
                        WorkspaceError::io(format!("cannot inspect `{key}`"), error)
                    })?;
                    count.account_file(metadata.len(), input.limits)?;
                }
                let state = hash(
                    &absolute,
                    &path,
                    &mut tree_bytes,
                    input.limits.max_bytes,
                    "ephemeral",
                )?;
                if state != *base {
                    pending.push(Pending {
                        path,
                        kind: ChangeKind::Modified,
                        base: Some(base.clone()),
                        new: Some(state),
                    });
                }
            }
        }
    }
    for (_, path) in walked {
        if walk::is_reserved_path(&path) {
            continue;
        }
        // A walked path may since have been replaced by a link or non-file.
        let absolute = match entry(input.tree, &path)? {
            Entry::File(absolute) => absolute,
            Entry::Link => {
                flag(&path, SkipReason::Link, &mut unsupported);
                continue;
            }
            Entry::Other => {
                flag(&path, SkipReason::NotAFile, &mut unsupported);
                continue;
            }
            Entry::Missing => continue,
        };
        let state = hash(
            &absolute,
            &path,
            &mut tree_bytes,
            input.limits.max_bytes,
            "ephemeral",
        )?;
        pending.push(Pending {
            path,
            kind: ChangeKind::Added,
            base: None,
            new: Some(state),
        });
    }
    pending.sort_by(|a, b| a.path.as_str().cmp(b.path.as_str()));
    unsupported.sort_by(|a, b| a.path.cmp(&b.path));

    let mut budget = input.limits.max_total_diff_bytes;
    // Source conflict checks have their own aggregate cap, so reviewing a tree
    // at its byte limit does not spend that same allowance a second time.
    let mut source_bytes = input.limits.max_bytes;
    let mut changes = Vec::with_capacity(pending.len());
    for Pending {
        path,
        kind,
        base,
        new,
    } in pending
    {
        let source = source_state_with(input.source, &path, |absolute| {
            hash(
                absolute,
                &path,
                &mut source_bytes,
                input.limits.max_bytes,
                "source",
            )
        })?;
        let conflict = conflict(base.as_ref(), new.as_ref(), &source);
        let old_file = match &source {
            SourceState::File { state, .. } => Some(DiffSide {
                root: input.source,
                state,
            }),
            _ => None,
        };
        let text = diff_text(
            &path,
            old_file,
            new.as_ref().map(|state| DiffSide {
                root: input.tree,
                state,
            }),
            input.limits,
            &mut budget,
        )?;
        changes.push(FileChange {
            path,
            kind,
            binary: text.binary,
            size: new.as_ref().map(|state| state.size),
            sha256: new.map(|state| state.sha256),
            unified_diff: text.diff,
            diff_truncated: text.truncated,
            conflict,
        });
    }
    let conflicts = changes
        .iter()
        .filter(|change| change.conflict.is_some())
        .map(|change| change.path.clone())
        .collect();
    Ok(EphemeralDiff {
        workspace_id: input.workspace_id.clone(),
        source_id: input.source_id.clone(),
        digest: digest(&changes),
        changes,
        conflicts,
        unsupported,
    })
}

fn hash(
    absolute: &Path,
    path: &WorkspacePath,
    remaining: &mut u64,
    max_bytes: u64,
    side: &str,
) -> Result<FileState, WorkspaceError> {
    let context = || format!("cannot read {side} `{}`", path.as_str());
    let file = File::open(absolute).map_err(|error| WorkspaceError::io(context(), error))?;
    let state = hash_reader(file, *remaining)
        .map_err(|error| WorkspaceError::io(context(), error))?
        .ok_or_else(|| {
            WorkspaceError::TooLarge(format!("reviewed {side} files exceed {max_bytes} bytes"))
        })?;
    *remaining -= state.size;
    Ok(state)
}

fn hash_reader(input: impl Read, limit: u64) -> io::Result<Option<FileState>> {
    let mut hasher = Sha256::new();
    Ok(
        read_bounded(input, limit, |bytes| hasher.update(bytes))?.map(|size| FileState {
            size,
            sha256: fsutil::hex(&hasher.finalize()),
        }),
    )
}

/// Read at most `limit` bytes plus one overflow probe, without trusting metadata
/// or asking a growable file to allocate its reported size.
fn read_bounded(
    mut input: impl Read,
    limit: u64,
    mut consume: impl FnMut(&[u8]),
) -> io::Result<Option<u64>> {
    let mut buffer = [0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        let available = limit - size;
        let cap = available.saturating_add(1).min(buffer.len() as u64) as usize;
        let read = match input.read(&mut buffer[..cap]) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if read == 0 {
            return Ok(Some(size));
        }
        if read as u64 > available {
            return Ok(None);
        }
        consume(&buffer[..read]);
        size += read as u64;
    }
}

/// SHA-256 over the canonical change list; paths cannot contain tabs or newlines.
pub(crate) fn digest(changes: &[FileChange]) -> String {
    let mut hasher = Sha256::new();
    for change in changes {
        hasher.update(
            format!(
                "{}\t{}\t{}\t{}\n",
                change.kind.label(),
                change.path.as_str(),
                change.sha256.as_deref().unwrap_or("-"),
                change.conflict.map_or("-", ConflictReason::label),
            )
            .as_bytes(),
        );
    }
    fsutil::hex(&hasher.finalize())
}

struct DiffText {
    binary: bool,
    diff: Option<String>,
    truncated: bool,
}

const SNIFF_BYTES: usize = 8 * 1024;

#[derive(Clone, Copy)]
struct DiffSide<'a> {
    root: &'a ConfinedRoot,
    state: &'a FileState,
}

fn changed_during_review(path: &WorkspacePath) -> WorkspaceError {
    WorkspaceError::Conflict(format!(
        "`{}` changed while being reviewed; review it again",
        path.as_str()
    ))
}

fn open_side(side: DiffSide<'_>, path: &WorkspacePath) -> Result<File, WorkspaceError> {
    let Entry::File(absolute) = entry(side.root, path)? else {
        return Err(changed_during_review(path));
    };
    File::open(absolute)
        .map_err(|error| WorkspaceError::io(format!("cannot read `{}`", path.as_str()), error))
}

fn sniff_binary(side: Option<DiffSide<'_>>, path: &WorkspacePath) -> Result<bool, WorkspaceError> {
    let Some(side) = side else {
        return Ok(false);
    };
    let file = open_side(side, path)?;
    let mut buffer = Vec::new();
    file.take(side.state.size.min(SNIFF_BYTES as u64))
        .read_to_end(&mut buffer)
        .map_err(|error| WorkspaceError::io(format!("cannot read `{}`", path.as_str()), error))?;
    Ok(buffer.contains(&0))
}

fn read_side(
    side: Option<DiffSide<'_>>,
    workspace_path: &WorkspacePath,
) -> Result<Vec<u8>, WorkspaceError> {
    let Some(side) = side else {
        return Ok(Vec::new());
    };
    let file = open_side(side, workspace_path)?;
    let mut bytes = Vec::new();
    let size = read_bounded(file, side.state.size, |chunk| {
        bytes.extend_from_slice(chunk)
    })
    .map_err(|error| {
        WorkspaceError::io(format!("cannot read `{}`", workspace_path.as_str()), error)
    })?;
    if size != Some(side.state.size) || fsutil::sha256_hex(&bytes) != side.state.sha256 {
        return Err(changed_during_review(workspace_path));
    }
    Ok(bytes)
}

fn diff_text(
    path: &WorkspacePath,
    old: Option<DiffSide<'_>>,
    new: Option<DiffSide<'_>>,
    limits: &EphemeralLimits,
    budget: &mut usize,
) -> Result<DiffText, WorkspaceError> {
    let too_large = [old, new]
        .into_iter()
        .flatten()
        .any(|side| side.state.size > limits.max_diff_input_bytes);
    if too_large {
        return Ok(DiffText {
            binary: sniff_binary(old, path)? || sniff_binary(new, path)?,
            diff: None,
            truncated: true,
        });
    }
    let old_bytes = read_side(old, path)?;
    let new_bytes = read_side(new, path)?;
    let (Ok(old_text), Ok(new_text)) = (
        std::str::from_utf8(&old_bytes),
        std::str::from_utf8(&new_bytes),
    ) else {
        return Ok(DiffText {
            binary: true,
            diff: None,
            truncated: false,
        });
    };
    if old_text.contains('\0') || new_text.contains('\0') {
        return Ok(DiffText {
            binary: true,
            diff: None,
            truncated: false,
        });
    }
    if *budget == 0 {
        return Ok(DiffText {
            binary: false,
            diff: None,
            truncated: true,
        });
    }
    let old_header = old.map_or_else(
        || "/dev/null".to_owned(),
        |_| format!("a/{}", path.as_str()),
    );
    let new_header = new.map_or_else(
        || "/dev/null".to_owned(),
        |_| format!("b/{}", path.as_str()),
    );
    let mut text = TextDiff::from_lines(old_text, new_text)
        .unified_diff()
        .context_radius(3)
        .header(&old_header, &new_header)
        .to_string();
    let cap = limits.max_file_diff_bytes.min(*budget);
    let mut truncated = false;
    if text.len() > cap {
        let mut cut = cap;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        let cut = text[..cut].rfind('\n').map_or(0, |index| index + 1);
        text.truncate(cut);
        truncated = true;
    }
    *budget -= text.len();
    Ok(DiffText {
        binary: false,
        diff: Some(text),
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // Appended content becomes readable only after the initial contents have
    // been read, without relying on thread timing or filesystem sleeps.
    struct GrowingReader {
        initial: Cursor<&'static [u8]>,
        appended: Cursor<&'static [u8]>,
        consumed: usize,
    }

    impl Read for GrowingReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let mut read = self.initial.read(buffer)?;
            if read == 0 {
                read = self.appended.read(buffer)?;
            }
            self.consumed += read;
            Ok(read)
        }
    }

    #[test]
    fn growing_hash_input_stops_after_one_overflow_probe() {
        let mut input = GrowingReader {
            initial: Cursor::new(b"12"),
            appended: Cursor::new(b"345678"),
            consumed: 0,
        };
        assert!(hash_reader(&mut input, 4).unwrap().is_none());
        assert_eq!(input.consumed, 5);
    }

    #[test]
    fn hashing_accepts_exact_and_empty_limits_and_refuses_zero_limit_growth() {
        assert_eq!(
            hash_reader(Cursor::new(b"abc"), 3).unwrap(),
            Some(FileState {
                size: 3,
                sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into(),
            })
        );
        assert_eq!(
            hash_reader(Cursor::new(b""), 0).unwrap(),
            Some(FileState {
                size: 0,
                sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into(),
            })
        );
        let mut input = Cursor::new(b"abc");
        assert!(hash_reader(&mut input, 0).unwrap().is_none());
        assert_eq!(input.position(), 1);
    }

    #[test]
    fn diff_inputs_changed_after_hashing_are_refused() {
        let tree_dir = tempfile::tempdir().unwrap();
        let tree = ConfinedRoot::new(tree_dir.path()).unwrap();
        let path = WorkspacePath::new("a").unwrap();
        let state = FileState {
            size: 3,
            sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into(),
        };
        let limits = EphemeralLimits {
            max_diff_input_bytes: 3,
            ..EphemeralLimits::default()
        };
        for bytes in [b"abcd".as_slice(), b"ab", b"xyz"] {
            fs::write(tree.root().join("a"), bytes).unwrap();
            let mut budget = limits.max_total_diff_bytes;
            assert!(matches!(
                diff_text(
                    &path,
                    None,
                    Some(DiffSide {
                        root: &tree,
                        state: &state,
                    }),
                    &limits,
                    &mut budget,
                ),
                Err(WorkspaceError::Conflict(_))
            ));
        }
    }

    #[test]
    fn files_growing_after_scan_cannot_exceed_the_aggregate_byte_limit() {
        for has_baseline in [false, true] {
            let source_dir = tempfile::tempdir().unwrap();
            let tree_dir = tempfile::tempdir().unwrap();
            let mut files = BTreeMap::new();
            for (path, bytes) in [("a", b"12"), ("b", b"34")] {
                fs::write(tree_dir.path().join(path), bytes).unwrap();
                if has_baseline {
                    fs::write(source_dir.path().join(path), bytes).unwrap();
                    files.insert(
                        path.to_owned(),
                        FileState {
                            size: 2,
                            sha256: fsutil::sha256_hex(bytes),
                        },
                    );
                }
            }
            let source = ConfinedRoot::new(source_dir.path()).unwrap();
            let tree = ConfinedRoot::new(tree_dir.path()).unwrap();
            let workspace_id = EntityId::new("wsp_copy").unwrap();
            let source_id = EntityId::new("wsp_source").unwrap();
            let baseline = Baseline::new(workspace_id.clone(), source_id.clone(), files);
            let limits = EphemeralLimits {
                max_bytes: 4,
                ..EphemeralLimits::default()
            };
            let input = ReviewInput {
                workspace_id: &workspace_id,
                source_id: &source_id,
                source: &source,
                tree: &tree,
                baseline: &baseline,
                limits: &limits,
            };
            let scanned = scan(&input).unwrap();
            // The real walk recorded four bytes. Each file still fits individually.
            fs::write(tree.root().join("a"), b"123").unwrap();
            assert!(matches!(
                review_scanned(&input, scanned),
                Err(WorkspaceError::TooLarge(_))
            ));
        }
    }
}
