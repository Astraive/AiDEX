//! Ephemeral workspaces: reviewable copies whose changes are applied only when
//! selected and conflict-free, and whose discard removes only AiDEX-created data.

mod common;

use std::fs;

use aidex_core::ServiceErrorCode;
use aidex_workspace::{
    ApplySelection, BASELINE_FILE, ChangeKind, ConflictReason, EPHEMERAL_DIR, EphemeralLimits,
    MARKER_FILE, SkipReason, TREE_DIR, Workspace, WorkspaceError, WorkspaceKind, WorkspaceRegistry,
};
use common::{Drive, dir_link, file_link, files_under, project, wp, write};

struct Fixture {
    drive: Drive,
    source: tempfile::TempDir,
    registry: WorkspaceRegistry,
    source_ws: Workspace,
}

impl Fixture {
    fn new(files: &[(&str, &str)]) -> Self {
        let drive = Drive::new();
        let source = project(files);
        let registry = drive.registry();
        let source_ws = registry.add_host(source.path(), Some("source")).unwrap();
        Self {
            drive,
            source,
            registry,
            source_ws,
        }
    }

    fn base(&self, workspace: &Workspace) -> std::path::PathBuf {
        self.drive
            .paths()
            .workspaces()
            .join(EPHEMERAL_DIR)
            .join(workspace.id.as_str())
    }

    fn tree(&self, workspace: &Workspace) -> std::path::PathBuf {
        self.base(workspace).join(TREE_DIR)
    }

    fn source_text(&self, path: &str) -> Option<String> {
        fs::read_to_string(self.source.path().join(path)).ok()
    }
}

const SOURCE: &[(&str, &str)] = &[
    (".gitignore", "target/\n*.log\n!keep.log\n"),
    ("src/main.rs", "fn main() {\n    println!(\"old\");\n}\n"),
    ("src/lib.rs", "pub fn lib() {}\n"),
    ("keep.log", "kept\n"),
    ("debug.log", "ignored\n"),
    ("target/out.bin", "build output"),
    (".git/HEAD", "ref: refs/heads/main\n"),
    (".git/hooks/pre-commit", "#!/bin/sh\n"),
    (".aidex/worktrees/a/file", "metadata"),
    ("nested/.gitignore", "secret.txt\n"),
    ("nested/secret.txt", "do not copy"),
    ("nested/ok.txt", "copy me"),
];

#[test]
fn creation_copies_the_source_honouring_gitignore_and_skipping_metadata() {
    let fixture = Fixture::new(SOURCE);
    let created = fixture
        .registry
        .create_ephemeral(&fixture.source_ws.id)
        .unwrap();
    let workspace = &created.workspace;
    assert_eq!(
        workspace.kind,
        WorkspaceKind::Ephemeral {
            source: fixture.source_ws.id.clone()
        }
    );
    assert_eq!(
        workspace.root.as_str(),
        format!("{EPHEMERAL_DIR}/{}/{TREE_DIR}", workspace.id)
    );
    assert!(workspace.is_available());
    assert!(workspace.name.starts_with("source-ephemeral-"));
    assert_eq!(
        files_under(&fixture.tree(workspace)),
        [
            ".gitignore",
            "keep.log",
            "nested/.gitignore",
            "nested/ok.txt",
            "src/lib.rs",
            "src/main.rs"
        ]
    );
    assert_eq!(created.files, 6);
    assert!(fixture.base(workspace).join(MARKER_FILE).is_file());
    let baseline = fs::read_to_string(fixture.base(workspace).join(BASELINE_FILE)).unwrap();
    assert!(baseline.contains("\"src/main.rs\""), "{baseline}");
    assert!(!baseline.contains("debug.log"));
    // The fresh copy has no changes.
    let diff = fixture.registry.review(&workspace.id).unwrap();
    assert!(diff.changes.is_empty(), "{:?}", diff.changes);
    assert!(diff.conflicts.is_empty());
}

