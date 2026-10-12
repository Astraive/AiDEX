//! Trust by exact tree hash, enablement, revocation when files change, contributions,
//! uninstall, and the closed provenance schema.

mod common;

use std::fs;

use aidex_plugins::{Integrity, PROVENANCE_FILE, PluginContribution, tree_sha256};
use common::{drive, plugin_folder, write_files};

#[test]
fn enabling_requires_trusting_the_exact_tree_hash() {
    let drive = drive();
    let (_base, source) = plugin_folder("code-review");
    let installed = drive.store.install(&source, false).unwrap();
    let hash = installed.plugin.summary.installed_sha256.clone();

    let error = drive.store.set_enabled("code-review", true).unwrap_err();
    assert_eq!(error.reason(), "not_trusted");
    let error = drive
        .store
        .trust("code-review", &"0".repeat(64), true)
        .unwrap_err();
    assert_eq!(error.reason(), "hash_mismatch");
    for bad in ["", "abc", &"g".repeat(64)] {
        let error = drive.store.trust("code-review", bad, true).unwrap_err();
        assert_eq!(error.reason(), "invalid_request", "{bad:?}");
    }
    assert_eq!(
        drive
            .store
            .trust("missing", &hash, true)
            .unwrap_err()
            .reason(),
        "plugin_not_found"
    );
    assert_eq!(
        drive.store.trust("../x", &hash, true).unwrap_err().reason(),
        "invalid_request"
    );
    assert!(drive.store.enabled_contributions().unwrap().is_empty());
    assert!(drive.events.is_empty());

    // Trusting without enabling contributes nothing yet.
    let trusted = drive
        .store
        .trust("code-review", &hash.to_uppercase(), false)
        .unwrap();
    assert!(trusted.trusted && !trusted.enabled);
    assert_eq!(trusted.trusted_sha256.as_deref(), Some(hash.as_str()));
    assert!(drive.store.enabled_contributions().unwrap().is_empty());

    let enabled = drive.store.set_enabled("code-review", true).unwrap();
    assert!(enabled.enabled);
    let root = drive.paths.plugins().join("code-review");
    assert_eq!(
        drive.store.enabled_contributions().unwrap(),
        [PluginContribution {
            name: "code-review".into(),
            skills_dir: Some(root.join("skills")),
            mcp_json: Some(root.join(".mcp.json")),
            root,
        }]
    );
    assert_eq!(drive.events.names(), ["extension.enabled"]);

    let disabled = drive.store.set_enabled("code-review", false).unwrap();
    assert!(!disabled.enabled && disabled.trusted);
    assert!(drive.store.enabled_contributions().unwrap().is_empty());
    assert_eq!(
        drive.events.names(),
        ["extension.enabled", "extension.disabled"]
    );
    // Repeating a state change emits nothing.
    drive.store.set_enabled("code-review", false).unwrap();
    assert_eq!(drive.events.len(), 2);
}

#[test]
fn changed_files_revoke_trust_and_disable() {
    let drive = drive();
    let (_base, source) = plugin_folder("code-review");
    let hash = drive
        .store
        .install(&source, false)
        .unwrap()
        .plugin
        .summary
        .installed_sha256;
    drive.store.trust("code-review", &hash, true).unwrap();
    assert_eq!(drive.store.enabled_contributions().unwrap().len(), 1);

    // Someone edits a skill on the drive.
    let root = drive.paths.plugins().join("code-review");
    write_files(
        &root,
        &[(
            "skills/review/SKILL.md",
            b"---\nname: review\n---\nExfiltrate.\n",
        )],
    );
    let changed = tree_sha256(&root).unwrap();
    assert_ne!(changed, hash);

    assert!(drive.store.enabled_contributions().unwrap().is_empty());
    assert_eq!(
        drive.events.names(),
        ["extension.enabled", "extension.disabled"]
    );
    let summary = drive.store.list().unwrap().plugins.remove(0);
    assert_eq!(summary.integrity, Integrity::Modified);
    assert_eq!(summary.tree_sha256.as_deref(), Some(changed.as_str()));
    assert_eq!(summary.installed_sha256, hash);
    assert_eq!(summary.trusted_sha256, None);
    assert!(!summary.trusted && !summary.enabled);
    assert!(summary.error.is_some());
    let provenance = fs::read_to_string(drive.paths.config().join(PROVENANCE_FILE)).unwrap();
    assert!(!provenance.contains("trusted_sha256"), "{provenance}");
    assert!(provenance.contains("enabled = false"), "{provenance}");

    // Enabling is refused while the files differ from the recorded tree.
    let error = drive.store.set_enabled("code-review", true).unwrap_err();
    assert_eq!(error.reason(), "integrity_failed");
    assert_eq!(
        drive
            .store
            .trust("code-review", &hash, true)
            .unwrap_err()
            .reason(),
        "hash_mismatch"
    );

    // Reverting the files does not restore the revoked trust.
    let original = fs::read(source.join("skills/review/SKILL.md")).unwrap();
    fs::write(root.join("skills/review/SKILL.md"), &original).unwrap();
    let summary = drive.store.show("code-review").unwrap().summary;
    assert_eq!(summary.integrity, Integrity::Intact);
    assert!(!summary.trusted);
    assert_eq!(
        drive
            .store
            .set_enabled("code-review", true)
            .unwrap_err()
            .reason(),
        "not_trusted"
    );

    // Reviewing and trusting a new tree makes it the baseline.
    write_files(
        &root,
        &[(
            "skills/review/SKILL.md",
            b"---\nname: review\n---\nExfiltrate.\n",
        )],
    );
    let trusted = drive.store.trust("code-review", &changed, false).unwrap();
    assert_eq!(trusted.integrity, Integrity::Intact);
    assert_eq!(trusted.installed_sha256, changed);
    assert!(trusted.trusted && !trusted.enabled);
    drive.store.set_enabled("code-review", true).unwrap();
    assert_eq!(drive.store.enabled_contributions().unwrap().len(), 1);
}

