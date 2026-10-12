//! `PluginStore`: install, inspect, trust, enable, and uninstall plugins on a drive.
//!
//! Files live in `DrivePaths::plugins()/<name>/`; provenance and trust live in
//! `private_config/plugins.toml`. Every read re-hashes the plugin tree: a plugin whose
//! files are missing, invalid, or no longer match the recorded tree hash loses its
//! trust and is disabled (and `extension.disabled` is emitted). Only enabled plugins
//! whose exact current tree the user trusted contribute skills and MCP servers.

use std::{
    fmt, fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use aidex_core::{EntityId, Timestamp};
use aidex_drive::{DriveError, DrivePaths};
use aidex_events::{EventKind, EventScope, EventSink, ExtensionKind, redact_text};
use serde::{Deserialize, Serialize};

use crate::{
    PluginError,
    contents::{self, MCP_JSON, McpServerEntry, PluginContents, SKILLS_DIR},
    executables,
    fsutil::{ScratchDir, rename_retry},
    install,
    limits::MAX_INSTALLED_PLUGINS,
    manifest::{self, LoadedManifest, ManifestFile, PluginManifest},
    names::{is_tree_hash, validate_name},
    provenance::{self, PROVENANCE_FILE, PluginRecord, Provenance},
    tree::{self, Tree},
};

/// Most unregistered directory names reported by [`PluginStore::list`].
const MAX_UNREGISTERED: usize = 256;
const MAX_SOURCE_TEXT: usize = 4096;

/// How a plugin's files compare with its recorded tree hash.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Integrity {
    /// The files hash to the recorded tree hash.
    Intact,
    /// The files changed since install (or since they were last trusted).
    Modified,
    /// The plugin directory is gone.
    Missing,
    /// The directory holds something a plugin may not (a link, oversize content, ...).
    Invalid,
}

/// One installed plugin, as listed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginSummary {
    pub name: String,
    pub version: String,
    /// From the current manifest (redacted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub integrity: Integrity,
    /// The current tree hash; absent when the files are missing or invalid.
    pub tree_sha256: Option<String>,
    /// The recorded tree hash: at install, or when a changed tree was last trusted.
    pub installed_sha256: String,
    /// The exact tree hash the user trusted.
    pub trusted_sha256: Option<String>,
    /// The user trusted exactly the current files.
    pub trusted: bool,
    /// Enabled and trusted: the plugin contributes its skills and MCP servers.
    pub enabled: bool,
    pub installed_at: Timestamp,
    /// The folder or archive it was installed from.
    pub source: String,
    /// Drive-relative directory, e.g. `plugins/code-review`.
    pub location: String,
    pub skill_count: usize,
    pub mcp_server_count: usize,
    /// Why the plugin cannot be used as installed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Everything the review dialog shows: summary, manifest, and contents.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PluginDetail {
    #[serde(flatten)]
    pub summary: PluginSummary,
    pub manifest_file: Option<ManifestFile>,
    pub manifest: Option<PluginManifest>,
    pub contents: Option<PluginContents>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginList {
    /// Recorded plugins, sorted by name.
    pub plugins: Vec<PluginSummary>,
    /// Directories under `plugins/` with no provenance record; they never contribute.
    pub unregistered: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InstallOutcome {
    #[serde(flatten)]
    pub plugin: PluginDetail,
    /// An installed plugin of the same name was replaced.
    pub replaced: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Uninstalled {
    pub name: String,
    /// The plugin directory existed and was removed.
    pub removed_files: bool,
    pub was_enabled: bool,
}

/// What an enabled, trusted, intact plugin contributes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PluginContribution {
    pub name: String,
    /// The plugin directory.
    pub root: PathBuf,
    /// `<root>/skills` when present (`plugin`-scope skills).
    pub skills_dir: Option<PathBuf>,
    /// `<root>/.mcp.json` when present (imported as DISABLED MCP server entries).
    pub mcp_json: Option<PathBuf>,
}

/// Installed plugins on one drive. Operations are serialized by an internal lock.
pub struct PluginStore {
    paths: DrivePaths,
    events: Option<Arc<dyn EventSink>>,
    lock: Mutex<()>,
}

impl fmt::Debug for PluginStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PluginStore")
            .field("plugins", &self.paths.plugins())
            .field("events", &self.events.is_some())
            .finish_non_exhaustive()
    }
}

