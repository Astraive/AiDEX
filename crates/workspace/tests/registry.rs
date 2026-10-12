//! Registry behaviour: add/list/select across reopen, explicit host roots, drive
//! workspaces, unavailable roots, and deletion limited to AiDEX-created data.

mod common;

use std::fs;

use aidex_core::ServiceErrorCode;
use aidex_drive::DrivePaths;
use aidex_workspace::{
    Access, MARKER_FILE, TREE_DIR, Vcs, WorkspaceAvailability, WorkspaceError, WorkspaceKind,
    WorkspaceRegistry, WorkspaceRoot,
};
use common::{Drive, canonical, project, wp};

#[test]
fn add_list_and_select_survive_reopening_the_registry() {
    let drive = Drive::new();
    let alpha = project(&[("README.md", "alpha")]);
    let beta = project(&[]);
    let (alpha_id, beta_id, drive_id) = {
        let registry = drive.registry();
        let a = registry.add_host(alpha.path(), Some("alpha")).unwrap();
        let b = registry.add_host(beta.path(), None).unwrap();
        let d = registry.create_drive("scratch").unwrap();
        assert!(a.id.as_str().starts_with("wsp_"));
        assert_eq!(a.kind, WorkspaceKind::Host);
        assert!(registry.selected().unwrap().is_none());
        registry.select(&b.id).unwrap();
        (a.id, b.id, d.id)
    };

    // A new registry over the same drive models a daemon restart.
    let registry = drive.registry();
    let listed: Vec<_> = registry.list().unwrap().into_iter().map(|w| w.id).collect();
    assert_eq!(listed, [alpha_id.clone(), beta_id.clone(), drive_id]);
    let selected = registry.selected().unwrap().expect("selection persisted");
    assert_eq!(selected.id, beta_id);
    assert!(selected.last_opened_at.is_some());
    assert_eq!(registry.find("alpha").unwrap().id, alpha_id);
    assert_eq!(registry.find("ALPHA").unwrap().id, alpha_id);
    assert_eq!(registry.find(alpha_id.as_str()).unwrap().id, alpha_id);
    assert!(matches!(
        registry.find("missing"),
        Err(WorkspaceError::NotFound(_))
    ));

    registry.select(&alpha_id).unwrap();
    drop(registry);
    assert_eq!(drive.registry().selected().unwrap().unwrap().id, alpha_id);
}

#[test]
fn the_registry_lives_in_private_config() {
    let drive = Drive::new();
    let _registry = drive.registry();
    assert!(drive.paths().config().join("workspaces.sqlite3").is_file());
}

#[test]
fn host_roots_are_registered_exactly_as_selected() {
    let drive = Drive::new();
    let repo = project(&[("app/src/main.rs", "fn main() {}"), (".git/HEAD", "ref")]);
    let registry = drive.registry();
    let selected = repo.path().join("app");
    let workspace = registry.add_host(&selected, None).unwrap();
    assert_eq!(workspace.name, "app");
    let WorkspaceRoot::Host(root) = &workspace.root else {
        panic!("host root expected");
    };
    assert_eq!(
        canonical(std::path::Path::new(root)),
        canonical(&selected),
        "the root is the selected directory, not the repository root"
    );
    let repository = workspace.repository.expect("enclosing repository detected");
    assert_eq!(repository.vcs, Vcs::Git);
    assert!(!repository.is_workspace_root);
    assert_eq!(
        canonical(std::path::Path::new(&repository.root)),
        canonical(repo.path())
    );

    let at_root = registry.add_host(repo.path(), Some("repo")).unwrap();
    assert!(at_root.repository.unwrap().is_workspace_root);
}