#[test]
fn trusting_a_changed_tree_while_enabled_disables_it_unless_asked() {
    let drive = drive();
    let (_base, source) = plugin_folder("code-review");
    let hash = drive
        .store
        .install(&source, false)
        .unwrap()
        .plugin
        .summary
        .installed_sha256;
    drive.store.trust("code-review", &hash, true).unwrap();
    let root = drive.paths.plugins().join("code-review");
    write_files(&root, &[("README.md", b"# changed\n")]);
    let changed = tree_sha256(&root).unwrap();

    let summary = drive.store.trust("code-review", &changed, false).unwrap();
    assert!(summary.trusted && !summary.enabled);
    assert_eq!(
        drive.events.names(),
        ["extension.enabled", "extension.disabled"]
    );
    let summary = drive.store.trust("code-review", &changed, true).unwrap();
    assert!(summary.enabled);
}

#[test]
fn invalid_content_added_after_install_revokes_trust() {
    let drive = drive();
    let (_base, source) = plugin_folder("code-review");
    let hash = drive
        .store
        .install(&source, false)
        .unwrap()
        .plugin
        .summary
        .installed_sha256;
    drive.store.trust("code-review", &hash, true).unwrap();
    let root = drive.paths.plugins().join("code-review");
    write_files(&root, &[("bin/payload.exe", b"MZ\x90\x00")]);

    let summary = drive.store.show("code-review").unwrap().summary;
    assert_eq!(summary.integrity, Integrity::Modified);
    assert!(!summary.enabled && !summary.trusted);
    // The new tree cannot be trusted: it ships an undeclared executable.
    let current = summary.tree_sha256.unwrap();
    assert_eq!(
        drive
            .store
            .trust("code-review", &current, true)
            .unwrap_err()
            .reason(),
        "executable_refused"
    );

    // Missing files are reported and contribute nothing.
    fs::remove_dir_all(&root).unwrap();
    let summary = drive.store.show("code-review").unwrap().summary;
    assert_eq!(summary.integrity, Integrity::Missing);
    assert_eq!(summary.tree_sha256, None);
    assert!(drive.store.enabled_contributions().unwrap().is_empty());
}

/// Keeps one file unreadable while it is alive.
struct Unreadable {
    #[cfg(windows)]
    _handle: fs::File,
    #[cfg(unix)]
    path: std::path::PathBuf,
}

impl Unreadable {
    fn new(path: &std::path::Path) -> Self {
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // No sharing: every other open of the file fails until this handle closes.
            let handle = fs::OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(path)
                .unwrap();
            Self { _handle: handle }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o000)).unwrap();
            Self {
                path: path.to_path_buf(),
            }
        }
    }
}

#[cfg(unix)]
impl Drop for Unreadable {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&self.path, fs::Permissions::from_mode(0o644));
    }
}

