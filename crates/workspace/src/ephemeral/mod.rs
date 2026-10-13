//! Ephemeral workspaces (spec §12.3, §23.4): a reviewable copy of a source
//! workspace whose changes are applied back only after review and selection.
//!
//! Layout under the drive's `workspaces/` directory:
//!
//! ```text
//! ephemeral/<id>/aidex-workspace.json   ownership marker
//! ephemeral/<id>/baseline.json          path → size + SHA-256 at copy time
//! ephemeral/<id>/tree/                  the working copy (the workspace root)
//! ```

mod apply;
mod baseline;
mod copy;
mod review;
mod types;

use std::{collections::BTreeSet, path::PathBuf, sync::PoisonError};

use aidex_core::{EntityId, Timestamp};
use aidex_drive::RelativePath;

pub use baseline::BASELINE_FILE;
pub use types::{
    ApplyFailure, ApplyReport, ApplySelection, ChangeKind, ConflictReason, EphemeralCreated,
    EphemeralDiff, EphemeralLimits, FileChange,
};

use crate::{
    ConfinedRoot, Workspace, WorkspaceError, WorkspaceKind, WorkspaceRegistry, WorkspaceRoot,
    model::MAX_NAME_CHARS,
    registry::{EPHEMERAL_DIR, Record, TREE_DIR, generate_id},
};
use baseline::Baseline;

/// Everything an ephemeral operation needs, resolved once.
struct Context {
    record: Record,
    source_id: EntityId,
    source: ConfinedRoot,
    tree: ConfinedRoot,
    base: PathBuf,
}

impl WorkspaceRegistry {
    /// Copy a host or drive workspace into a new ephemeral workspace.
    ///
    /// Honors `.gitignore` (see the crate docs), always skips `.git/` and `.aidex/`,
    /// never follows or copies links, and records a baseline of every copied file.
    /// Exceeding [`EphemeralLimits`] aborts and removes the partial copy.
    pub fn create_ephemeral(&self, source: &EntityId) -> Result<EphemeralCreated, WorkspaceError> {
        let _guard = self
            .ephemeral_ops
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let source_record = self.record(source)?;
        if let WorkspaceKind::Ephemeral { .. } = source_record.kind {
            return Err(WorkspaceError::WrongKind {
                workspace: source.to_string(),
                expected: "a host or drive workspace (ephemeral copies cannot be nested)",
            });
        }
        let source_root = self.confine(&source_record)?;
        let id = generate_id()?;
        let name = ephemeral_name(&source_record.name, &id);
        if self.name_taken(&name)? {
            return Err(WorkspaceError::Conflict(format!(
                "a workspace named `{name}` is already registered"
            )));
        }
        let parent = self
            .paths()
            .ensure_dir(&self.paths().workspaces().join(EPHEMERAL_DIR))?;
        let base = parent.join(id.as_str());
        let tree = self.create_owned_dir(&base, &id, "ephemeral")?;
        let record = Record {
            id: id.clone(),
            name,
            kind: WorkspaceKind::Ephemeral {
                source: source.clone(),
            },
            root: WorkspaceRoot::Drive(
                RelativePath::new(format!("{EPHEMERAL_DIR}/{id}/{TREE_DIR}"))
                    .map_err(|error| WorkspaceError::Corrupt(format!("ephemeral root: {error}")))?,
            ),
            created_at: Timestamp::now(),
            last_opened_at: None,
        };
        let created = copy::copy_tree(&source_root, &tree, &self.limits).and_then(|outcome| {
            Baseline::new(id.clone(), source.clone(), outcome.files.clone()).save(&base)?;
            self.insert_record(&record)?;
            Ok(outcome)
        });
        let outcome = match created {
            Ok(outcome) => outcome,
            Err(error) => {
                // Only the directory this call created (and marked) is removed.
                let _ = std::fs::remove_dir_all(&base);
                return Err(error);
            }
        };
        Ok(EphemeralCreated {
            workspace: self.view(record),
            files: outcome.files.len() as u64,
            bytes: outcome.bytes,
            skipped: outcome.skipped,
        })
    }

