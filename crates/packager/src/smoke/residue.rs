//! Host-residue check: list well-known per-user directories before and after the
//! smoke run and report entries that appeared.
//!
//! This is a top-level listing, not a full filesystem audit: it catches new files or
//! directories directly inside the home directory and the per-user config, data,
//! cache, and state roots, plus AiDEX-named entries in the system temp directory
//! (other processes create unrelated temp files concurrently). Writes deeper inside
//! pre-existing directories and changes to existing entries are not detected.
//!
//! macOS services (Siri, Apple Intelligence reporting, remote management, managed
//! app distribution, ...) create their own state at any time under Apple's
//! reserved `com.apple.` reverse-DNS namespace. In `~/Library/Preferences`,
//! `~/Library/Application Support`, and `~/Library/Caches` only, new entries in that
//! namespace with the type the OS keeps there are therefore not residue (see
//! [`Watch::is_apple_service_state`]). AiDEX's own identifiers are `dev.aidex.*`;
//! AiDEX-named entries, other types, symlinks, and the same names in any other
//! watched directory remain audited. This audit does not attribute entries to a
//! process.

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
    /// `~/Library/Preferences`: all new entries except regular
    /// `com.apple.<domain>.plist` files.
    MacosPreferences,
    /// `~/Library/Application Support` and `~/Library/Caches`: all new entries
    /// except `com.apple.<domain>` directories.
    MacosAppData,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EntryKind {
    RegularFile,
    Directory,
    /// Symlinks, special files, and entries whose file type could not be read.
    Other,
}

/// Apple's reverse-DNS namespace for OS preference domains and service data.
const APPLE_NAMESPACE: &str = "com.apple.";

impl Watch {
    /// Whether a new entry is macOS service state: a name in Apple's namespace
    /// with the file type the OS keeps in this directory. Never true for
    /// [`Watch::All`], [`Watch::AidexNamed`], symlinks, or non-UTF-8 names.
    fn is_apple_service_state(self, name: &OsStr, kind: EntryKind) -> bool {
        let Some(name) = name.to_str() else {
            return false;
        };
        let domain = match (self, kind) {
            (Self::MacosPreferences, EntryKind::RegularFile) => name.strip_suffix(".plist"),
            (Self::MacosAppData, EntryKind::Directory) => Some(name),
            _ => None,
        };
        domain.is_some_and(is_apple_domain)
    }
}

/// `com.apple.` (case-sensitive, as Apple's services write it) followed by one or
/// more non-empty dot-separated labels of ASCII letters, digits, `-`, and `_`.
fn is_apple_domain(domain: &str) -> bool {
    domain.strip_prefix(APPLE_NAMESPACE).is_some_and(|labels| {
        labels.split('.').all(|label| {
            !label.is_empty()
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
    })
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
            for (sub, watch) in [
                ("Application Support", Watch::MacosAppData),
                ("Caches", Watch::MacosAppData),
                ("Preferences", Watch::MacosPreferences),
                ("Logs", Watch::All),
            ] {
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
                let named = name
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .contains("aidex");
                let residue = match watch {
                    Watch::AidexNamed => named,
                    // Background macOS services write Apple-namespace state on
                    // their own schedule; an AiDEX-named entry is never theirs.
                    _ => named || !watch.is_apple_service_state(name, *kind),
                };
                if residue {
                    found.insert(dir.join(name));
                }
            }
        }
        found.into_iter().collect()
    }
}