#[test]
fn duplicate_roots_and_names_conflict() {
    let drive = Drive::new();
    let repo = project(&[("sub/file", "x")]);
    let registry = drive.registry();
    registry.add_host(repo.path(), Some("repo")).unwrap();
    // Another spelling of the same directory.
    let respelled = repo.path().join("sub").join("..");
    let error = registry.add_host(&respelled, Some("again")).unwrap_err();
    assert!(matches!(error, WorkspaceError::Conflict(_)), "{error}");
    assert_eq!(error.code(), ServiceErrorCode::Conflict);
    let other = project(&[]);
    let error = registry.add_host(other.path(), Some("REPO")).unwrap_err();
    assert!(matches!(error, WorkspaceError::Conflict(_)), "{error}");
    assert!(matches!(
        registry.create_drive("repo"),
        Err(WorkspaceError::Conflict(_))
    ));
    assert_eq!(registry.list().unwrap().len(), 1);
}

#[test]
fn relative_missing_and_non_directory_paths_are_rejected() {
    let drive = Drive::new();
    let repo = project(&[("file.txt", "x")]);
    let registry = drive.registry();
    for path in [
        std::path::PathBuf::from("relative/dir"),
        repo.path().join("missing"),
        repo.path().join("file.txt"),
    ] {
        let error = registry.add_host(&path, None).unwrap_err();
        assert!(
            matches!(error, WorkspaceError::InvalidPath { .. }),
            "{}: {error}",
            path.display()
        );
        assert_eq!(error.code(), ServiceErrorCode::InvalidArgument);
    }
    for name in ["", "wsp_01H", "bad\nname"] {
        assert!(matches!(
            registry.add_host(repo.path(), Some(name)),
            Err(WorkspaceError::InvalidName(_))
        ));
    }
}

#[test]
fn drive_state_can_never_become_a_host_workspace() {
    let drive = Drive::new();
    let registry = drive.registry();
    let paths = drive.paths();
    fs::create_dir_all(paths.runtime_state()).unwrap();
    let parent = drive.dir.path().parent().unwrap().to_path_buf();
    for forbidden in [
        drive.dir.path().to_path_buf(),
        paths.vault(),
        paths.config(),
        paths.workspaces(),
        paths.runtime_state(),
        parent,
    ] {
        let error = registry.add_host(&forbidden, Some("nope")).unwrap_err();
        assert!(
            matches!(error, WorkspaceError::Forbidden(_)),
            "{}: {error}",
            forbidden.display()
        );
        assert_eq!(error.code(), ServiceErrorCode::PermissionDenied);
    }
    // An ordinary folder on the drive outside managed directories is allowed.
    let user_folder = drive.dir.path().join("projects").join("mine");
    fs::create_dir_all(&user_folder).unwrap();
    registry.add_host(&user_folder, None).unwrap();
}

#[test]
fn unavailable_host_roots_are_reported_and_never_recreated() {
    let drive = Drive::new();
    let repo = project(&[("a.txt", "a")]);
    let root = repo.path().to_path_buf();
    let registry = drive.registry();
    let workspace = registry.add_host(&root, Some("gone")).unwrap();
    assert!(workspace.is_available());
    drop(repo);
    assert!(!root.exists());

    let listed = &registry.list().unwrap()[0];
    let WorkspaceAvailability::Unavailable { reason } = &listed.availability else {
        panic!("expected unavailable, got {:?}", listed.availability);
    };
    assert!(reason.contains("does not exist"), "{reason}");
    assert!(listed.repository.is_none());
    let error = registry.confined_root(&workspace.id).unwrap_err();
    assert!(
        matches!(error, WorkspaceError::RootUnavailable { .. }),
        "{error}"
    );
    assert_eq!(error.code(), ServiceErrorCode::NotFound);
    assert!(matches!(
        registry.create_ephemeral(&workspace.id),
        Err(WorkspaceError::RootUnavailable { .. })
    ));
    // Selection and reopen still work; nothing recreated the directory.
    registry.select(&workspace.id).unwrap();
    drop(registry);
    let reopened = drive.registry();
    assert!(!reopened.get(&workspace.id).unwrap().is_available());
    assert!(!root.exists(), "an unavailable root is never recreated");
}