#[test]
fn repository_ignore_rules_above_a_subdirectory_source_apply() {
    let drive = Drive::new();
    let repo = project(&[
        (".git/HEAD", "ref"),
        (".git/info/exclude", "*.local\n"),
        (".gitignore", "*.tmp\n"),
        ("app/main.rs", "fn main() {}"),
        ("app/scratch.tmp", "tmp"),
        ("app/settings.local", "local"),
    ]);
    let registry = drive.registry();
    let source = registry.add_host(&repo.path().join("app"), None).unwrap();
    let created = registry.create_ephemeral(&source.id).unwrap();
    let tree = drive
        .paths()
        .workspaces()
        .join(EPHEMERAL_DIR)
        .join(created.workspace.id.as_str())
        .join(TREE_DIR);
    assert_eq!(files_under(&tree), ["main.rs"]);
}

#[test]
fn source_links_are_reported_and_never_copied() {
    let fixture = Fixture::new(&[("real/file.txt", "real")]);
    let outside = project(&[("secret.txt", "secret")]);
    if !dir_link(outside.path(), &fixture.source.path().join("escape")) {
        return;
    }
    let created = fixture
        .registry
        .create_ephemeral(&fixture.source_ws.id)
        .unwrap();
    assert_eq!(
        files_under(&fixture.tree(&created.workspace)),
        ["real/file.txt"]
    );
    assert_eq!(created.skipped.len(), 1);
    assert_eq!(created.skipped[0].path, "escape");
    assert_eq!(created.skipped[0].reason, SkipReason::Link);
}

/// Edit the ephemeral copy: modify, delete, add text, add binary.
fn edit(fixture: &Fixture, workspace: &Workspace) {
    let tree = fixture.tree(workspace);
    write(
        &tree,
        "src/main.rs",
        "fn main() {\n    println!(\"new\");\n}\n",
    );
    fs::remove_file(tree.join("keep.log")).unwrap();
    write(&tree, "src/new.rs", "pub fn added() {}\n");
    fs::write(tree.join("image.bin"), [0u8, 159, 146, 150, 0, 1]).unwrap();
    // Ignored output and VCS metadata created in the copy are never reviewable.
    write(&tree, "target/debug/app", "binary");
    write(&tree, ".git/hooks/pre-commit", "#!/bin/sh\nrm -rf /\n");
}

#[test]
fn review_shows_added_modified_and_deleted_files() {
    let fixture = Fixture::new(SOURCE);
    let workspace = fixture
        .registry
        .create_ephemeral(&fixture.source_ws.id)
        .unwrap()
        .workspace;
    edit(&fixture, &workspace);
    let diff = fixture.registry.review(&workspace.id).unwrap();
    let summary: Vec<_> = diff
        .changes
        .iter()
        .map(|change| (change.path.as_str(), change.kind))
        .collect();
    assert_eq!(
        summary,
        [
            ("image.bin", ChangeKind::Added),
            ("keep.log", ChangeKind::Deleted),
            ("src/main.rs", ChangeKind::Modified),
            ("src/new.rs", ChangeKind::Added),
        ]
    );
    assert!(diff.conflicts.is_empty());
    let main = &diff.changes[2];
    let patch = main.unified_diff.as_deref().unwrap();
    assert!(patch.contains("--- a/src/main.rs"), "{patch}");
    assert!(patch.contains("+++ b/src/main.rs"), "{patch}");
    assert!(patch.contains("-    println!(\"old\");"), "{patch}");
    assert!(patch.contains("+    println!(\"new\");"), "{patch}");
    assert!(!main.binary);
    let image = &diff.changes[0];
    assert!(image.binary);
    assert!(image.unified_diff.is_none());
    let deleted = &diff.changes[1];
    assert_eq!(deleted.sha256, None);
    assert!(
        deleted
            .unified_diff
            .as_deref()
            .unwrap()
            .contains("+++ /dev/null")
    );
    // Review is read-only and deterministic.
    assert_eq!(fixture.registry.review(&workspace.id).unwrap(), diff);
    assert_eq!(fixture.source_text("keep.log").as_deref(), Some("kept\n"));
}

