//! Host-residue check: list well-known per-user directories before and after the
//! smoke run and report entries that appeared.
//!
//! This is a top-level listing, not a full filesystem audit: it catches new files or
//! directories directly inside the home directory and the per-user config, data,
//! cache, and state roots, plus AiDEX-named entries in the system temp directory
//! (other processes create unrelated temp files concurrently). Writes deeper inside
//! pre-existing directories and changes to existing entries are not detected.
//! macOS Preferences and Application Support exclude a finite list of exact
//! OS-maintained entry names with their expected file types; other entries there
//! remain fully watched. This audit does not attribute entries to a process.

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
};

use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Watch {
    /// Any new entry is residue.
    All,
    /// Only new entries whose name contains `aidex` (case-insensitive).
    AidexNamed,
    /// All new entries except known OS-maintained regular plists.
    MacosPreferences,
    /// All new entries except known OS-maintained application data directories.
    MacosApplicationSupport,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EntryKind {
    RegularFile,
    Directory,
    /// Symlinks, special files, and entries whose file type could not be read.
    Other,
}

impl Watch {
    fn is_known_os_entry(self, name: &OsStr, kind: EntryKind) -> bool {
        matches!(
            (self, kind, name.to_str()),
            (
                Self::MacosPreferences,
                EntryKind::RegularFile,
                Some(
                    "com.apple.siri.ODDI.ExperimentationWorker.plist"
                        | "com.apple.GenerativeFunctions.GenerativeFunctionsInstrumentation.plist"
                        | "com.apple.appleintelligencereporting.plist"
                ),
            ) | (
                Self::MacosApplicationSupport,
                EntryKind::Directory,
                Some(
                    "com.apple.RemoteManagementAgent"
                        | "com.apple.appleintelligencereporting.processing"
                ),
            )
        )
    }
}

/// Entry names mapped to their file type at snapshot time.
type Entries = BTreeMap<OsString, EntryKind>;

#[derive(Debug)]
pub struct Snapshot {
    dirs: Vec<(PathBuf, Watch, Option<Entries>)>,
}

/// Directories to watch on this host.
pub fn watched() -> Vec<(PathBuf, Watch)> {
    let var = |name: &str| {
        env::var_os(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let home = if cfg!(windows) {
        var("USERPROFILE")
    } else {
        var("HOME")
    };
    let mut dirs = Vec::new();
    if let Some(home) = &home {
        dirs.push((home.clone(), Watch::All));
    }
    if cfg!(windows) {
        for name in ["APPDATA", "LOCALAPPDATA"] {
            if let Some(dir) = var(name) {
                dirs.push((dir, Watch::All));
            }
        }
    } else if let Some(home) = &home {
        let xdg = |name: &str, default: &str| var(name).unwrap_or_else(|| home.join(default));
        if cfg!(target_os = "macos") {
            for sub in ["Application Support", "Caches", "Preferences", "Logs"] {
                let watch = match sub {
                    "Preferences" => Watch::MacosPreferences,
                    "Application Support" => Watch::MacosApplicationSupport,
                    _ => Watch::All,
                };
                dirs.push((home.join("Library").join(sub), watch));
            }
        } else {
            dirs.push((xdg("XDG_CONFIG_HOME", ".config"), Watch::All));
            dirs.push((xdg("XDG_DATA_HOME", ".local/share"), Watch::All));
            dirs.push((xdg("XDG_STATE_HOME", ".local/state"), Watch::All));
            dirs.push((xdg("XDG_CACHE_HOME", ".cache"), Watch::All));
        }
    }
    dirs.push((env::temp_dir(), Watch::AidexNamed));
    let mut seen = BTreeSet::new();
    dirs.retain(|(dir, _)| seen.insert(dir.clone()));
    dirs
}

impl Snapshot {
    pub fn take(dirs: &[(PathBuf, Watch)]) -> Self {
        Self {
            dirs: dirs
                .iter()
                .map(|(dir, watch)| (dir.clone(), *watch, list(dir)))
                .collect(),
        }
    }

    pub fn dirs(&self) -> Vec<PathBuf> {
        self.dirs.iter().map(|(dir, _, _)| dir.clone()).collect()
    }

    /// Entries present in `after` but not in `self`, filtered per directory.
    pub fn new_entries(&self, after: &Self) -> Vec<PathBuf> {
        let mut found = BTreeSet::new();
        for ((dir, watch, before), (_, _, now)) in self.dirs.iter().zip(&after.dirs) {
            let Some(now) = now else { continue };
            let Some(before) = before else {
                // The directory itself appeared.
                if *watch != Watch::AidexNamed {
                    found.insert(dir.clone());
                }
                continue;
            };
            for (name, kind) in now {
                if before.contains_key(name) {
                    continue;
                }
                // Background OS services can create these entries during smoke.
                // Match the directory policy, exact name, and snapshot file type
                // without following symlinks or exempting a namespace prefix.
                let os_entry = watch.is_known_os_entry(name, *kind);
                let named = name
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .contains("aidex");
                if (*watch != Watch::AidexNamed || named) && !os_entry {
                    found.insert(dir.join(name));
                }
            }
        }
        found.into_iter().collect()
    }
}

/// Entry names and their file type at snapshot time. Symlinks, special files, and
/// unknown types are never eligible for an OS-maintained entry exception.
fn list(dir: &Path) -> Option<Entries> {
    let entries = fs::read_dir(dir).ok()?;
    Some(
        entries
            .flatten()
            .map(|entry| {
                let kind = match entry.file_type() {
                    Ok(kind) if kind.is_file() => EntryKind::RegularFile,
                    Ok(kind) if kind.is_dir() => EntryKind::Directory,
                    _ => EntryKind::Other,
                };
                (entry.file_name(), kind)
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_preferences_background_churn_does_not_hide_host_writes() {
        let host = tempfile::tempdir().unwrap();
        let preferences = host.path().join("Library/Preferences");
        fs::create_dir_all(&preferences).unwrap();
        let dirs = vec![(preferences.clone(), Watch::MacosPreferences)];
        let before = Snapshot::take(&dirs);

        // A separate background writer reproduces the runner's Siri preference
        // appearing between snapshots; joining makes the fixture deterministic.
        let background_preferences = preferences.clone();
        let background = std::thread::spawn(move || {
            for name in [
                "com.apple.siri.ODDI.ExperimentationWorker.plist",
                "com.apple.GenerativeFunctions.GenerativeFunctionsInstrumentation.plist",
                "com.apple.appleintelligencereporting.plist",
            ] {
                fs::write(background_preferences.join(name), "OS-owned preference").unwrap();
            }
        });
        for name in [
            "unexpected.plist",
            "com.apple.unexpected.plist",
            "AiDEX.plist",
            "com.apple.siri.ODDI.ExperimentationWorker.plist.bak",
            "com.apple.siri.ODDI.ExperimentationWorker-AiDEX.plist",
        ] {
            fs::write(preferences.join(name), "unexpected host write").unwrap();
        }
        fs::create_dir(preferences.join("AiDEX-data")).unwrap();
        background.join().unwrap();

        let mut expected = vec![
            preferences.join("unexpected.plist"),
            preferences.join("com.apple.unexpected.plist"),
            preferences.join("AiDEX.plist"),
            preferences.join("com.apple.siri.ODDI.ExperimentationWorker.plist.bak"),
            preferences.join("com.apple.siri.ODDI.ExperimentationWorker-AiDEX.plist"),
            preferences.join("AiDEX-data"),
        ];
        expected.sort();
        assert_eq!(before.new_entries(&Snapshot::take(&dirs)), expected);
    }

    #[test]
    fn macos_application_support_background_churn_does_not_hide_host_writes() {
        let host = tempfile::tempdir().unwrap();
        let support = host.path().join("Library/Application Support");
        fs::create_dir_all(&support).unwrap();
        let dirs = vec![(support.clone(), Watch::MacosApplicationSupport)];
        let before = Snapshot::take(&dirs);
        let background_support = support.clone();
        let background = std::thread::spawn(move || {
            for name in [
                "com.apple.RemoteManagementAgent",
                "com.apple.appleintelligencereporting.processing",
            ] {
                fs::create_dir(background_support.join(name)).unwrap();
            }
        });
        for name in [
            "unexpected-data",
            "com.apple.unexpected",
            "AiDEX-data",
            "com.apple.RemoteManagementAgent.bak",
            "com.apple.appleintelligencereporting.processing-AiDEX",
        ] {
            fs::create_dir(support.join(name)).unwrap();
        }
        fs::write(support.join("unexpected-file"), "host write").unwrap();
        background.join().unwrap();
        let mut expected = vec![
            support.join("unexpected-data"),
            support.join("com.apple.unexpected"),
            support.join("AiDEX-data"),
            support.join("com.apple.RemoteManagementAgent.bak"),
            support.join("com.apple.appleintelligencereporting.processing-AiDEX"),
            support.join("unexpected-file"),
        ];
        expected.sort();
        assert_eq!(before.new_entries(&Snapshot::take(&dirs)), expected);
    }

    #[test]
    fn macos_os_entry_names_are_audited_with_wrong_types_or_directory_policies() {
        let cases = [
            (
                Watch::MacosPreferences,
                Watch::MacosApplicationSupport,
                &[
                    "com.apple.siri.ODDI.ExperimentationWorker.plist",
                    "com.apple.GenerativeFunctions.GenerativeFunctionsInstrumentation.plist",
                    "com.apple.appleintelligencereporting.plist",
                ][..],
                false,
            ),
            (
                Watch::MacosApplicationSupport,
                Watch::MacosPreferences,
                &[
                    "com.apple.RemoteManagementAgent",
                    "com.apple.appleintelligencereporting.processing",
                ][..],
                true,
            ),
        ];
        for (policy, other_policy, names, directory) in cases {
            for name in names {
                // Even the exact name is residue when its type is wrong, when
                // it appears under the other macOS policy, or under Watch::All.
                for (watch, is_directory) in [
                    (policy, !directory),
                    (other_policy, directory),
                    (Watch::All, directory),
                ] {
                    let root = tempfile::tempdir().unwrap();
                    let dirs = vec![(root.path().to_path_buf(), watch)];
                    let before = Snapshot::take(&dirs);
                    let unexpected = root.path().join(name);
                    if is_directory {
                        fs::create_dir(&unexpected).unwrap();
                    } else {
                        fs::write(&unexpected, "unexpected host write").unwrap();
                    }
                    assert_eq!(before.new_entries(&Snapshot::take(&dirs)), vec![unexpected]);
                }
            }
        }
    }

    #[test]
    fn unknown_file_types_are_not_excluded_for_known_macos_entry_names() {
        for watch in [Watch::MacosPreferences, Watch::MacosApplicationSupport] {
            for name in [
                "com.apple.siri.ODDI.ExperimentationWorker.plist",
                "com.apple.GenerativeFunctions.GenerativeFunctionsInstrumentation.plist",
                "com.apple.appleintelligencereporting.plist",
                "com.apple.RemoteManagementAgent",
                "com.apple.appleintelligencereporting.processing",
            ] {
                assert!(!watch.is_known_os_entry(OsStr::new(name), EntryKind::Other));
            }
        }
    }

    #[test]
    fn differently_cased_macos_entry_names_are_audited() {
        for (watch, name, directory) in [
            (
                Watch::MacosPreferences,
                "Com.apple.siri.ODDI.ExperimentationWorker.plist",
                false,
            ),
            (
                Watch::MacosPreferences,
                "Com.apple.GenerativeFunctions.GenerativeFunctionsInstrumentation.plist",
                false,
            ),
            (
                Watch::MacosPreferences,
                "Com.apple.appleintelligencereporting.plist",
                false,
            ),
            (
                Watch::MacosApplicationSupport,
                "Com.apple.RemoteManagementAgent",
                true,
            ),
            (
                Watch::MacosApplicationSupport,
                "Com.apple.appleintelligencereporting.processing",
                true,
            ),
        ] {
            let root = tempfile::tempdir().unwrap();
            let dirs = vec![(root.path().to_path_buf(), watch)];
            let before = Snapshot::take(&dirs);
            // Each case is separate from its OS-owned filename fixture, so
            // case-insensitive hosts do not alias the two names.
            let unexpected = root.path().join(name);
            if directory {
                fs::create_dir(&unexpected).unwrap();
            } else {
                fs::write(&unexpected, "unexpected host write").unwrap();
            }
            assert_eq!(before.new_entries(&Snapshot::take(&dirs)), vec![unexpected]);
        }
    }

    #[test]
    fn known_macos_preference_name_is_audited_in_other_directories() {
        let host = tempfile::tempdir().unwrap();
        let cache = host.path().join("Library/Caches");
        fs::create_dir_all(&cache).unwrap();
        let dirs = vec![
            (host.path().to_path_buf(), Watch::All),
            (cache.clone(), Watch::All),
        ];
        let before = Snapshot::take(&dirs);
        let name = "com.apple.siri.ODDI.ExperimentationWorker.plist";
        fs::write(host.path().join(name), "unexpected host write").unwrap();
        fs::write(cache.join(name), "unexpected cache write").unwrap();
        let mut expected = vec![host.path().join(name), cache.join(name)];
        expected.sort();
        assert_eq!(before.new_entries(&Snapshot::take(&dirs)), expected);
    }

    #[test]
    fn known_macos_preference_name_does_not_exclude_a_directory() {
        let preferences = tempfile::tempdir().unwrap();
        let dirs = vec![(preferences.path().to_path_buf(), Watch::MacosPreferences)];
        let before = Snapshot::take(&dirs);
        let unexpected = preferences
            .path()
            .join("com.apple.siri.ODDI.ExperimentationWorker.plist");
        fs::create_dir(&unexpected).unwrap();
        assert_eq!(before.new_entries(&Snapshot::take(&dirs)), vec![unexpected]);
    }

    #[test]
    fn macos_preference_file_type_is_captured_in_the_snapshot() {
        let preferences = tempfile::tempdir().unwrap();
        let dirs = vec![(preferences.path().to_path_buf(), Watch::MacosPreferences)];
        let before = Snapshot::take(&dirs);
        let unexpected = preferences
            .path()
            .join("com.apple.siri.ODDI.ExperimentationWorker.plist");
        fs::create_dir(&unexpected).unwrap();
        let after = Snapshot::take(&dirs);
        fs::remove_dir(&unexpected).unwrap();
        fs::write(
            &unexpected,
            "regular file replacing an unexpected directory",
        )
        .unwrap();
        assert_eq!(before.new_entries(&after), vec![unexpected]);
    }

    #[test]
    fn macos_application_support_file_type_is_captured_in_the_snapshot() {
        for was_directory in [false, true] {
            let support = tempfile::tempdir().unwrap();
            let dirs = vec![(support.path().to_path_buf(), Watch::MacosApplicationSupport)];
            let before = Snapshot::take(&dirs);
            let entry = support.path().join("com.apple.RemoteManagementAgent");
            if was_directory {
                fs::create_dir(&entry).unwrap();
            } else {
                fs::write(&entry, "unexpected regular file").unwrap();
            }
            let after = Snapshot::take(&dirs);
            if was_directory {
                fs::remove_dir(&entry).unwrap();
                fs::write(&entry, "regular file replacing an OS directory").unwrap();
            } else {
                fs::remove_file(&entry).unwrap();
                fs::create_dir(&entry).unwrap();
            }
            let expected = if was_directory { vec![] } else { vec![entry] };
            assert_eq!(before.new_entries(&after), expected);
        }
    }

    #[test]
    fn newly_created_macos_watched_directory_is_audited() {
        for (sub, watch) in [
            ("Preferences", Watch::MacosPreferences),
            ("Application Support", Watch::MacosApplicationSupport),
        ] {
            let host = tempfile::tempdir().unwrap();
            let root = host.path().join("Library").join(sub);
            let dirs = vec![(root.clone(), watch)];
            let before = Snapshot::take(&dirs);
            fs::create_dir_all(&root).unwrap();
            assert_eq!(before.new_entries(&Snapshot::take(&dirs)), vec![root]);
        }
    }

    #[cfg(unix)]
    #[test]
    fn known_macos_entry_names_do_not_exclude_symlinks() {
        for (watch, name) in [
            (
                Watch::MacosPreferences,
                "com.apple.siri.ODDI.ExperimentationWorker.plist",
            ),
            (
                Watch::MacosPreferences,
                "com.apple.GenerativeFunctions.GenerativeFunctionsInstrumentation.plist",
            ),
            (
                Watch::MacosPreferences,
                "com.apple.appleintelligencereporting.plist",
            ),
            (
                Watch::MacosApplicationSupport,
                "com.apple.RemoteManagementAgent",
            ),
            (
                Watch::MacosApplicationSupport,
                "com.apple.appleintelligencereporting.processing",
            ),
        ] {
            // A regular-file target, directory target, and dangling link must
            // all be classified as symlinks rather than as their target's type.
            for target_kind in [
                EntryKind::RegularFile,
                EntryKind::Directory,
                EntryKind::Other,
            ] {
                let root = tempfile::tempdir().unwrap();
                let target = root.path().join("existing");
                match target_kind {
                    EntryKind::RegularFile => fs::write(&target, "existing file").unwrap(),
                    EntryKind::Directory => fs::create_dir(&target).unwrap(),
                    EntryKind::Other => {}
                }
                let dirs = vec![(root.path().to_path_buf(), watch)];
                let before = Snapshot::take(&dirs);
                let unexpected = root.path().join(name);
                std::os::unix::fs::symlink(target, &unexpected).unwrap();
                assert_eq!(before.new_entries(&Snapshot::take(&dirs)), vec![unexpected]);
            }
        }
    }

    #[test]
    fn new_entries_respect_the_watch_mode() {
        let all = tempfile::tempdir().unwrap();
        let named = tempfile::tempdir().unwrap();
        fs::write(all.path().join("existing"), "").unwrap();
        let dirs = vec![
            (all.path().to_path_buf(), Watch::All),
            (named.path().to_path_buf(), Watch::AidexNamed),
            (all.path().join("later"), Watch::All),
        ];
        let before = Snapshot::take(&dirs);
        fs::write(all.path().join("new-file"), "").unwrap();
        fs::write(named.path().join("unrelated.tmp"), "").unwrap();
        fs::write(named.path().join("AiDEX-residue"), "").unwrap();
        fs::create_dir(all.path().join("later")).unwrap();
        let after = Snapshot::take(&dirs);
        let found = before.new_entries(&after);
        let mut expected = vec![
            all.path().join("new-file"),
            all.path().join("later"),
            named.path().join("AiDEX-residue"),
        ];
        expected.sort();
        assert_eq!(found, expected);
        assert!(Snapshot::take(&dirs).new_entries(&after).is_empty());
    }
}
