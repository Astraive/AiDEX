//! Installing plugins from folders and `.zip` archives: contents, provenance, tree
//! hashes, and every refusal (links, traversal, executables, size caps).

mod common;

use std::{fs, path::Path};

use aidex_plugins::{
    Integrity, MAX_PLUGIN_BYTES, McpTransport, PROVENANCE_FILE, PluginError, tree_sha256,
};
use common::{Entry, SECRET, drive, plugin_folder, plugin_zip, write_files, write_zip};
use zip::CompressionMethod;

fn refused(result: Result<impl std::fmt::Debug, PluginError>, reason: &str) -> PluginError {
    let error = result.expect_err("install should be refused");
    assert_eq!(error.reason(), reason, "{error}");
    error
}

#[test]
fn folders_install_disabled_with_reviewable_contents() {
    let drive = drive();
    let (_base, source) = plugin_folder("code-review");
    let source_hash = tree_sha256(&source).unwrap();

    let outcome = drive.store.install(&source, false).unwrap();
    assert!(!outcome.replaced);
    let plugin = &outcome.plugin;
    let summary = &plugin.summary;
    assert_eq!(summary.name, "code-review");
    assert_eq!(summary.version, "1.2.0");
    assert_eq!(summary.integrity, Integrity::Intact);
    assert_eq!(summary.tree_sha256.as_deref(), Some(source_hash.as_str()));
    assert_eq!(summary.installed_sha256, source_hash);
    assert_eq!(summary.trusted_sha256, None);
    assert!(!summary.trusted && !summary.enabled);
    assert_eq!(summary.location, "plugins/code-review");
    assert_eq!((summary.skill_count, summary.mcp_server_count), (1, 1));
    assert_eq!(summary.error, None);

    let manifest = plugin.manifest.as_ref().unwrap();
    assert_eq!(manifest.permissions.network, ["api.github.com"]);
    assert_eq!(manifest.permissions.secrets, ["github-token"]);
    let contents = plugin.contents.as_ref().unwrap();
    assert_eq!(contents.skills.len(), 1);
    assert_eq!(contents.skills[0].path, "skills/review/SKILL.md");
    assert_eq!(contents.skills[0].name.as_deref(), Some("review"));
    assert_eq!(contents.mcp_servers[0].name, "github");
    assert_eq!(contents.mcp_servers[0].transport, McpTransport::Stdio);
    assert_eq!(contents.mcp_servers[0].command.as_deref(), Some("npx"));
    assert_eq!(contents.mcp_servers[0].env_keys, ["GITHUB_TOKEN"]);
    assert_eq!(contents.ignored_fields, ["hooks"]);
    assert!(contents.readme.as_deref().unwrap().contains("Code review"));
    let shown = serde_json::to_string(plugin).unwrap();
    assert!(!shown.contains(SECRET), "{shown}");

    // Files are on the drive; provenance is recorded; scratch space is gone.
    let installed = drive.paths.plugins().join("code-review");
    assert_eq!(tree_sha256(&installed).unwrap(), source_hash);
    let provenance = fs::read_to_string(drive.paths.config().join(PROVENANCE_FILE)).unwrap();
    assert!(provenance.contains("version = 1"), "{provenance}");
    assert!(provenance.contains(&source_hash), "{provenance}");
    assert!(provenance.contains("enabled = false"), "{provenance}");
    assert!(!provenance.contains("trusted_sha256"), "{provenance}");
    assert!(drive.tmp_entries().is_empty(), "{:?}", drive.tmp_entries());
    assert!(drive.events.is_empty());

    let list = drive.store.list().unwrap();
    assert_eq!(list.plugins, std::slice::from_ref(summary));
    assert!(list.unregistered.is_empty());
    assert_eq!(drive.store.show("code-review").unwrap(), *plugin);
}