    /// Compare the ephemeral tree with its baseline and the current source.
    ///
    /// Requires the source to be available (conflicts cannot be judged otherwise).
    pub fn review(&self, id: &EntityId) -> Result<EphemeralDiff, WorkspaceError> {
        let context = self.context(id)?;
        self.review_context(&context)
    }

    /// Apply the selected non-conflicting changes to the source atomically per file.
    ///
    /// `expected_digest` (from [`EphemeralDiff::digest`]) makes the apply refuse with
    /// [`WorkspaceError::ReviewChanged`] unless the workspace is exactly as reviewed.
    /// Every selected path must be a current change ([`WorkspaceError::NotAChange`]
    /// otherwise, before anything is written). Unselected and conflicting changes are
    /// never written; applied changes are folded into the baseline.
    pub fn apply(
        &self,
        id: &EntityId,
        selection: &ApplySelection,
        expected_digest: Option<&str>,
    ) -> Result<ApplyReport, WorkspaceError> {
        let _guard = self
            .ephemeral_ops
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let context = self.context(id)?;
        let diff = self.review_context(&context)?;
        if let Some(expected) = expected_digest
            && expected != diff.digest
        {
            return Err(WorkspaceError::ReviewChanged {
                expected: expected.to_owned(),
                actual: diff.digest,
            });
        }
        let selected: Vec<&FileChange> = match selection {
            ApplySelection::All => diff.changes.iter().collect(),
            ApplySelection::Paths(paths) => {
                let wanted: BTreeSet<&str> = paths.iter().map(|path| path.as_str()).collect();
                if let Some(missing) = paths.iter().find(|path| {
                    !diff
                        .changes
                        .iter()
                        .any(|change| change.path.as_str() == path.as_str())
                }) {
                    return Err(WorkspaceError::NotAChange(missing.clone()));
                }
                diff.changes
                    .iter()
                    .filter(|change| wanted.contains(change.path.as_str()))
                    .collect()
            }
        };
        let mut baseline = Baseline::load(&context.base, id)?;
        let report = apply::apply(&context.source, &context.tree, &selected, &mut baseline);
        if !report.applied.is_empty() {
            baseline.save(&context.base)?;
        }
        Ok(report)
    }

    /// Delete the ephemeral workspace's directory (only after verifying its marker
    /// and location under `workspaces/ephemeral/`) and unregister it. Nothing
    /// outside that directory is touched; links inside it are removed, not followed.
    pub fn discard(&self, id: &EntityId) -> Result<Workspace, WorkspaceError> {
        let _guard = self
            .ephemeral_ops
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let record = self.record(id)?;
        if !matches!(record.kind, WorkspaceKind::Ephemeral { .. }) {
            return Err(WorkspaceError::WrongKind {
                workspace: id.to_string(),
                expected: "an ephemeral workspace",
            });
        }
        self.delete_owned(record)
    }

    fn context(&self, id: &EntityId) -> Result<Context, WorkspaceError> {
        let record = self.record(id)?;
        let WorkspaceKind::Ephemeral { source } = &record.kind else {
            return Err(WorkspaceError::WrongKind {
                workspace: id.to_string(),
                expected: "an ephemeral workspace",
            });
        };
        let source_id = source.clone();
        let (base, _) = self.owned_dir(&record)?;
        let tree = self.confine(&record)?;
        let source = self.confine(&self.record(&source_id)?)?;
        Ok(Context {
            record,
            source_id,
            source,
            tree,
            base,
        })
    }

    fn review_context(&self, context: &Context) -> Result<EphemeralDiff, WorkspaceError> {
        let baseline = Baseline::load(&context.base, &context.record.id)?;
        if baseline.source_id != context.source_id {
            return Err(WorkspaceError::Corrupt(format!(
                "the baseline of {} names a different source",
                context.record.id
            )));
        }
        review::review(&review::ReviewInput {
            workspace_id: &context.record.id,
            source_id: &context.source_id,
            source: &context.source,
            tree: &context.tree,
            baseline: &baseline,
            limits: &self.limits,
        })
    }
}