#[test]
fn applying_a_subset_changes_only_the_selected_files() {
    let fixture = Fixture::new(SOURCE);
    let workspace = fixture
        .registry
        .create_ephemeral(&fixture.source_ws.id)
        .unwrap()
        .workspace;
    edit(&fixture, &workspace);
    let diff = fixture.registry.review(&workspace.id).unwrap();
    let report = fixture
        .registry
        .apply(
            &workspace.id,
            &ApplySelection::Paths(vec![wp("src/main.rs"), wp("src/new.rs")]),
            Some(&diff.digest),
        )
        .unwrap();
    assert_eq!(report.applied, [wp("src/main.rs"), wp("src/new.rs")]);
    assert!(report.skipped_conflicts.is_empty() && report.failed.is_empty());
    assert!(
        fixture
            .source_text("src/main.rs")
            .unwrap()
            .contains("\"new\"")
    );
    assert_eq!(
        fixture.source_text("src/new.rs").as_deref(),
        Some("pub fn added() {}\n")
    );
    // Unselected changes were not applied: the deletion and the binary file.
    assert_eq!(fixture.source_text("keep.log").as_deref(), Some("kept\n"));
    assert!(!fixture.source.path().join("image.bin").exists());
    // Ignored and metadata paths in the copy never reach the source.
    assert!(!fixture.source.path().join("target/debug/app").exists());
    assert_eq!(
        fixture.source_text(".git/hooks/pre-commit").as_deref(),
        Some("#!/bin/sh\n")
    );
    // Applied changes are folded into the baseline; the rest remain reviewable.
    let remaining: Vec<_> = fixture
        .registry
        .review(&workspace.id)
        .unwrap()
        .changes
        .into_iter()
        .map(|change| change.path)
        .collect();
    assert_eq!(remaining, [wp("image.bin"), wp("keep.log")]);

    let report = fixture
        .registry
        .apply(&workspace.id, &ApplySelection::All, None)
        .unwrap();
    assert_eq!(report.applied, [wp("image.bin"), wp("keep.log")]);
    assert!(!fixture.source.path().join("keep.log").exists());
    assert_eq!(
        fs::read(fixture.source.path().join("image.bin")).unwrap(),
        [0u8, 159, 146, 150, 0, 1]
    );
    assert!(
        fixture
            .registry
            .review(&workspace.id)
            .unwrap()
            .changes
            .is_empty()
    );
}