#[test]
fn the_review_lists_every_skill_discovery_loads() {
    let drive = drive();
    let base = tempfile::tempdir().unwrap();
    let source = base.path().join("helpers");
    write_files(
        &source,
        &[
            (
                "aidex-plugin.json",
                br#"{"name": "helpers", "version": "1", "description": "Helpers."}"#,
            ),
            (
                "skills/helper/Skill.md",
                b"---\nname: helper\ndescription: Use for every request\x1b[2K\n---\nDo it.\n",
            ),
            (
                "skills/.hidden/SKILL.md",
                b"---\nname: hidden\ndescription: d\n---\nx\n",
            ),
        ],
    );
    let outcome = drive.store.install(&source, false).unwrap();
    assert_eq!(outcome.plugin.summary.skill_count, 1);
    let skills = &outcome.plugin.contents.as_ref().unwrap().skills;
    assert_eq!(skills[0].dir, "helper");
    assert_eq!(skills[0].path, "skills/helper/Skill.md");
    assert_eq!(skills[0].name.as_deref(), Some("helper"));
    let description = skills[0].description.as_deref().unwrap();
    assert!(!description.contains('\x1b'), "{description:?}");
    let shown = drive.store.show("helpers").unwrap();
    assert_eq!(shown.summary.skill_count, 1);
}

#[test]
fn zip_and_folder_sources_hash_identically() {
    let (_base, source) = plugin_folder("code-review");
    let expected = tree_sha256(&source).unwrap();
    let archives = tempfile::tempdir().unwrap();
    for prefix in [None, Some("code-review-main")] {
        let drive = drive();
        let zip = plugin_zip(archives.path(), "code-review", prefix);
        let outcome = drive.store.install(&zip, false).unwrap();
        assert_eq!(
            outcome.plugin.summary.tree_sha256.as_deref(),
            Some(expected.as_str())
        );
        assert_eq!(outcome.plugin.summary.source, zip.display().to_string());
        assert_eq!(drive.plugin_dirs(), ["code-review"]);
        assert!(drive.tmp_entries().is_empty());
    }
}

#[test]
fn existing_plugins_are_replaced_only_on_request() {
    let drive = drive();
    let (_base, source) = plugin_folder("code-review");
    let first = drive.store.install(&source, false).unwrap();
    let hash = first.plugin.summary.installed_sha256.clone();
    refused(drive.store.install(&source, false), "plugin_exists");

    // Identical files keep their trust; enabled state never carries over.
    drive.store.trust("code-review", &hash, true).unwrap();
    let same = drive.store.install(&source, true).unwrap();
    assert!(same.replaced);
    assert!(same.plugin.summary.trusted && !same.plugin.summary.enabled);
    assert_eq!(
        drive.events.names(),
        ["extension.enabled", "extension.disabled"]
    );

    // Changed files are untrusted.
    write_files(
        &source,
        &[("skills/review/SKILL.md", b"---\nname: review\n---\nnew\n")],
    );
    let changed = drive.store.install(&source, true).unwrap();
    assert!(!changed.plugin.summary.trusted);
    assert_eq!(changed.plugin.summary.trusted_sha256, None);
    assert_ne!(changed.plugin.summary.installed_sha256, hash);
    assert_eq!(drive.plugin_dirs(), ["code-review"]);
}

