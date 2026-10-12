//! Shared fixtures: temporary drives, plugin folders, and `.zip` archives.

#![allow(dead_code)]

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

use aidex_drive::DrivePaths;
use aidex_events::MemoryEventSink;
use aidex_plugins::PluginStore;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

pub const SECRET: &str = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";

/// A temporary, initialized drive with a plugin store that records events.
pub struct Drive {
    pub root: tempfile::TempDir,
    pub paths: DrivePaths,
    pub events: Arc<MemoryEventSink>,
    pub store: PluginStore,
}

pub fn drive() -> Drive {
    let root = tempfile::tempdir().unwrap();
    aidex_drive::init_with(
        root.path(),
        aidex_drive::InitOptions {
            require_removable: false,
        },
    )
    .unwrap();
    let paths = DrivePaths::open(root.path()).unwrap();
    let events = Arc::new(MemoryEventSink::new());
    let store = PluginStore::open(&paths)
        .unwrap()
        .with_events(events.clone());
    Drive {
        root,
        paths,
        events,
        store,
    }
}

impl Drive {
    /// Entries left in the drive's `plugins/` directory.
    pub fn plugin_dirs(&self) -> Vec<String> {
        names(&self.paths.plugins())
    }

    /// Entries left in the drive's `tmp/` directory (install scratch space).
    pub fn tmp_entries(&self) -> Vec<String> {
        names(&self.paths.tmp())
    }
}

fn names(dir: &Path) -> Vec<String> {
    match fs::read_dir(dir) {
        Ok(entries) => {
            let mut names: Vec<String> = entries
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }
        Err(_) => Vec::new(),
    }
}

/// Write `files` (relative path, content) under `root`.
pub fn write_files(root: &Path, files: &[(&str, &[u8])]) {
    for (path, content) in files {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
}

/// The files of a Claude Code layout plugin named `name`.
pub fn plugin_files(name: &str) -> Vec<(String, Vec<u8>)> {
    vec![
        (
            ".claude-plugin/plugin.json".into(),
            format!(
                r#"{{"name": "{name}", "version": "1.2.0", "description": "Reviews code.",
                    "author": {{"name": "Ada", "email": "ada@example.com"}},
                    "permissions": {{"network": ["api.github.com"], "secrets": ["github-token"]}},
                    "hooks": {{"PreToolUse": []}}}}"#
            )
            .into_bytes(),
        ),
        (
            "skills/review/SKILL.md".into(),
            b"---\nname: review\ndescription: Checks a diff\n---\nRead the diff carefully.\n"
                .to_vec(),
        ),
        (
            "skills/notes/README.md".into(),
            b"not a skill: no SKILL.md here\n".to_vec(),
        ),
        (
            ".mcp.json".into(),
            format!(
                r#"{{"mcpServers": {{"github": {{"command": "npx", "args": ["-y", "server-github"], "env": {{"GITHUB_TOKEN": "{SECRET}"}}}}}}}}"#
            )
            .into_bytes(),
        ),
        ("README.md".into(), b"# Code review\n\nA plugin.\n".to_vec()),
        ("scripts/check.py".into(), b"print('ok')\n".to_vec()),
    ]
}

/// A plugin folder named `name` inside a fresh temporary directory.
pub fn plugin_folder(name: &str) -> (tempfile::TempDir, PathBuf) {
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join(name);
    for (path, content) in plugin_files(name) {
        write_files(&root, &[(&path, &content)]);
    }
    (base, root)
}

/// One archive entry.
pub enum Entry<'a> {
    File(&'a str, &'a [u8]),
    Dir(&'a str),
    Symlink(&'a str, &'a str),
}

/// Write a `.zip` with `entries` (deflated files) at `path`.
pub fn write_zip(path: &Path, entries: &[Entry<'_>]) {
    write_zip_with(path, entries, CompressionMethod::Deflated);
}

pub fn write_zip_with(path: &Path, entries: &[Entry<'_>], method: CompressionMethod) {
    let mut zip = ZipWriter::new(fs::File::create(path).unwrap());
    let options = SimpleFileOptions::default()
        .compression_method(method)
        .large_file(true);
    for entry in entries {
        match entry {
            Entry::File(name, content) => {
                zip.start_file(*name, options).unwrap();
                zip.write_all(content).unwrap();
            }
            Entry::Dir(name) => zip.add_directory(*name, options).unwrap(),
            Entry::Symlink(name, target) => zip.add_symlink(*name, *target, options).unwrap(),
        }
    }
    zip.finish().unwrap();
}

/// A `.zip` of the standard plugin, under a top-level folder when `prefix` is set.
pub fn plugin_zip(dir: &Path, name: &str, prefix: Option<&str>) -> PathBuf {
    let files = plugin_files(name);
    let names: Vec<String> = files
        .iter()
        .map(|(path, _)| match prefix {
            Some(prefix) => format!("{prefix}/{path}"),
            None => path.clone(),
        })
        .collect();
    let mut entries: Vec<Entry<'_>> = Vec::new();
    if let Some(prefix) = prefix {
        entries.push(Entry::Dir(prefix));
    }
    entries.extend(
        names
            .iter()
            .zip(&files)
            .map(|(name, (_, content))| Entry::File(name, content)),
    );
    let path = dir.join(format!("{name}.zip"));
    write_zip(&path, &entries);
    path
}