#[test]
fn conflicting_source_changes_are_detected_and_never_overwritten() {
    let fixture = Fixture::new(SOURCE);
    let workspace = fixture
        .registry
        .create_ephemeral(&fixture.source_ws.id)
        .unwrap()
        .workspace;
    let tree = fixture.tree(&workspace);
    write(&tree, "src/main.rs", "agent version\n");
    write(&tree, "src/lib.rs", "agent lib\n");
    write(&tree, "added.txt", "agent added\n");
    write(&tree, "same.txt", "identical\n");
    fs::remove_file(tree.join("keep.log")).unwrap();
    // Meanwhile the user edits the source.
    write(fixture.source.path(), "src/main.rs", "user version\n");
    write(fixture.source.path(), "added.txt", "user added\n");
    write(fixture.source.path(), "same.txt", "identical\n");
    write(fixture.source.path(), "keep.log", "user edited log\n");

    let diff = fixture.registry.review(&workspace.id).unwrap();
    let conflict = |path: &str| {
        diff.changes
            .iter()
            .find(|change| change.path.as_str() == path)
            .unwrap()
            .conflict
    };
    assert_eq!(conflict("src/main.rs"), Some(ConflictReason::SourceChanged));
    assert_eq!(conflict("added.txt"), Some(ConflictReason::SourceAdded));
    assert_eq!(conflict("keep.log"), Some(ConflictReason::SourceChanged));
    assert_eq!(
        conflict("same.txt"),
        None,
        "identical results do not conflict"
    );
    assert_eq!(conflict("src/lib.rs"), None);
    assert_eq!(
        diff.conflicts,
        [wp("added.txt"), wp("keep.log"), wp("src/main.rs")]
    );

    let report = fixture
        .registry
        .apply(&workspace.id, &ApplySelection::All, Some(&diff.digest))
        .unwrap();
    assert_eq!(report.applied, [wp("same.txt"), wp("src/lib.rs")]);
    assert_eq!(
        report.skipped_conflicts,
        [wp("added.txt"), wp("keep.log"), wp("src/main.rs")]
    );
    assert_eq!(
        fixture.source_text("src/main.rs").as_deref(),
        Some("user version\n")
    );
    assert_eq!(
        fixture.source_text("added.txt").as_deref(),
        Some("user added\n")
    );
    assert_eq!(
        fixture.source_text("keep.log").as_deref(),
        Some("user edited log\n")
    );
    assert_eq!(
        fixture.source_text("src/lib.rs").as_deref(),
        Some("agent lib\n")
    );

    // A source file removed since the copy conflicts with an ephemeral edit.
    write(&tree, "nested/ok.txt", "agent edit\n");
    fs::remove_file(fixture.source.path().join("nested/ok.txt")).unwrap();
    let diff = fixture.registry.review(&workspace.id).unwrap();
    let ok = diff
        .changes
        .iter()
        .find(|change| change.path.as_str() == "nested/ok.txt")
        .unwrap();
    assert_eq!(ok.conflict, Some(ConflictReason::SourceRemoved));
}

#[test]
fn rejected_or_stale_selections_write_nothing() {
    let fixture = Fixture::new(SOURCE);
    let workspace = fixture
        .registry
        .create_ephemeral(&fixture.source_ws.id)
        .unwrap()
        .workspace;
    let tree = fixture.tree(&workspace);
    write(&tree, "src/main.rs", "changed\n");
    let reviewed = fixture.registry.review(&workspace.id).unwrap();

    // Selecting something that is not a change rejects the whole request.
    let error = fixture
        .registry
        .apply(
            &workspace.id,
            &ApplySelection::Paths(vec![wp("src/main.rs"), wp("src/lib.rs")]),
            None,
        )
        .unwrap_err();
    assert!(matches!(error, WorkspaceError::NotAChange(_)), "{error}");
    assert!(fixture.source_text("src/main.rs").unwrap().contains("old"));

    // The copy changed after review: the approved digest no longer matches.
    write(&tree, "src/sneaky.rs", "added after review\n");
    let error = fixture
        .registry
        .apply(&workspace.id, &ApplySelection::All, Some(&reviewed.digest))
        .unwrap_err();
    assert!(
        matches!(error, WorkspaceError::ReviewChanged { .. }),
        "{error}"
    );
    assert_eq!(error.code(), ServiceErrorCode::Conflict);
    assert!(fixture.source_text("src/main.rs").unwrap().contains("old"));
    assert!(!fixture.source.path().join("src/sneaky.rs").exists());

    // An empty selection applies nothing.
    let report = fixture
        .registry
        .apply(&workspace.id, &ApplySelection::Paths(Vec::new()), None)
        .unwrap();
    assert!(report.applied.is_empty());
    assert!(fixture.source_text("src/main.rs").unwrap().contains("old"));
}