/// The result of re-hashing one recorded plugin.
struct Checked {
    integrity: Integrity,
    tree: Option<Tree>,
    hash: Option<String>,
    error: Option<String>,
    /// The files could not be read (an I/O error, not a content problem): the plugin
    /// contributes nothing this time, but its trust is not revoked.
    transient: bool,
}

impl Checked {
    fn failed(integrity: Integrity, error: impl Into<String>) -> Self {
        Self {
            integrity,
            tree: None,
            hash: None,
            error: Some(error.into()),
            transient: false,
        }
    }

    fn unreadable(error: impl Into<String>) -> Self {
        Self {
            transient: true,
            ..Self::failed(Integrity::Invalid, error)
        }
    }

    fn integrity_error(&self, name: &str) -> PluginError {
        PluginError::Integrity {
            name: name.to_owned(),
            reason: self
                .error
                .clone()
                .unwrap_or_else(|| "the plugin files do not match".into()),
        }
    }
}

/// What enforcing the integrity rule changed in a record.
#[derive(Default)]
struct Revocation {
    changed: bool,
    disabled: bool,
}

impl PluginStore {
    /// A store over the drive's `plugins/` directory and `private_config/plugins.toml`.
    /// Nothing is read or created until an operation needs it.
    pub fn open(paths: &DrivePaths) -> Result<Self, PluginError> {
        Ok(Self {
            paths: paths.clone(),
            events: None,
            lock: Mutex::new(()),
        })
    }

    /// Emit `extension.enabled` / `extension.disabled` (kind `plugin`) through `events`.
    pub fn with_events(mut self, events: Arc<dyn EventSink>) -> Self {
        self.events = Some(events);
        self
    }

    /// `DrivePaths::plugins()`.
    pub fn plugins_dir(&self) -> PathBuf {
        self.paths.plugins()
    }

    /// `private_config/plugins.toml`.
    pub fn provenance_path(&self) -> PathBuf {
        self.paths.config().join(PROVENANCE_FILE)
    }

    /// Every recorded plugin, re-verified, plus unregistered directories.
    pub fn list(&self) -> Result<PluginList, PluginError> {
        let _guard = self.guard();
        let mut provenance = self.load()?;
        let mut revocations = Vec::new();
        let mut plugins = Vec::with_capacity(provenance.plugins.len());
        for record in &mut provenance.plugins {
            let checked = self.check(record);
            revocations.push((record.name.clone(), enforce(record, &checked)));
            plugins.push(self.detail(record, &checked).summary);
        }
        self.commit_revocations(&provenance, revocations)?;
        plugins.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(PluginList {
            plugins,
            unregistered: self.unregistered(&provenance)?,
        })
    }

    /// One plugin with its manifest and contents, re-verified.
    pub fn show(&self, name: &str) -> Result<PluginDetail, PluginError> {
        validate_name(name).map_err(PluginError::InvalidRequest)?;
        let _guard = self.guard();
        let mut provenance = self.load()?;
        let index = provenance
            .position(name)
            .ok_or_else(|| PluginError::NotFound(name.to_owned()))?;
        let record = &mut provenance.plugins[index];
        let checked = self.check(record);
        let revocation = enforce(record, &checked);
        let detail = self.detail(record, &checked);
        self.commit_revocations(&provenance, vec![(name.to_owned(), revocation)])?;
        Ok(detail)
    }