#[test]
fn archives_with_traversal_links_or_unportable_names_are_refused() {
    let drive = drive();
    let archives = tempfile::tempdir().unwrap();
    let manifest: &[u8] = br#"{"name": "evil", "version": "1", "description": "d"}"#;
    let cases: [(&str, Vec<Entry<'_>>); 6] = [
        (
            "traversal",
            vec![
                Entry::File("aidex-plugin.json", manifest),
                Entry::File("../escaped.txt", b"x"),
            ],
        ),
        (
            "nested-traversal",
            vec![
                Entry::File("aidex-plugin.json", manifest),
                Entry::File("a/../../escaped.txt", b"x"),
            ],
        ),
        (
            "absolute",
            vec![
                Entry::File("aidex-plugin.json", manifest),
                Entry::File("/escaped.txt", b"x"),
            ],
        ),
        (
            "drive-letter",
            vec![
                Entry::File("aidex-plugin.json", manifest),
                Entry::File("C:/escaped.txt", b"x"),
            ],
        ),
        (
            "backslash",
            vec![
                Entry::File("aidex-plugin.json", manifest),
                Entry::File("a\\..\\..\\escaped.txt", b"x"),
            ],
        ),
        (
            "symlink",
            vec![
                Entry::File("aidex-plugin.json", manifest),
                Entry::Symlink("link", "../../outside"),
            ],
        ),
    ];
    for (label, entries) in cases {
        let path = archives.path().join(format!("{label}.zip"));
        write_zip(&path, &entries);
        let error = refused(drive.store.install(&path, false), "unsafe_content");
        assert!(!error.to_string().is_empty(), "{label}");
        assert!(drive.plugin_dirs().is_empty(), "{label}");
        assert!(
            drive.tmp_entries().is_empty(),
            "{label}: {:?}",
            drive.tmp_entries()
        );
    }
    assert!(!drive.root.path().join("escaped.txt").exists());
    assert!(!drive.paths.tmp().join("escaped.txt").exists());
    assert!(
        !drive
            .paths
            .root()
            .parent()
            .unwrap()
            .join("escaped.txt")
            .exists()
    );
    assert!(drive.store.list().unwrap().plugins.is_empty());

    // Case-only duplicates would collide on Windows and macOS.
    let path = archives.path().join("case.zip");
    write_zip(
        &path,
        &[
            Entry::File("aidex-plugin.json", manifest),
            Entry::File("Notes.md", b"a"),
            Entry::File("notes.md", b"b"),
        ],
    );
    refused(drive.store.install(&path, false), "unsafe_content");

    // A refused name is shown escaped: it cannot drive a terminal or fake lines.
    let path = archives.path().join("escapes.zip");
    write_zip(
        &path,
        &[
            Entry::File("aidex-plugin.json", manifest),
            Entry::File("a\u{1b}]52;c;eA==\u{7}\rfake.md", b"x"),
        ],
    );
    let error = refused(drive.store.install(&path, false), "unsafe_content");
    let message = error.to_string();
    assert!(!message.chars().any(char::is_control), "{message:?}");
    assert!(
        message.contains(r"a\u{1b}]52;c;eA==\u{7}\rfake.md"),
        "{message}"
    );
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).is_ok()
}

#[cfg(windows)]
fn symlink(target: &Path, link: &Path) -> bool {
    // Needs Developer Mode or elevation; the test is skipped without it.
    std::os::windows::fs::symlink_file(target, link).is_ok()
}

#[test]
fn folders_with_links_are_refused() {
    let drive = drive();
    let (base, source) = plugin_folder("linked");
    let outside = base.path().join("outside.txt");
    fs::write(&outside, "secret").unwrap();
    if !symlink(&outside, &source.join("skills/review/linked.md")) {
        eprintln!("skipping: symbolic links are unavailable");
        return;
    }
    refused(drive.store.install(&source, false), "unsafe_content");
    assert!(drive.plugin_dirs().is_empty());

    // A link as the source itself is refused too.
    let (_other, real) = plugin_folder("real");
    let link = base.path().join("link-to-plugin");
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(&real, &link).is_ok();
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_dir(&real, &link).is_ok();
    if made {
        refused(drive.store.install(&link, false), "unsafe_content");
    }
}

#[test]
fn oversize_plugins_are_refused() {
    let drive = drive();
    let (_base, source) = plugin_folder("big");
    let big = fs::File::create(source.join("blob.bin")).unwrap();
    big.set_len(MAX_PLUGIN_BYTES + 1).unwrap();
    drop(big);
    refused(drive.store.install(&source, false), "too_large");
    assert!(drive.plugin_dirs().is_empty());
    assert!(drive.tmp_entries().is_empty());

    // An archive declaring more than the cap is refused before it is written out.
    let archives = tempfile::tempdir().unwrap();
    let path = archives.path().join("big.zip");
    let blob = vec![0_u8; usize::try_from(MAX_PLUGIN_BYTES).unwrap() + 1];
    common::write_zip_with(
        &path,
        &[
            Entry::File(
                "aidex-plugin.json",
                br#"{"name": "big", "version": "1", "description": "d"}"#,
            ),
            Entry::File("blob.bin", &blob),
        ],
        CompressionMethod::Stored,
    );
    refused(drive.store.install(&path, false), "too_large");
    assert!(drive.tmp_entries().is_empty());
}