#[test]
fn links_in_the_copy_or_the_source_are_never_written_through() {
    let fixture = Fixture::new(SOURCE);
    let outside = project(&[("victim.txt", "untouched")]);
    let workspace = fixture
        .registry
        .create_ephemeral(&fixture.source_ws.id)
        .unwrap()
        .workspace;
    let tree = fixture.tree(&workspace);
    // The agent replaces `nested/` in the copy with a link to an outside directory.
    fs::remove_dir_all(tree.join("nested")).unwrap();
    if !dir_link(outside.path(), &tree.join("nested")) {
        return;
    }
    write(&tree, "src/main.rs", "changed\n");
    let diff = fixture.registry.review(&workspace.id).unwrap();
    assert!(
        diff.unsupported
            .iter()
            .any(|entry| entry.path == "nested" && entry.reason == SkipReason::Link),
        "{:?}",
        diff.unsupported
    );
    assert!(
        diff.changes
            .iter()
            .all(|change| !change.path.as_str().starts_with("nested")),
        "{:?}",
        diff.changes
    );

    // The user's source `src/` is swapped for a link to an outside directory.
    fs::rename(
        fixture.source.path().join("src"),
        fixture.source.path().join("src-moved"),
    )
    .unwrap();
    assert!(dir_link(outside.path(), &fixture.source.path().join("src")));
    let diff = fixture.registry.review(&workspace.id).unwrap();
    let main = diff
        .changes
        .iter()
        .find(|change| change.path.as_str() == "src/main.rs")
        .unwrap();
    assert_eq!(main.conflict, Some(ConflictReason::UnsafePath));
    let report = fixture
        .registry
        .apply(&workspace.id, &ApplySelection::All, None)
        .unwrap();
    assert!(report.skipped_conflicts.contains(&wp("src/main.rs")));
    assert_eq!(files_under(outside.path()), ["victim.txt"]);
    assert_eq!(
        fs::read_to_string(outside.path().join("victim.txt")).unwrap(),
        "untouched"
    );
}

#[test]
fn discard_removes_only_the_ephemeral_directory() {
    let fixture = Fixture::new(SOURCE);
    let outside = project(&[("precious.txt", "precious")]);
    let first = fixture
        .registry
        .create_ephemeral(&fixture.source_ws.id)
        .unwrap()
        .workspace;
    let second = fixture
        .registry
        .create_ephemeral(&fixture.source_ws.id)
        .unwrap()
        .workspace;
    let workspaces = fixture.drive.paths().workspaces();
    let sentinels = [
        workspaces.join("sentinel.txt"),
        workspaces.join(EPHEMERAL_DIR).join("sentinel.txt"),
    ];
    for sentinel in &sentinels {
        fs::write(sentinel, "keep").unwrap();
    }
    let tree = fixture.tree(&first);
    let linked = dir_link(outside.path(), &tree.join("outside-link"));
    if file_link(
        &outside.path().join("precious.txt"),
        &tree.join("file-link"),
    ) {
        assert!(tree.join("file-link").exists());
    }
    write(&tree, "src/work.rs", "agent work\n");

    // A host workspace is never discarded; a tampered marker blocks deletion.
    assert!(matches!(
        fixture.registry.discard(&fixture.source_ws.id),
        Err(WorkspaceError::WrongKind { .. })
    ));
    let marker = fixture.base(&first).join(MARKER_FILE);
    let original = fs::read(&marker).unwrap();
    fs::remove_file(&marker).unwrap();
    assert!(matches!(
        fixture.registry.discard(&first.id),
        Err(WorkspaceError::OwnershipMarker(_))
    ));
    assert!(tree.join("src/work.rs").exists());
    fs::write(&marker, &original).unwrap();

    let discarded = fixture.registry.discard(&first.id).unwrap();
    assert_eq!(discarded.id, first.id);
    assert!(!fixture.base(&first).exists());
    assert!(fixture.registry.get(&first.id).is_err());
    for sentinel in &sentinels {
        assert_eq!(fs::read_to_string(sentinel).unwrap(), "keep");
    }
    if linked {
        assert_eq!(files_under(outside.path()), ["precious.txt"]);
    }
    assert_eq!(
        fs::read_to_string(outside.path().join("precious.txt")).unwrap(),
        "precious"
    );
    assert!(fixture.tree(&second).join("src/main.rs").is_file());
    assert!(fixture.registry.get(&second.id).unwrap().is_available());
    assert!(fixture.source_text("src/main.rs").unwrap().contains("old"));
    assert!(fixture.source.path().join(".git/HEAD").is_file());
}