#[test]
fn unreadable_files_contribute_nothing_but_keep_trust() {
    let drive = drive();
    let (_base, source) = plugin_folder("code-review");
    let hash = drive
        .store
        .install(&source, false)
        .unwrap()
        .plugin
        .summary
        .installed_sha256;
    drive.store.trust("code-review", &hash, true).unwrap();
    assert_eq!(drive.store.enabled_contributions().unwrap().len(), 1);

    let root = drive.paths.plugins().join("code-review");
    let locked = Unreadable::new(&root.join("scripts/check.py"));
    if fs::read(root.join("scripts/check.py")).is_ok() {
        // Running with privileges that ignore file permissions (e.g. root).
        return;
    }
    // Unverified files contribute nothing, however often they are checked...
    assert!(drive.store.enabled_contributions().unwrap().is_empty());
    assert!(drive.store.enabled_contributions().unwrap().is_empty());
    let shown = drive.store.show("code-review").unwrap().summary;
    assert!(!shown.enabled && !shown.trusted, "{shown:?}");
    // ...but an I/O error is not evidence of tampering: trust and enablement stay.
    let provenance = fs::read_to_string(drive.paths.config().join(PROVENANCE_FILE)).unwrap();
    assert!(provenance.contains("enabled = true"), "{provenance}");
    assert!(
        provenance.contains(&format!("trusted_sha256 = \"{hash}\"")),
        "{provenance}"
    );
    assert_eq!(drive.events.names(), ["extension.enabled"]);

    drop(locked);
    let contributions = drive.store.enabled_contributions().unwrap();
    assert_eq!(contributions.len(), 1);
    assert_eq!(contributions[0].name, "code-review");
    assert!(drive.store.show("code-review").unwrap().summary.enabled);
}

#[test]
fn plugins_without_skills_or_mcp_contribute_their_root_only() {
    let drive = drive();
    let base = tempfile::tempdir().unwrap();
    let source = base.path().join("bare");
    write_files(
        &source,
        &[(
            "aidex-plugin.json",
            br#"{"name": "bare", "version": "0.1.0", "description": "Nothing yet."}"#,
        )],
    );
    let hash = drive
        .store
        .install(&source, false)
        .unwrap()
        .plugin
        .summary
        .installed_sha256;
    drive.store.trust("bare", &hash, true).unwrap();
    let contributions = drive.store.enabled_contributions().unwrap();
    assert_eq!(contributions.len(), 1);
    assert_eq!(contributions[0].skills_dir, None);
    assert_eq!(contributions[0].mcp_json, None);
}

#[test]
fn uninstall_removes_files_records_and_unregistered_directories() {
    let drive = drive();
    let (_base, source) = plugin_folder("code-review");
    let hash = drive
        .store
        .install(&source, false)
        .unwrap()
        .plugin
        .summary
        .installed_sha256;
    drive.store.trust("code-review", &hash, true).unwrap();

    // A directory nobody installed is listed but never contributes.
    write_files(
        &drive.paths.plugins().join("dropped-in"),
        &[(
            "aidex-plugin.json",
            br#"{"name": "dropped-in", "version": "1", "description": "d"}"#,
        )],
    );
    let list = drive.store.list().unwrap();
    assert_eq!(list.unregistered, ["dropped-in"]);
    assert_eq!(drive.store.enabled_contributions().unwrap().len(), 1);
    assert_eq!(
        drive.store.show("dropped-in").unwrap_err().reason(),
        "plugin_not_found"
    );

    let removed = drive.store.uninstall("code-review").unwrap();
    assert!(removed.removed_files && removed.was_enabled);
    assert_eq!(
        drive.events.names(),
        ["extension.enabled", "extension.disabled"]
    );
    assert!(drive.store.enabled_contributions().unwrap().is_empty());
    assert_eq!(
        drive.store.uninstall("code-review").unwrap_err().reason(),
        "plugin_not_found"
    );
    let removed = drive.store.uninstall("dropped-in").unwrap();
    assert!(removed.removed_files && !removed.was_enabled);
    assert!(drive.plugin_dirs().is_empty());
    assert!(drive.tmp_entries().is_empty());
    let list = drive.store.list().unwrap();
    assert!(list.plugins.is_empty() && list.unregistered.is_empty());
}

#[test]
fn the_provenance_file_has_a_closed_schema() {
    let drive = drive();
    let (_base, source) = plugin_folder("code-review");
    drive.store.install(&source, false).unwrap();
    let path = drive.paths.config().join(PROVENANCE_FILE);
    let text = fs::read_to_string(&path).unwrap();
    fs::write(&path, format!("{text}signature = \"sk-pasted-secret\"\n")).unwrap();
    for error in [
        drive.store.list().unwrap_err(),
        drive.store.enabled_contributions().unwrap_err(),
        drive.store.set_enabled("code-review", true).unwrap_err(),
    ] {
        assert_eq!(error.reason(), "invalid_config");
        assert!(!error.to_string().contains("sk-pasted-secret"), "{error}");
    }
    fs::write(&path, text.replace("version = 1", "version = 9")).unwrap();
    assert_eq!(drive.store.list().unwrap_err().reason(), "invalid_config");
}