    /// Install a plugin from an absolute folder or `.zip` path, DISABLED.
    ///
    /// The source is staged on the drive (bounded, links refused), validated (manifest,
    /// `.mcp.json`, no undeclared executables), hashed, and moved into
    /// `plugins/<name>/`. An existing plugin of that name is replaced only with
    /// `replace`; trust carries over only when the new tree hash is identical.
    pub fn install(&self, source: &Path, replace: bool) -> Result<InstallOutcome, PluginError> {
        let kind = install::source_kind(source)?;
        install::refuse_private_source(&self.paths, source)?;
        let tmp = self.ensure_dir(&self.paths.tmp())?;
        let scratch = ScratchDir::create(&tmp, "plugin-install")?;
        let staged = scratch.path().join("source");
        install::stage(source, kind, &staged)?;
        let root = install::locate_root(&staged)?;
        let tree = tree::scan(&root)?;
        let (loaded, servers) = validate_plugin(&root, &tree, None)?;
        let declared: Vec<String> = servers
            .iter()
            .filter_map(|server| server.bundled_command.clone())
            .collect();
        executables::mark_declared(&root, &declared)?;
        let hash = tree::hash(&root, &tree)?;
        let name = loaded.manifest.name.clone();

        let _guard = self.guard();
        let mut provenance = self.load()?;
        let index = provenance.position(&name);
        if index.is_none() && provenance.plugins.len() >= MAX_INSTALLED_PLUGINS {
            return Err(PluginError::TooLarge(format!(
                "at most {MAX_INSTALLED_PLUGINS} plugins can be installed"
            )));
        }
        let destination = self.ensure_dir(&self.paths.plugins())?.join(&name);
        let exists = match fs::symlink_metadata(&destination) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => true,
            Ok(_) => {
                return Err(PluginError::Io(format!(
                    "`{}` exists but is not a directory; remove it first",
                    destination.display()
                )));
            }
            Err(error) if error.kind() == ErrorKind::NotFound => false,
            Err(error) => return Err(PluginError::io("inspect", &destination, &error)),
        };
        if (exists || index.is_some()) && !replace {
            return Err(PluginError::Exists(name));
        }
        let previous = scratch.path().join("previous");
        if exists {
            rename_retry(&destination, &previous)
                .map_err(|error| PluginError::io("move aside", &destination, &error))?;
        }
        if let Err(error) = rename_retry(&root, &destination) {
            if exists {
                let _ = rename_retry(&previous, &destination);
            }
            return Err(PluginError::io("move into place", &destination, &error));
        }
        let old = index.map(|index| provenance.plugins[index].clone());
        let record = PluginRecord {
            name: name.clone(),
            version: loaded.manifest.version.clone(),
            tree_sha256: hash.clone(),
            installed_at: Timestamp::now(),
            source: source_text(source),
            trusted_sha256: old
                .as_ref()
                .and_then(|old| old.trusted_sha256.clone())
                .filter(|trusted| *trusted == hash),
            enabled: false,
        };
        match index {
            Some(index) => provenance.plugins[index] = record.clone(),
            None => provenance.plugins.push(record.clone()),
        }
        if let Err(error) = self.save(&provenance) {
            // Put the previous files back so the drive matches the unchanged records.
            let _ = rename_retry(&destination, &scratch.path().join("failed"));
            if exists {
                let _ = rename_retry(&previous, &destination);
            }
            return Err(error);
        }
        drop(scratch);
        let checked = Checked {
            integrity: Integrity::Intact,
            tree: Some(tree),
            hash: Some(hash),
            error: None,
            transient: false,
        };
        let plugin = self.detail(&record, &checked);
        if old.as_ref().is_some_and(|old| old.enabled) {
            self.emit(&name, false)?;
        }
        Ok(InstallOutcome {
            plugin,
            replaced: exists || old.is_some(),
        })
    }

    /// Trust exactly `tree_sha256` (the hash the user reviewed) and optionally enable.
    ///
    /// Refused when the plugin's current tree hash differs (`hash_mismatch`) or its
    /// files are missing or invalid. Trusting a modified tree re-validates it and makes
    /// it the new recorded baseline.
    pub fn trust(
        &self,
        name: &str,
        tree_sha256: &str,
        enable: bool,
    ) -> Result<PluginSummary, PluginError> {
        validate_name(name).map_err(PluginError::InvalidRequest)?;
        let expected = tree_sha256.trim().to_ascii_lowercase();
        if !is_tree_hash(&expected) {
            return Err(PluginError::InvalidRequest(
                "`tree_sha256` must be 64 hexadecimal digits".into(),
            ));
        }
        let _guard = self.guard();
        let mut provenance = self.load()?;
        let index = provenance
            .position(name)
            .ok_or_else(|| PluginError::NotFound(name.to_owned()))?;
        let record = &mut provenance.plugins[index];
        let was_enabled = record.enabled;
        let checked = self.check(record);
        // A changed tree loses its old trust and is disabled before the new hash is
        // considered, so trusting changed files never leaves them enabled implicitly.
        let revocation = enforce(record, &checked);
        let refused = match (&checked.tree, &checked.hash) {
            (Some(tree), Some(current)) if *current == expected => {
                validate_plugin(&self.plugin_dir(name), tree, Some(name)).err()
            }
            (Some(_), Some(_)) => Some(PluginError::HashMismatch {
                name: name.to_owned(),
            }),
            _ => Some(checked.integrity_error(name)),
        };
        if let Some(error) = refused {
            self.commit_revocations(&provenance, vec![(name.to_owned(), revocation)])?;
            return Err(error);
        }
        let loaded = manifest::load(&self.plugin_dir(name))?;
        record.version = loaded.manifest.version;
        record.tree_sha256.clone_from(&expected);
        record.trusted_sha256 = Some(expected.clone());
        if enable {
            record.enabled = true;
        }
        let record = record.clone();
        self.save(&provenance)?;
        let checked = Checked {
            integrity: Integrity::Intact,
            error: None,
            ..checked
        };
        let summary = self.detail(&record, &checked).summary;
        if record.enabled != was_enabled {
            self.emit(name, record.enabled)?;
        }
        Ok(summary)
    }

    /// Enable (requires trust in the exact current tree) or disable a plugin.
    pub fn set_enabled(&self, name: &str, enabled: bool) -> Result<PluginSummary, PluginError> {
        validate_name(name).map_err(PluginError::InvalidRequest)?;
        let _guard = self.guard();
        let mut provenance = self.load()?;
        let index = provenance
            .position(name)
            .ok_or_else(|| PluginError::NotFound(name.to_owned()))?;
        let record = &mut provenance.plugins[index];
        let was_enabled = record.enabled;
        let checked = self.check(record);
        let revocation = enforce(record, &checked);
        if enabled && !trust_is_valid(record, &checked) {
            let error = match checked.integrity {
                Integrity::Intact => PluginError::NotTrusted {
                    name: name.to_owned(),
                },
                _ => checked.integrity_error(name),
            };
            self.commit_revocations(&provenance, vec![(name.to_owned(), revocation)])?;
            return Err(error);
        }
        record.enabled = enabled;
        let record = record.clone();
        if record.enabled != was_enabled || revocation.changed {
            self.save(&provenance)?;
        }
        let summary = self.detail(&record, &checked).summary;
        if record.enabled != was_enabled {
            self.emit(name, record.enabled)?;
        }
        Ok(summary)
    }

    /// Remove a plugin's files and its provenance record (also for an unregistered
    /// directory of that name).
    pub fn uninstall(&self, name: &str) -> Result<Uninstalled, PluginError> {
        validate_name(name).map_err(PluginError::InvalidRequest)?;
        let _guard = self.guard();
        let mut provenance = self.load()?;
        let index = provenance.position(name);
        let directory = self.plugin_dir(name);
        let exists = match fs::symlink_metadata(&directory) {
            Ok(_) => true,
            Err(error) if error.kind() == ErrorKind::NotFound => false,
            Err(error) => return Err(PluginError::io("inspect", &directory, &error)),
        };
        if index.is_none() && !exists {
            return Err(PluginError::NotFound(name.to_owned()));
        }
        let scratch = if exists {
            let tmp = self.ensure_dir(&self.paths.tmp())?;
            let scratch = ScratchDir::create(&tmp, "plugin-remove")?;
            // Renaming moves a link itself, never its target.
            rename_retry(&directory, &scratch.path().join("plugin"))
                .map_err(|error| PluginError::io("remove", &directory, &error))?;
            Some(scratch)
        } else {
            None
        };
        let removed = index.map(|index| provenance.plugins.remove(index));
        if removed.is_some()
            && let Err(error) = self.save(&provenance)
        {
            if let Some(scratch) = &scratch {
                let _ = rename_retry(&scratch.path().join("plugin"), &directory);
            }
            return Err(error);
        }
        drop(scratch);
        let was_enabled = removed.is_some_and(|record| record.enabled);
        if was_enabled {
            self.emit(name, false)?;
        }
        Ok(Uninstalled {
            name: name.to_owned(),
            removed_files: exists,
            was_enabled,
        })
    }

    /// The skills and MCP files of every enabled plugin whose current tree the user
    /// trusted. Enabled plugins that fail verification are disabled, lose their trust,
    /// and contribute nothing; a plugin whose files cannot be read this time keeps its
    /// trust and stays enabled, but contributes nothing until a check verifies it again.
    pub fn enabled_contributions(&self) -> Result<Vec<PluginContribution>, PluginError> {
        let _guard = self.guard();
        let mut provenance = self.load()?;
        let mut revocations = Vec::new();
        let mut contributions = Vec::new();
        for record in provenance
            .plugins
            .iter_mut()
            .filter(|record| record.enabled)
        {
            let checked = self.check(record);
            revocations.push((record.name.clone(), enforce(record, &checked)));
            // Only files this check proved intact and trusted contribute: unreadable
            // ones keep their trust (see `enforce`) but contribute nothing now.
            if !record.enabled || !trust_is_valid(record, &checked) {
                continue;
            }
            let root = self.plugin_dir(&record.name);
            let skills_dir = root.join(SKILLS_DIR);
            let mcp_json = root.join(MCP_JSON);
            contributions.push(PluginContribution {
                name: record.name.clone(),
                skills_dir: is_real(&skills_dir, true).then_some(skills_dir),
                mcp_json: is_real(&mcp_json, false).then_some(mcp_json),
                root,
            });
        }
        self.commit_revocations(&provenance, revocations)?;
        contributions.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(contributions)
    }

    fn guard(&self) -> MutexGuard<'_, ()> {
        self.lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn load(&self) -> Result<Provenance, PluginError> {
        provenance::load(&self.provenance_path())
    }

    fn save(&self, provenance: &Provenance) -> Result<(), PluginError> {
        let dir = self.ensure_dir(&self.paths.config())?;
        provenance::save(&dir.join(PROVENANCE_FILE), provenance)
    }

    fn ensure_dir(&self, dir: &Path) -> Result<PathBuf, PluginError> {
        self.paths.ensure_dir(dir).map_err(|error: DriveError| {
            PluginError::Io(format!("cannot create `{}`: {error}", dir.display()))
        })
    }

    fn plugin_dir(&self, name: &str) -> PathBuf {
        self.paths.plugins().join(name)
    }

    /// Persist revocations (if any) and emit `extension.disabled` for plugins they
    /// disabled. Events here are best effort: the revocation itself is what matters.
    fn commit_revocations(
        &self,
        provenance: &Provenance,
        revocations: Vec<(String, Revocation)>,
    ) -> Result<(), PluginError> {
        if revocations.iter().any(|(_, revocation)| revocation.changed) {
            self.save(provenance)?;
        }
        for (name, revocation) in revocations {
            if revocation.disabled {
                let _ = self.emit(&name, false);
            }
        }
        Ok(())
    }

    fn emit(&self, name: &str, enabled: bool) -> Result<(), PluginError> {
        let Some(events) = &self.events else {
            return Ok(());
        };
        let extension_id = EntityId::new(name)
            .map_err(|error| PluginError::EventNotRecorded(error.to_string()))?;
        let kind = ExtensionKind::Plugin;
        let event = if enabled {
            EventKind::ExtensionEnabled { extension_id, kind }
        } else {
            EventKind::ExtensionDisabled { extension_id, kind }
        };
        events
            .emit(EventScope::global(), event)
            .map(|_| ())
            .map_err(|error| PluginError::EventNotRecorded(error.to_string()))
    }

    /// Re-hash a recorded plugin's directory.
    fn check(&self, record: &PluginRecord) -> Checked {
        let dir = self.plugin_dir(&record.name);
        match fs::symlink_metadata(&dir) {
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Checked::failed(Integrity::Missing, "the plugin files are missing");
            }
            Err(error) => {
                return Checked::unreadable(format!(
                    "cannot inspect the plugin directory: {error}"
                ));
            }
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Checked::failed(
                    Integrity::Invalid,
                    "the plugin directory is not a real directory",
                );
            }
            Ok(_) => {}
        }
        let hashed = tree::scan(&dir).and_then(|tree| {
            let hash = tree::hash(&dir, &tree)?;
            Ok((tree, hash))
        });
        match hashed {
            Ok((tree, hash)) => {
                let intact = hash == record.tree_sha256;
                Checked {
                    integrity: if intact {
                        Integrity::Intact
                    } else {
                        Integrity::Modified
                    },
                    error: (!intact).then(|| {
                        "the plugin files changed since they were installed or last trusted"
                            .to_owned()
                    }),
                    tree: Some(tree),
                    hash: Some(hash),
                    transient: false,
                }
            }
            Err(error @ PluginError::Io(_)) => Checked::unreadable(error.to_string()),
            Err(error) => Checked::failed(Integrity::Invalid, error.to_string()),
        }
    }

    fn detail(&self, record: &PluginRecord, checked: &Checked) -> PluginDetail {
        let root = self.plugin_dir(&record.name);
        let mut error = checked.error.clone();
        let (mut loaded, mut contents) = (None, None);
        if let Some(tree) = &checked.tree {
            match manifest::load(&root) {
                Ok(manifest) => {
                    match contents::inspect(&root, tree, manifest.ignored_fields.clone()) {
                        Ok(inspected) => contents = Some(inspected),
                        Err(inspect_error) => {
                            error.get_or_insert_with(|| inspect_error.to_string());
                        }
                    }
                    loaded = Some(manifest);
                }
                Err(manifest_error) => {
                    error.get_or_insert_with(|| manifest_error.to_string());
                }
            }
        }
        let trusted = trust_is_valid(record, checked);
        let manifest =
            loaded.map(|loaded: LoadedManifest| (loaded.file, redacted(loaded.manifest)));
        PluginDetail {
            summary: PluginSummary {
                name: record.name.clone(),
                version: record.version.clone(),
                description: manifest
                    .as_ref()
                    .map(|(_, manifest)| manifest.description.clone()),
                integrity: checked.integrity,
                tree_sha256: checked.hash.clone(),
                installed_sha256: record.tree_sha256.clone(),
                trusted_sha256: record.trusted_sha256.clone(),
                trusted,
                enabled: record.enabled && trusted,
                installed_at: record.installed_at.clone(),
                source: record.source.clone(),
                location: format!(
                    "{}/{}",
                    self.paths.layout().public_plugins.as_str(),
                    record.name
                ),
                skill_count: contents.as_ref().map_or(0, |c| c.skills.len()),
                mcp_server_count: contents.as_ref().map_or(0, |c| c.mcp_servers.len()),
                error,
            },
            manifest_file: manifest.as_ref().map(|(file, _)| *file),
            manifest: manifest.map(|(_, manifest)| manifest),
            contents,
        }
    }

    /// Directories under `plugins/` without a provenance record.
    fn unregistered(&self, provenance: &Provenance) -> Result<Vec<String>, PluginError> {
        let dir = self.plugins_dir();
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(PluginError::io("list", &dir, &error)),
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| PluginError::io("list", &dir, &error))?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let is_dir_or_link = fs::symlink_metadata(entry.path())
                .is_ok_and(|metadata| metadata.is_dir() || metadata.file_type().is_symlink());
            if is_dir_or_link && !name.starts_with('.') && provenance.position(&name).is_none() {
                names.push(name);
            }
        }
        names.sort();
        names.truncate(MAX_UNREGISTERED);
        Ok(names)
    }
}