#[test]
fn copy_limits_abort_and_remove_the_partial_copy() {
    let drive = Drive::new();
    let source = project(&[("a", "1"), ("b", "2"), ("c", "3")]);
    let registry = drive.registry().with_limits(EphemeralLimits {
        max_files: 2,
        ..EphemeralLimits::default()
    });
    let workspace = registry.add_host(source.path(), None).unwrap();
    let error = registry.create_ephemeral(&workspace.id).unwrap_err();
    assert!(matches!(error, WorkspaceError::TooLarge(_)), "{error}");
    let ephemeral = drive.paths().workspaces().join(EPHEMERAL_DIR);
    assert_eq!(fs::read_dir(&ephemeral).unwrap().count(), 0);
    assert_eq!(registry.list().unwrap().len(), 1);

    let registry = drive.registry().with_limits(EphemeralLimits {
        max_bytes: 2,
        ..EphemeralLimits::default()
    });
    assert!(matches!(
        registry.create_ephemeral(&workspace.id),
        Err(WorkspaceError::TooLarge(_))
    ));
    assert_eq!(fs::read_dir(&ephemeral).unwrap().count(), 0);
}

#[test]
fn ephemeral_relationships_are_enforced() {
    let fixture = Fixture::new(&[("a.txt", "a")]);
    let copy = fixture
        .registry
        .create_ephemeral(&fixture.source_ws.id)
        .unwrap()
        .workspace;
    assert!(matches!(
        fixture.registry.create_ephemeral(&copy.id),
        Err(WorkspaceError::WrongKind { .. })
    ));
    assert!(matches!(
        fixture.registry.review(&fixture.source_ws.id),
        Err(WorkspaceError::WrongKind { .. })
    ));
    assert!(matches!(
        fixture.registry.remove(&fixture.source_ws.id),
        Err(WorkspaceError::Conflict(_))
    ));
    // Ephemeral workspaces persist across a registry reopen.
    let reopened = fixture.drive.registry();
    assert_eq!(reopened.get(&copy.id).unwrap().kind, copy.kind);
    assert!(reopened.review(&copy.id).unwrap().changes.is_empty());
    reopened.discard(&copy.id).unwrap();
    reopened.remove(&fixture.source_ws.id).unwrap();
    assert_eq!(fixture.source_text("a.txt").as_deref(), Some("a"));
}

#[test]
fn a_created_ephemeral_child_prevents_deleting_its_drive_source() {
    let drive = Drive::new();
    let registry = drive.registry();
    let source = registry.create_drive("source").unwrap();
    let tree = registry.confined_root(&source.id).unwrap();
    write(tree.root(), "keep.txt", "source data");
    let copy = registry.create_ephemeral(&source.id).unwrap().workspace;
    assert!(matches!(
        registry.delete_data(&source.id),
        Err(WorkspaceError::Conflict(_))
    ));
    assert!(registry.get(&source.id).unwrap().is_available());
    assert_eq!(
        fs::read_to_string(tree.root().join("keep.txt")).unwrap(),
        "source data"
    );
    assert!(registry.review(&copy.id).unwrap().changes.is_empty());
    registry.discard(&copy.id).unwrap();
    registry.delete_data(&source.id).unwrap();
    assert!(registry.list().unwrap().is_empty());
}