#[test]
fn remove_unregisters_without_touching_host_files() {
    let drive = Drive::new();
    let repo = project(&[("keep.txt", "keep")]);
    let registry = drive.registry();
    let workspace = registry.add_host(repo.path(), None).unwrap();
    registry.select(&workspace.id).unwrap();
    let error = registry.delete_data(&workspace.id).unwrap_err();
    assert!(matches!(error, WorkspaceError::Forbidden(_)), "{error}");
    registry.remove(&workspace.id).unwrap();
    assert!(registry.list().unwrap().is_empty());
    assert!(registry.selected().unwrap().is_none());
    assert_eq!(
        fs::read_to_string(repo.path().join("keep.txt")).unwrap(),
        "keep"
    );
    assert!(matches!(
        registry.remove(&workspace.id),
        Err(WorkspaceError::NotFound(_))
    ));
}

#[test]
fn drive_workspaces_are_marked_and_survive_moving_the_drive() {
    let drive = Drive::new();
    let workspace = {
        let registry = drive.registry();
        let workspace = registry.create_drive("notes").unwrap();
        assert_eq!(workspace.kind, WorkspaceKind::Drive);
        assert_eq!(
            workspace.root,
            WorkspaceRoot::Drive(
                aidex_drive::RelativePath::new(format!("{}/{TREE_DIR}", workspace.id)).unwrap()
            )
        );
        let base = drive.paths().workspaces().join(workspace.id.as_str());
        assert!(base.join(MARKER_FILE).is_file());
        assert!(base.join(TREE_DIR).is_dir());
        let handle = registry.handle(&workspace.id).unwrap();
        let file = handle.resolve(&wp("todo.md"), Access::Create).unwrap();
        fs::write(file, "remember").unwrap();
        workspace
    };

    // Simulate a drive-letter / mount-point change by moving the whole drive.
    let elsewhere = tempfile::tempdir().unwrap();
    let moved = elsewhere.path().join("moved-drive");
    fs::rename(drive.dir.path(), &moved).unwrap();
    let paths = DrivePaths::open(&moved).unwrap();
    let registry = WorkspaceRegistry::open(&paths).unwrap();
    let reopened = registry.get(&workspace.id).unwrap();
    assert!(reopened.is_available(), "{:?}", reopened.availability);
    assert_eq!(reopened.root, workspace.root);
    let handle = registry.handle(&workspace.id).unwrap();
    let file = handle.resolve(&wp("todo.md"), Access::Read).unwrap();
    assert!(
        aidex_workspace::ConfinedRoot::new(&moved)
            .unwrap()
            .contains(&file)
    );
    assert_eq!(fs::read_to_string(file).unwrap(), "remember");
}

#[test]
fn deleting_drive_data_requires_a_matching_marker_and_touches_nothing_else() {
    let drive = Drive::new();
    let registry = drive.registry();
    let doomed = registry.create_drive("doomed").unwrap();
    let kept = registry.create_drive("kept").unwrap();
    let workspaces = drive.paths().workspaces();
    let sentinel = workspaces.join("user-sentinel.txt");
    fs::write(&sentinel, "mine").unwrap();
    let base = workspaces.join(doomed.id.as_str());
    let tree_file = base.join(TREE_DIR).join("data.txt");
    fs::write(&tree_file, "data").unwrap();

    // A tampered marker blocks deletion entirely.
    let marker = base.join(MARKER_FILE);
    let original = fs::read(&marker).unwrap();
    fs::write(&marker, original.replace_ascii(&doomed.id, &kept.id)).unwrap();
    let error = registry.delete_data(&doomed.id).unwrap_err();
    assert!(
        matches!(error, WorkspaceError::OwnershipMarker(_)),
        "{error}"
    );
    assert!(tree_file.exists());
    fs::write(&marker, &original).unwrap();

    let removed = registry.delete_data(&doomed.id).unwrap();
    assert_eq!(removed.id, doomed.id);
    assert!(!base.exists());
    assert!(registry.get(&doomed.id).is_err());
    assert_eq!(fs::read_to_string(&sentinel).unwrap(), "mine");
    assert!(
        workspaces
            .join(kept.id.as_str())
            .join(MARKER_FILE)
            .is_file()
    );
    assert!(registry.get(&kept.id).unwrap().is_available());
}