/// Entry names and their file type at snapshot time, without following symlinks.
/// Symlinks, special files, and unknown types are never macOS service state.
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

    /// Preference plists macOS services created during package smoke runs on
    /// GitHub's macos-arm64 runners; each one failed a smoke run.
    const OBSERVED_PREFERENCES: [&str; 5] = [
        "com.apple.siri.ODDI.ExperimentationWorker.plist",
        "com.apple.GenerativeFunctions.GenerativeFunctionsInstrumentation.plist",
        "com.apple.appleintelligencereporting.plist",
        "com.apple.managedappdistributionagent.plist",
        "com.apple.remotemanagement.ManagedSettingsSubscriber.plist",
    ];

    /// Directories the same runs saw in Application Support and Caches.
    const OBSERVED_APP_DATA: [&str; 3] = [
        "com.apple.RemoteManagementAgent",
        "com.apple.appleintelligencereporting.processing",
        "com.apple.managedappdistributionagent",
    ];

    #[derive(Clone, Copy)]
    enum Make {
        File,
        Dir,
    }

    fn make(dir: &Path, name: &str, kind: Make) -> PathBuf {
        let path = dir.join(name);
        match kind {
            Make::File => fs::write(&path, "host write").unwrap(),
            Make::Dir => fs::create_dir(&path).unwrap(),
        }
        path
    }

    /// Snapshots `dir` under `watch` while a background writer stands in for
    /// macOS services creating `service` entries and the foreground creates
    /// `audited`. Returns the residue found and the sorted `audited` paths.
    fn churn(
        dir: &Path,
        watch: Watch,
        service: &'static [&'static str],
        service_kind: Make,
        audited: &[(&str, Make)],
    ) -> (Vec<PathBuf>, Vec<PathBuf>) {
        let dirs = vec![(dir.to_path_buf(), watch)];
        let before = Snapshot::take(&dirs);
        let background_dir = dir.to_path_buf();
        let background = std::thread::spawn(move || {
            for name in service {
                make(&background_dir, name, service_kind);
            }
        });
        let mut expected: Vec<_> = audited
            .iter()
            .map(|&(name, kind)| make(dir, name, kind))
            .collect();
        // Joining before the second snapshot makes the fixture deterministic.
        background.join().unwrap();
        expected.sort();
        (before.new_entries(&Snapshot::take(&dirs)), expected)
    }

    #[test]
    fn apple_domains_require_the_exact_namespace_and_label_syntax() {
        for domain in OBSERVED_PREFERENCES
            .iter()
            .map(|name| name.strip_suffix(".plist").unwrap())
            .chain(OBSERVED_APP_DATA)
            .chain([
                "com.apple.knowledge-agent",
                "com.apple.x_y",
                "com.apple.a.b.c",
            ])
        {
            assert!(is_apple_domain(domain), "{domain}");
        }
        for domain in [
            "",
            "com.apple",
            "com.apple.",
            "com.apple..x",
            "com.apple.x.",
            "com.apple.x..y",
            "Com.apple.x",
            "COM.APPLE.X",
            "com.applex.y",
            "com.apple-x.y",
            "xcom.apple.y",
            " com.apple.x",
            "apple.com.x",
            "dev.aidex.desktop",
            "com.apple.x y",
            "com.apple.x/y",
            "com.apple.x\\y",
            "com.apple.x\0",
            "com.apple.\u{e9}",
        ] {
            assert!(!is_apple_domain(domain), "{domain:?}");
        }
    }

    #[test]
    fn macos_preferences_skip_apple_service_plists_and_audit_everything_else() {
        let host = tempfile::tempdir().unwrap();
        let preferences = host.path().join("Library/Preferences");
        fs::create_dir_all(&preferences).unwrap();
        let (found, expected) = churn(
            &preferences,
            Watch::MacosPreferences,
            &OBSERVED_PREFERENCES,
            Make::File,
            &[
                ("unexpected.plist", Make::File),
                ("AiDEX.plist", Make::File),
                ("dev.aidex.desktop.plist", Make::File),
                // AiDEX-named entries are audited even inside Apple's namespace.
                ("com.apple.aidex.plist", Make::File),
                ("com.apple.AIDEX.settings.plist", Make::File),
                (
                    "com.apple.siri.ODDI.ExperimentationWorker-AiDEX.plist",
                    Make::File,
                ),
                // Not a `com.apple.<domain>.plist` name.
                (
                    "com.apple.siri.ODDI.ExperimentationWorker.plist.bak",
                    Make::File,
                ),
                ("com.apple.remotemanagement", Make::File),
                ("com.apple.plist", Make::File),
                ("com.apple..plist", Make::File),
                ("com.apple.with space.plist", Make::File),
                ("Com.apple.MixedCase.plist", Make::File),
                ("com.applex.plist", Make::File),
                // Wrong type for an OS preference.
                ("com.apple.directory.plist", Make::Dir),
                ("AiDEX-data", Make::Dir),
            ],
        );
        assert_eq!(found, expected);
    }

    #[test]
    fn macos_app_data_skips_apple_service_directories_and_audits_everything_else() {
        for sub in ["Application Support", "Caches"] {
            let host = tempfile::tempdir().unwrap();
            let root = host.path().join("Library").join(sub);
            fs::create_dir_all(&root).unwrap();
            let (found, expected) = churn(
                &root,
                Watch::MacosAppData,
                &OBSERVED_APP_DATA,
                Make::Dir,
                &[
                    ("unexpected-data", Make::Dir),
                    ("AiDEX-data", Make::Dir),
                    ("dev.aidex.desktop", Make::Dir),
                    ("com.apple.aidex", Make::Dir),
                    ("com.apple.RemoteManagementAgent-AiDEX", Make::Dir),
                    ("com.apple", Make::Dir),
                    ("com.apple.with space", Make::Dir),
                    ("Com.apple.MixedCase", Make::Dir),
                    // Wrong type for OS service data.
                    ("com.apple.file", Make::File),
                    ("com.apple.preference.plist", Make::File),
                    ("unexpected-file", Make::File),
                ],
            );
            assert_eq!(found, expected, "{sub}");
        }
    }

    #[test]
    fn apple_service_names_are_audited_in_every_other_watched_directory() {
        // The home directory, `~/Library/Logs`, and every Windows and Linux root
        // use `Watch::All`; the two macOS policies audit each other's entries.
        for (watch, plist_kind, data_kind) in [
            (Watch::All, Make::File, Make::Dir),
            (Watch::All, Make::Dir, Make::File),
            (Watch::MacosPreferences, Make::Dir, Make::File),
            (Watch::MacosAppData, Make::File, Make::File),
        ] {
            let root = tempfile::tempdir().unwrap();
            let dirs = vec![(root.path().to_path_buf(), watch)];
            let before = Snapshot::take(&dirs);
            let mut expected = Vec::new();
            for name in OBSERVED_PREFERENCES {
                expected.push(make(root.path(), name, plist_kind));
            }
            for name in OBSERVED_APP_DATA {
                expected.push(make(root.path(), name, data_kind));
            }
            expected.sort();
            assert_eq!(
                before.new_entries(&Snapshot::take(&dirs)),
                expected,
                "{watch:?}"
            );
        }
    }

    #[test]
    fn apple_service_exceptions_apply_only_to_the_macos_library_directories() {
        let mut policies = Vec::new();
        for (dir, watch) in watched() {
            let in_library = dir
                .parent()
                .is_some_and(|parent| parent.ends_with("Library"));
            let sub = dir.file_name().and_then(OsStr::to_str);
            match watch {
                Watch::MacosPreferences => {
                    assert!(in_library && sub == Some("Preferences"), "{dir:?}");
                }
                Watch::MacosAppData => assert!(
                    in_library && matches!(sub, Some("Application Support" | "Caches")),
                    "{dir:?}"
                ),
                Watch::All | Watch::AidexNamed => continue,
            }
            policies.push(watch);
        }
        if !cfg!(target_os = "macos") {
            assert!(policies.is_empty(), "{policies:?}");
        } else if env::var_os("HOME").is_some_and(|home| !home.is_empty()) {
            assert_eq!(
                policies,
                [
                    Watch::MacosAppData,
                    Watch::MacosAppData,
                    Watch::MacosPreferences
                ]
            );
        }
    }

    #[test]
    fn only_regular_files_and_directories_can_be_apple_service_state() {
        for name in OBSERVED_PREFERENCES.iter().chain(&OBSERVED_APP_DATA) {
            let name = OsStr::new(name);
            for kind in [
                EntryKind::RegularFile,
                EntryKind::Directory,
                EntryKind::Other,
            ] {
                assert!(!Watch::All.is_apple_service_state(name, kind));
                assert!(!Watch::AidexNamed.is_apple_service_state(name, kind));
            }
            assert!(!Watch::MacosPreferences.is_apple_service_state(name, EntryKind::Other));
            assert!(!Watch::MacosAppData.is_apple_service_state(name, EntryKind::Other));
        }
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_names_are_never_apple_service_state() {
        use std::os::unix::ffi::OsStrExt;

        for (watch, kind, name) in [
            (
                Watch::MacosPreferences,
                EntryKind::RegularFile,
                &b"com.apple.\xff.plist"[..],
            ),
            (
                Watch::MacosAppData,
                EntryKind::Directory,
                &b"com.apple.\xff"[..],
            ),
        ] {
            assert!(!watch.is_apple_service_state(OsStr::from_bytes(name), kind));
        }
    }

    #[test]
    fn macos_preference_file_type_is_captured_in_the_snapshot() {
        let preferences = tempfile::tempdir().unwrap();
        let dirs = vec![(preferences.path().to_path_buf(), Watch::MacosPreferences)];
        let before = Snapshot::take(&dirs);
        let unexpected = preferences.path().join(OBSERVED_PREFERENCES[4]);
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
    fn macos_app_data_file_type_is_captured_in_the_snapshot() {
        for was_directory in [false, true] {
            let support = tempfile::tempdir().unwrap();
            let dirs = vec![(support.path().to_path_buf(), Watch::MacosAppData)];
            let before = Snapshot::take(&dirs);
            let entry = support.path().join(OBSERVED_APP_DATA[0]);
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
            ("Application Support", Watch::MacosAppData),
            ("Caches", Watch::MacosAppData),
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
    fn apple_service_names_do_not_exclude_symlinks() {
        for (watch, name) in [
            (Watch::MacosPreferences, OBSERVED_PREFERENCES[4]),
            (Watch::MacosAppData, OBSERVED_APP_DATA[0]),
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
        fs::write(named.path().join("com.apple.unrelated"), "").unwrap();
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