#[test]
fn large_diffs_are_bounded() {
    let fixture = Fixture::new(&[("big.txt", "line\n")]);
    let registry = fixture.drive.registry().with_limits(EphemeralLimits {
        max_file_diff_bytes: 200,
        ..EphemeralLimits::default()
    });
    let workspace = registry
        .create_ephemeral(&fixture.source_ws.id)
        .unwrap()
        .workspace;
    let content: String = (0..500).map(|index| format!("line {index}\n")).collect();
    write(&fixture.tree(&workspace), "big.txt", &content);
    let diff = registry.review(&workspace.id).unwrap();
    let change = &diff.changes[0];
    assert!(change.diff_truncated);
    let patch = change.unified_diff.as_deref().unwrap();
    assert!(
        patch.len() <= 200 && patch.ends_with('\n'),
        "{}",
        patch.len()
    );
}

#[test]
fn mutable_review_enforces_file_count_and_byte_limits_before_apply() {
    let drive = Drive::new();
    let source = project(&[("a", "12")]);
    let registry = drive.registry().with_limits(EphemeralLimits {
        max_files: 2,
        max_bytes: 4,
        ..EphemeralLimits::default()
    });
    let source_ws = registry.add_host(source.path(), None).unwrap();
    let copy = registry.create_ephemeral(&source_ws.id).unwrap().workspace;
    let tree = registry.confined_root(&copy.id).unwrap();
    write(tree.root(), "b", "34");
    assert_eq!(registry.review(&copy.id).unwrap().changes.len(), 1);

    write(tree.root(), "c", "");
    assert!(matches!(
        registry.review(&copy.id),
        Err(WorkspaceError::TooLarge(_))
    ));
    assert!(matches!(
        registry.apply(&copy.id, &ApplySelection::All, None),
        Err(WorkspaceError::TooLarge(_))
    ));
    assert!(!source.path().join("b").exists());
    fs::remove_file(tree.root().join("c")).unwrap();
    write(tree.root(), "b", "345");
    assert!(matches!(
        registry.review(&copy.id),
        Err(WorkspaceError::TooLarge(_))
    ));
    fs::remove_file(tree.root().join("b")).unwrap();
    write(tree.root(), "a", "12345");
    assert!(matches!(
        registry.review(&copy.id),
        Err(WorkspaceError::TooLarge(_))
    ));
    write(tree.root(), "a", "12");
    assert!(registry.review(&copy.id).unwrap().changes.is_empty());
    assert_eq!(fs::read_to_string(source.path().join("a")).unwrap(), "12");
}

#[test]
fn review_bounds_the_aggregate_source_bytes_before_apply() {
    let fixture = Fixture::new(&[("a", "12"), ("b", "34")]);
    let registry = fixture.drive.registry().with_limits(EphemeralLimits {
        max_bytes: 4,
        ..EphemeralLimits::default()
    });
    let copy = registry
        .create_ephemeral(&fixture.source_ws.id)
        .unwrap()
        .workspace;
    let tree = fixture.tree(&copy);
    write(&tree, "a", "ab");
    write(&tree, "b", "cd");
    // Each source file fits the cap; their aggregate no longer does.
    write(fixture.source.path(), "a", "123");
    write(fixture.source.path(), "b", "345");
    assert!(matches!(
        registry.review(&copy.id),
        Err(WorkspaceError::TooLarge(_))
    ));
    assert!(matches!(
        registry.apply(&copy.id, &ApplySelection::All, None),
        Err(WorkspaceError::TooLarge(_))
    ));
    assert_eq!(fixture.source_text("a").as_deref(), Some("123"));
    assert_eq!(fixture.source_text("b").as_deref(), Some("345"));
}