/// The checks every installed or newly trusted tree passes: a valid manifest (whose
/// name matches `expected_name`), a valid `.mcp.json`, and no undeclared executables.
fn validate_plugin(
    root: &Path,
    tree: &Tree,
    expected_name: Option<&str>,
) -> Result<(LoadedManifest, Vec<McpServerEntry>), PluginError> {
    let loaded = manifest::load(root)?;
    if let Some(expected) = expected_name
        && loaded.manifest.name != expected
    {
        return Err(PluginError::InvalidManifest(format!(
            "the manifest no longer names the plugin `{expected}`"
        )));
    }
    let servers = contents::read_mcp_json(root)?;
    let declared: Vec<String> = servers
        .iter()
        .filter_map(|server| server.bundled_command.clone())
        .collect();
    executables::check(root, tree, &declared)?;
    Ok((loaded, servers))
}

/// Trust holds only for intact files whose recorded hash is the trusted hash.
fn trust_is_valid(record: &PluginRecord, checked: &Checked) -> bool {
    checked.integrity == Integrity::Intact
        && record.trusted_sha256.as_deref() == Some(record.tree_sha256.as_str())
}

/// Disable a plugin and revoke its trust unless that trust still holds (or the files
/// were merely unreadable this time).
fn enforce(record: &mut PluginRecord, checked: &Checked) -> Revocation {
    let mut revocation = Revocation::default();
    if checked.transient || trust_is_valid(record, checked) {
        return revocation;
    }
    if record.enabled {
        record.enabled = false;
        revocation.changed = true;
        revocation.disabled = true;
    }
    if record.trusted_sha256.is_some() {
        record.trusted_sha256 = None;
        revocation.changed = true;
    }
    revocation
}

/// Free text from the manifest, redacted before it leaves the crate.
fn redacted(mut manifest: PluginManifest) -> PluginManifest {
    let redact = |text: &mut String| *text = redact_text(text).into_owned();
    redact(&mut manifest.description);
    if let Some(homepage) = &mut manifest.homepage {
        redact(homepage);
    }
    if let Some(author) = &mut manifest.author {
        redact(&mut author.name);
        if let Some(url) = &mut author.url {
            redact(url);
        }
    }
    manifest
}

/// The install source as recorded: control characters replaced, bounded.
fn source_text(source: &Path) -> String {
    source
        .display()
        .to_string()
        .chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .take(MAX_SOURCE_TEXT / 4)
        .collect()
}

fn is_real(path: &Path, dir: bool) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| {
        !metadata.file_type().is_symlink()
            && if dir {
                metadata.is_dir()
            } else {
                metadata.is_file()
            }
    })
}