/// `<source name>-ephemeral-<last 6 id characters>`, within the name length limit.
fn ephemeral_name(source: &str, id: &EntityId) -> String {
    let suffix: String = id
        .as_str()
        .chars()
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let suffix = format!("-ephemeral-{}", suffix.to_lowercase());
    let room = MAX_NAME_CHARS - suffix.chars().count();
    let stem: String = source.chars().take(room).collect();
    format!("{}{suffix}", stem.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, thread, time::Duration};

    fn open_registry() -> (
        tempfile::TempDir,
        aidex_drive::DrivePaths,
        WorkspaceRegistry,
    ) {
        let drive = tempfile::tempdir().unwrap();
        aidex_drive::init_with(
            drive.path(),
            aidex_drive::InitOptions {
                require_removable: false,
            },
        )
        .unwrap();
        let paths = aidex_drive::DrivePaths::open(drive.path()).unwrap();
        let registry = WorkspaceRegistry::open(&paths).unwrap();
        (drive, paths, registry)
    }

    #[test]
    fn creation_waits_for_source_deletion_before_reading_or_copying() {
        let (_drive, paths, registry) = open_registry();
        let source = registry.create_drive("source").unwrap();
        let tree = registry.confined_root(&source.id).unwrap();
        std::fs::write(tree.root().join("keep.txt"), "source data").unwrap();
        let guard = registry
            .ephemeral_ops
            .lock()
            .unwrap_or_else(PoisonError::into_inner);

        thread::scope(|scope| {
            let (started_tx, started_rx) = mpsc::channel();
            let (result_tx, result_rx) = mpsc::channel();
            let registry_ref = &registry;
            let source_id = &source.id;
            scope.spawn(move || {
                started_tx.send(()).unwrap();
                result_tx
                    .send(registry_ref.create_ephemeral(source_id))
                    .unwrap();
            });
            started_rx.recv().unwrap();
            assert!(matches!(
                result_rx.recv_timeout(Duration::from_millis(200)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ));
            assert_eq!(registry.list().unwrap().len(), 1);
            assert!(!paths.workspaces().join(EPHEMERAL_DIR).exists());
            // Finish the same destructive operation that delete_data performs
            // while holding the operation lock, then let creation proceed.
            registry
                .delete_owned(registry.record(&source.id).unwrap())
                .unwrap();
            drop(guard);
            assert!(matches!(
                result_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
                Err(WorkspaceError::NotFound(_))
            ));
        });
        assert!(registry.list().unwrap().is_empty());
        assert!(!tree.root().exists());
    }

    #[test]
    fn unregistering_a_source_waits_for_ephemeral_creation() {
        let (_drive, _paths, registry) = open_registry();
        let source = registry.create_drive("source").unwrap();
        // Holding the operation lock stands in for a creation in progress.
        let guard = registry
            .ephemeral_ops
            .lock()
            .unwrap_or_else(PoisonError::into_inner);

        thread::scope(|scope| {
            let (started_tx, started_rx) = mpsc::channel();
            let (result_tx, result_rx) = mpsc::channel();
            let registry_ref = &registry;
            let source_id = &source.id;
            scope.spawn(move || {
                started_tx.send(()).unwrap();
                result_tx.send(registry_ref.remove(source_id)).unwrap();
            });
            started_rx.recv().unwrap();
            assert!(matches!(
                result_rx.recv_timeout(Duration::from_millis(200)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ));
            assert!(registry.get(&source.id).is_ok());
            // Finish the creation the way create_ephemeral does: register the copy
            // (its source must still exist), then release the lock.
            let id = generate_id().unwrap();
            registry
                .insert_record(&Record {
                    id: id.clone(),
                    name: ephemeral_name(&source.name, &id),
                    kind: WorkspaceKind::Ephemeral {
                        source: source.id.clone(),
                    },
                    root: WorkspaceRoot::Drive(
                        RelativePath::new(format!("{EPHEMERAL_DIR}/{id}/{TREE_DIR}")).unwrap(),
                    ),
                    created_at: Timestamp::now(),
                    last_opened_at: None,
                })
                .unwrap();
            drop(guard);
            assert!(matches!(
                result_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
                Err(WorkspaceError::Conflict(_))
            ));
        });
        assert_eq!(registry.list().unwrap().len(), 2);
    }
}