#[test]
fn newly_ignored_baseline_files_still_count_toward_review_limits() {
    let drive = Drive::new();
    let source = project(&[(".gitignore", ""), ("tracked", "x")]);
    let registry = drive.registry().with_limits(EphemeralLimits {
        max_files: 2,
        max_bytes: 16,
        ..EphemeralLimits::default()
    });
    let source_ws = registry.add_host(source.path(), None).unwrap();
    let copy = registry.create_ephemeral(&source_ws.id).unwrap().workspace;
    let tree = registry.confined_root(&copy.id).unwrap();
    write(tree.root(), ".gitignore", "tracked\n");
    write(tree.root(), "tracked", "123456789");
    assert!(matches!(
        registry.review(&copy.id),
        Err(WorkspaceError::TooLarge(_))
    ));
    write(tree.root(), "tracked", "x");
    write(tree.root(), "new", "");
    assert!(matches!(
        registry.review(&copy.id),
        Err(WorkspaceError::TooLarge(_))
    ));
    fs::remove_file(tree.root().join("new")).unwrap();
    assert_eq!(registry.review(&copy.id).unwrap().changes.len(), 1);
}

#[test]
fn separate_git_dir_excludes_apply_to_repository_and_subdirectory_sources() {
    for absolute in [false, true] {
        let drive = Drive::new();
        let layout = project(&[
            ("metadata/info/exclude", "/app/private.txt\n"),
            ("repo/app/private.txt", "excluded"),
            ("repo/app/keep.txt", "kept"),
        ]);
        let repo = layout.path().join("repo");
        let target = if absolute {
            layout
                .path()
                .join("metadata")
                .to_string_lossy()
                .into_owned()
        } else {
            "../metadata".into()
        };
        write(&repo, ".git", &format!("gitdir: {target}\r\n"));
        let registry = drive.registry();
        for (name, source_root, kept) in [
            ("root", repo.clone(), "app/keep.txt"),
            ("subdir", repo.join("app"), "keep.txt"),
        ] {
            let source = registry.add_host(&source_root, Some(name)).unwrap();
            let copy = registry.create_ephemeral(&source.id).unwrap().workspace;
            let tree = registry.confined_root(&copy.id).unwrap();
            assert_eq!(files_under(tree.root()), [kept]);
            let base = tree.root().parent().unwrap();
            let baseline = fs::read_to_string(base.join(BASELINE_FILE)).unwrap();
            assert!(!baseline.contains("private.txt"));
            assert!(registry.review(&copy.id).unwrap().changes.is_empty());
        }
    }
}

#[test]
fn linked_worktree_excludes_come_from_commondir_not_private_gitdir() {
    let drive = Drive::new();
    let layout = project(&[
        ("shared/info/exclude", "/app/private.txt\n"),
        ("shared/worktrees/topic/commondir", "../..\n"),
        ("shared/worktrees/topic/info/exclude", "/app/keep.txt\n"),
        ("repo/.git", "gitdir: ../shared/worktrees/topic\n"),
        ("repo/app/private.txt", "excluded"),
        ("repo/app/keep.txt", "kept"),
    ]);
    let registry = drive.registry();
    let source = registry
        .add_host(&layout.path().join("repo/app"), None)
        .unwrap();
    let copy = registry.create_ephemeral(&source.id).unwrap().workspace;
    let tree = registry.confined_root(&copy.id).unwrap();
    assert_eq!(files_under(tree.root()), ["keep.txt"]);
    assert!(registry.review(&copy.id).unwrap().changes.is_empty());
}

#[test]
fn malformed_gitdir_files_do_not_load_unrelated_excludes() {
    let drive = Drive::new();
    let layout = project(&[
        ("metadata/info/exclude", "*.txt\n"),
        ("repo/.git", "../metadata\n"),
        ("repo/keep.txt", "kept"),
    ]);
    let registry = drive.registry();
    let source = registry
        .add_host(&layout.path().join("repo"), None)
        .unwrap();
    let copy = registry.create_ephemeral(&source.id).unwrap().workspace;
    let tree = registry.confined_root(&copy.id).unwrap();
    assert_eq!(files_under(tree.root()), ["keep.txt"]);
}