#[test]
fn a_missing_drive_workspace_directory_is_unavailable() {
    let drive = Drive::new();
    let registry = drive.registry();
    let workspace = registry.create_drive("lost").unwrap();
    fs::remove_dir_all(drive.paths().workspaces().join(workspace.id.as_str())).unwrap();
    let listed = registry.get(&workspace.id).unwrap();
    assert!(matches!(
        listed.availability,
        WorkspaceAvailability::Unavailable { .. }
    ));
    // Removing data that is already gone just unregisters it.
    registry.delete_data(&workspace.id).unwrap();
    assert!(registry.list().unwrap().is_empty());
}

#[test]
fn a_newer_registry_schema_is_refused() {
    let drive = Drive::new();
    drop(drive.registry());
    let database = drive.paths().config().join("workspaces.sqlite3");
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection.pragma_update(None, "user_version", 99).unwrap();
    drop(connection);
    let error = WorkspaceRegistry::open(&drive.paths()).unwrap_err();
    assert!(matches!(error, WorkspaceError::Incompatible(_)), "{error}");
    assert_eq!(error.code(), ServiceErrorCode::IncompatibleVersion);
}

#[cfg(windows)]
#[test]
fn windows_drive_letter_roots_are_stored_without_verbatim_prefixes() {
    let drive = Drive::new();
    let repo = project(&[]);
    let registry = drive.registry();
    let workspace = registry.add_host(repo.path(), Some("lettered")).unwrap();
    let WorkspaceRoot::Host(root) = &workspace.root else {
        panic!("host root expected");
    };
    let bytes = root.as_bytes();
    assert!(
        bytes[0].is_ascii_alphabetic() && &root[1..3] == ":\\",
        "{root}"
    );
    assert!(!root.starts_with(r"\\?\"));
    assert_eq!(workspace.path, *root);
    // Forward slashes, a verbatim prefix, and different case name the same root.
    let forward = std::path::PathBuf::from(root.replace('\\', "/"));
    let verbatim = std::path::PathBuf::from(format!(r"\\?\{root}"));
    let upper = std::path::PathBuf::from(root.to_uppercase());
    for spelling in [forward, verbatim, upper] {
        assert!(
            matches!(
                registry.add_host(&spelling, Some("dup")),
                Err(WorkspaceError::Conflict(_))
            ),
            "{}",
            spelling.display()
        );
    }
}

trait ReplaceAscii {
    fn replace_ascii(&self, from: &aidex_core::EntityId, to: &aidex_core::EntityId) -> Vec<u8>;
}

impl ReplaceAscii for Vec<u8> {
    fn replace_ascii(&self, from: &aidex_core::EntityId, to: &aidex_core::EntityId) -> Vec<u8> {
        String::from_utf8(self.clone())
            .unwrap()
            .replace(from.as_str(), to.as_str())
            .into_bytes()
    }
}

#[test]
fn a_drive_workspace_redirected_through_a_link_is_refused() {
    let drive = Drive::new();
    let registry = drive.registry();
    let workspace = registry.create_drive("redirected").unwrap();
    let base = drive.paths().workspaces().join(workspace.id.as_str());
    let outside = project(&[("tree/secret.txt", "secret")]);
    fs::remove_dir_all(&base).unwrap();
    if !common::dir_link(outside.path(), &base) {
        return;
    }
    let error = registry.handle(&workspace.id).unwrap_err();
    assert!(matches!(error, WorkspaceError::EscapesRoot(_)), "{error}");
    // Deleting data refuses to follow the link; the outside files survive.
    assert!(registry.delete_data(&workspace.id).is_err());
    assert_eq!(
        fs::read_to_string(outside.path().join("tree/secret.txt")).unwrap(),
        "secret"
    );
}

#[test]
fn a_host_root_replaced_by_a_link_is_unavailable_and_never_accessed() {
    let drive = Drive::new();
    let registry = drive.registry();
    let repo = project(&[("root/original.txt", "original")]);
    let root = repo.path().join("root");
    let outside = project(&[("secret.txt", "secret"), (".git/HEAD", "ref")]);
    let workspace = registry.add_host(&root, Some("redirected-host")).unwrap();
    fs::rename(&root, repo.path().join("original")).unwrap();
    if !common::dir_link(outside.path(), &root) {
        return;
    }

    for current in [
        registry.get(&workspace.id).unwrap(),
        registry.list().unwrap().remove(0),
        drive.registry().get(&workspace.id).unwrap(),
    ] {
        assert!(matches!(
            current.availability,
            WorkspaceAvailability::Unavailable { .. }
        ));
        assert!(current.repository.is_none());
    }
    assert!(matches!(
        registry.confined_root(&workspace.id),
        Err(WorkspaceError::RootUnavailable { .. })
    ));
    assert!(matches!(
        registry.handle(&workspace.id),
        Err(WorkspaceError::RootUnavailable { .. })
    ));
    assert!(matches!(
        registry.create_ephemeral(&workspace.id),
        Err(WorkspaceError::RootUnavailable { .. })
    ));
    assert_eq!(
        fs::read_to_string(outside.path().join("secret.txt")).unwrap(),
        "secret"
    );
    assert_eq!(
        fs::read_to_string(repo.path().join("original/original.txt")).unwrap(),
        "original"
    );
}

#[test]
fn host_roots_cannot_contain_or_be_inside_any_managed_directory() {
    let drive = Drive::new();
    let paths = drive.paths();
    let registry = drive.registry();
    for (_, relative) in paths.layout().entries() {
        let managed = relative.resolve_under(paths.root());
        let child = managed.join("nested");
        fs::create_dir_all(&child).unwrap();
        for forbidden in [
            managed.clone(),
            child,
            managed.parent().unwrap().to_path_buf(),
        ] {
            assert!(
                matches!(
                    registry.add_host(&forbidden, Some("forbidden")),
                    Err(WorkspaceError::Forbidden(_))
                ),
                "{}",
                forbidden.display()
            );
        }
    }
    for allowed in ["private/projects/mine", "models-neighbor", "projects/mine"] {
        let root = paths.root().join(allowed);
        fs::create_dir_all(&root).unwrap();
        registry
            .add_host(&root, Some(allowed.replace('/', "-").as_str()))
            .unwrap();
    }
}

#[test]
fn host_overlap_checks_use_custom_layouts_and_repeat_when_resolving_roots() {
    let drive = Drive::new();
    let registry = drive.registry();
    let root = drive.paths().root().join("projects/mine");
    fs::create_dir_all(&root).unwrap();
    let workspace = registry.add_host(&root, Some("project")).unwrap();
    let layout = aidex_drive::DriveLayout {
        private_vault: aidex_drive::RelativePath::new("projects/mine/state/vault").unwrap(),
        ..aidex_drive::DriveLayout::default()
    };
    let paths = DrivePaths::from_layout(drive.paths().root().to_path_buf(), layout);
    // Refuse containing roots even before the managed child is created.
    let relocated = WorkspaceRegistry::open(&paths).unwrap();
    assert!(matches!(
        relocated.handle(&workspace.id),
        Err(WorkspaceError::Forbidden(_))
    ));
    assert!(matches!(
        relocated.create_ephemeral(&workspace.id),
        Err(WorkspaceError::Forbidden(_))
    ));
    fs::create_dir_all(paths.vault()).unwrap();
    for forbidden in [root.clone(), root.join("state"), paths.vault()] {
        assert!(matches!(
            relocated.add_host(&forbidden, Some("forbidden")),
            Err(WorkspaceError::Forbidden(_))
        ));
    }
    let sibling = root.join("other-project");
    fs::create_dir_all(&sibling).unwrap();
    assert!(relocated.add_host(&sibling, Some("sibling")).is_ok());
}