#[test]
fn undeclared_executables_are_refused_and_declared_mcp_commands_allowed() {
    let drive = drive();
    let (_base, source) = plugin_folder("tools");
    write_files(&source, &[("bin/tool.exe", b"MZ\x90\x00binary")]);
    let error = refused(drive.store.install(&source, false), "executable_refused");
    assert!(error.to_string().contains("bin/tool.exe"), "{error}");

    // Executable content is recognized without an extension.
    fs::remove_file(source.join("bin/tool.exe")).unwrap();
    write_files(&source, &[("bin/data.bin", b"\x7fELF\x02\x01")]);
    refused(drive.store.install(&source, false), "executable_refused");
    fs::remove_file(source.join("bin/data.bin")).unwrap();

    // Declared as an MCP server command, the same binary installs.
    write_files(
        &source,
        &[
            ("bin/tool.exe", b"MZ\x90\x00binary"),
            (
                ".mcp.json",
                br#"{"mcpServers": {"tool": {"command": "${CLAUDE_PLUGIN_ROOT}/bin/tool.exe"}}}"#,
            ),
        ],
    );
    let outcome = drive.store.install(&source, false).unwrap();
    let servers = &outcome.plugin.contents.unwrap().mcp_servers;
    assert_eq!(servers[0].bundled_command.as_deref(), Some("bin/tool.exe"));
}

#[test]
fn invalid_sources_and_manifests_are_refused() {
    let drive = drive();
    refused(
        drive.store.install(Path::new("relative/plugin"), false),
        "invalid_request",
    );
    let base = tempfile::tempdir().unwrap();
    refused(
        drive.store.install(&base.path().join("missing"), false),
        "invalid_request",
    );
    let text = base.path().join("plugin.txt");
    fs::write(&text, "x").unwrap();
    refused(drive.store.install(&text, false), "invalid_request");

    let empty = base.path().join("empty");
    fs::create_dir(&empty).unwrap();
    refused(drive.store.install(&empty, false), "invalid_manifest");

    let bad = base.path().join("bad");
    write_files(
        &bad,
        &[(
            "aidex-plugin.json",
            br#"{"name": "Bad Name", "version": "1", "description": "d"}"#,
        )],
    );
    refused(drive.store.install(&bad, false), "invalid_manifest");

    let bad_mcp = base.path().join("bad-mcp");
    write_files(
        &bad_mcp,
        &[
            (
                "aidex-plugin.json",
                br#"{"name": "bad-mcp", "version": "1", "description": "d"}"#,
            ),
            (".mcp.json", br#"{"mcpServers": {"x": {}}}"#),
        ],
    );
    refused(drive.store.install(&bad_mcp, false), "invalid_manifest");

    let not_zip = base.path().join("plugin.zip");
    fs::write(&not_zip, "not a zip").unwrap();
    refused(drive.store.install(&not_zip, false), "invalid_request");

    // The drive's private directories are never an install source.
    let config = drive.paths.ensure_dir(&drive.paths.config()).unwrap();
    write_files(
        &config,
        &[(
            "aidex-plugin.json",
            br#"{"name": "cfg", "version": "1", "description": "d"}"#,
        )],
    );
    let error = refused(drive.store.install(&config, false), "invalid_request");
    assert!(error.to_string().contains("private"), "{error}");
    fs::remove_file(config.join("aidex-plugin.json")).unwrap();
    refused(
        drive.store.install(drive.root.path(), false),
        "invalid_request",
    );

    assert!(drive.plugin_dirs().is_empty());
    assert!(drive.tmp_entries().is_empty());
    assert!(drive.store.list().unwrap().plugins.is_empty());
}

#[test]
fn aidex_manifests_take_precedence() {
    let drive = drive();
    let (_base, source) = plugin_folder("claude-name");
    write_files(
        &source,
        &[(
            "aidex-plugin.json",
            br#"{"name": "aidex-name", "version": "2.0.0", "description": "Native manifest."}"#,
        )],
    );
    let outcome = drive.store.install(&source, false).unwrap();
    assert_eq!(outcome.plugin.summary.name, "aidex-name");
    assert_eq!(
        outcome.plugin.manifest_file.map(|file| file.path()),
        Some("aidex-plugin.json")
    );
    assert_eq!(drive.plugin_dirs(), ["aidex-name"]);
}
