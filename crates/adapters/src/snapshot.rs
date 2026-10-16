//! Bounded compact recovery archives of vendor homes, with coherent copies of
//! vendor SQLite databases. Account profile records keep the legacy encoding.
use crate::state::{MAX_STATE, error, unsafe_link};
use aidex_native_sessions::RuntimeError;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::Path,
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

/// Aggregate raw bytes one recovery archive may hold.
pub(crate) const MAX_RAW: usize = MAX_STATE / 2;
/// Files one recovery archive may hold.
pub(crate) const MAX_FILES: usize = 4096;
/// Directory depth below the vendor home.
pub(crate) const MAX_DEPTH: usize = 32;
/// How long a coherent copy of one vendor SQLite database may wait for locks.
pub(crate) const SQLITE_DEADLINE: Duration = Duration::from_secs(3);
/// Sidecars a coherent database copy replaces; restoring a stale one beside the
/// copy could roll the copy back.
const SQLITE_SIDECARS: [&str; 3] = ["-wal", "-shm", "-journal"];

/// What one capture leaves out and how long it waits for vendor databases.
#[derive(Clone, Copy)]
pub(crate) struct Capture<'a> {
    /// Entry names skipped at the top of the vendor home only.
    pub skip: &'a [&'a str],
    pub sqlite_deadline: Duration,
}
impl<'a> Capture<'a> {
    pub fn skipping(skip: &'a [&'a str]) -> Self {
        Self {
            skip,
            sqlite_deadline: SQLITE_DEADLINE,
        }
    }
}

fn sqlite_copy(
    source: &Path,
    budget: usize,
    deadline: Duration,
) -> Result<Zeroizing<Vec<u8>>, RuntimeError> {
    for suffix in SQLITE_SIDECARS {
        let sibling = std::path::PathBuf::from(format!("{}{suffix}", source.to_string_lossy()));
        if fs::symlink_metadata(&sibling).is_ok_and(|m| unsafe_link(&m)) {
            return Err(error(
                "state_corrupt",
                "Native SQLite sidecar contains a link or reparse point",
            ));
        }
    }
    // A backup copies every page of the main file, so a larger file cannot fit.
    let unavailable = || {
        error(
            "state_unavailable",
            "Native SQLite recovery checkpoint unavailable",
        )
    };
    if fs::metadata(source).map_err(|_| unavailable())?.len() > budget as u64 {
        return Err(error(
            "state_limit",
            "Native SQLite recovery checkpoint exceeds bounds",
        ));
    }
    let input =
        rusqlite::Connection::open_with_flags(source, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| unavailable())?;
    let _ = input.busy_timeout(Duration::from_millis(50));
    let temp = tempfile::NamedTempFile::new().map_err(|_| unavailable())?;
    let mut output = rusqlite::Connection::open(temp.path()).map_err(|_| unavailable())?;
    let backup = rusqlite::backup::Backup::new(&input, &mut output).map_err(|_| unavailable())?;
    let deadline = Instant::now() + deadline;
    loop {
        let step = backup.step(128).map_err(|_| {
            error(
                "state_unavailable",
                "Native SQLite recovery checkpoint failed",
            )
        })?;
        if step == rusqlite::backup::StepResult::Done {
            break;
        }
        if Instant::now() >= deadline {
            // Nothing is published: the staged copy is discarded with `temp`.
            return Err(error(
                "state_busy",
                "Native SQLite recovery checkpoint remained busy",
            ));
        }
        if step != rusqlite::backup::StepResult::More {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    drop(backup);
    drop(output);
    if fs::metadata(temp.path()).map_err(|_| unavailable())?.len() > budget as u64 {
        return Err(error(
            "state_limit",
            "Native SQLite recovery checkpoint exceeds bounds",
        ));
    }
    fs::read(temp.path())
        .map(Zeroizing::new)
        .map_err(|_| unavailable())
}
fn relative(root: &Path, path: &Path) -> Result<String, RuntimeError> {
    path.strip_prefix(root)
        .map_err(|_| error("state_corrupt", "Runtime recovery path escaped its home"))?
        .components()
        .map(|c| {
            c.as_os_str()
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| error("state_corrupt", "Runtime recovery path is not UTF-8"))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|v| v.join("/"))
}
struct Collector<'a> {
    root: &'a Path,
    capture: Capture<'a>,
    files: BTreeMap<String, Zeroizing<Vec<u8>>>,
    databases: BTreeSet<String>,
    size: usize,
}
impl Collector<'_> {
    fn collect(&mut self, dir: &Path, depth: usize) -> Result<(), RuntimeError> {
        if depth > MAX_DEPTH {
            return Err(error(
                "state_limit",
                "Runtime recovery directory depth exceeds bounds",
            ));
        }
        let mut entries = fs::read_dir(dir)
            .map_err(|_| {
                error(
                    "state_unavailable",
                    "Runtime recovery directory unavailable",
                )
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| error("state_unavailable", "Runtime recovery entry unavailable"))?;
        // Sorted, a database precedes its `-wal`/`-shm`/`-journal` sidecars.
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            if dir == self.root
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| self.capture.skip.contains(&name))
            {
                continue;
            }
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|_| error("state_unavailable", "Runtime recovery metadata unavailable"))?;
            if unsafe_link(&metadata) {
                return Err(error(
                    "state_corrupt",
                    "Runtime recovery contains a link or reparse point",
                ));
            }
            if metadata.is_dir() {
                self.collect(&path, depth + 1)?;
            } else if metadata.is_file() {
                self.file(&path, &metadata)?;
            }
        }
        Ok(())
    }
    fn file(&mut self, path: &Path, metadata: &fs::Metadata) -> Result<(), RuntimeError> {
        let name = relative(self.root, path)?;
        if SQLITE_SIDECARS.iter().any(|suffix| {
            name.strip_suffix(suffix)
                .is_some_and(|base| self.databases.contains(base))
        }) {
            return Ok(());
        }
        if self.files.len() >= MAX_FILES {
            return Err(error(
                "state_limit",
                "Runtime recovery file count exceeds bounds",
            ));
        }
        let budget = MAX_RAW.saturating_sub(self.size);
        let mut signature = [0u8; 16];
        let count = fs::File::open(path)
            .and_then(|mut f| f.read(&mut signature))
            .map_err(|e| {
                error(
                    "state_unavailable",
                    &format!(
                        "Runtime recovery file unavailable ({:?}, OS {:?})",
                        e.kind(),
                        e.raw_os_error()
                    ),
                )
            })?;
        let bytes = if count == 16 && signature == *b"SQLite format 3\0" {
            let bytes = sqlite_copy(path, budget, self.capture.sqlite_deadline)?;
            self.databases.insert(name.clone());
            bytes
        } else {
            if metadata.len() > budget as u64 {
                return Err(error("state_limit", "Runtime recovery exceeds bounds"));
            }
            Zeroizing::new(
                fs::read(path)
                    .map_err(|_| error("state_unavailable", "Runtime recovery file unavailable"))?,
            )
        };
        self.size += bytes.len();
        if self.size > MAX_RAW {
            return Err(error("state_limit", "Runtime recovery exceeds bounds"));
        }
        self.files.insert(name, bytes);
        Ok(())
    }
}
/// Archive `home` as `{"format":"base64_v1","files":{path: base64}}`: at most
/// [`MAX_RAW`] raw bytes, [`MAX_FILES`] files and [`MAX_DEPTH`] levels, with no
/// links or reparse points, and SQLite databases copied through the backup API.
pub(crate) fn capture(
    home: &Path,
    capture: Capture<'_>,
) -> Result<Zeroizing<Vec<u8>>, RuntimeError> {
    let mut collector = Collector {
        root: home,
        capture,
        files: BTreeMap::new(),
        databases: BTreeSet::new(),
        size: 0,
    };
    collector.collect(home, 0)?;
    let encoded: BTreeMap<_, _> = collector
        .files
        .into_iter()
        .map(|(name, bytes)| (name, Zeroizing::new(STANDARD.encode(&*bytes))))
        .collect();
    #[derive(Serialize)]
    struct Archive<'a> {
        format: &'static str,
        files: BTreeMap<&'a str, &'a str>,
    }
    let archive = Archive {
        format: "base64_v1",
        files: encoded
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.as_str()))
            .collect(),
    };
    let plain = Zeroizing::new(serde_json::to_vec(&archive).map_err(|_| {
        error(
            "state_corrupt",
            "Runtime recovery archive cannot be encoded",
        )
    })?);
    if plain.len() > MAX_STATE {
        return Err(error(
            "state_limit",
            "Runtime recovery archive exceeds bounds",
        ));
    }
    Ok(plain)
}
/// The archive format daemons before `base64_v1` read and write: a JSON map of
/// path to byte array. Account profile records keep it so an older daemon can
/// still restore them.
pub(crate) fn encode_legacy(
    files: &BTreeMap<String, Zeroizing<Vec<u8>>>,
) -> Result<Zeroizing<Vec<u8>>, RuntimeError> {
    let plain: BTreeMap<&str, &[u8]> = files
        .iter()
        .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
        .collect();
    serde_json::to_vec(&plain)
        .map(Zeroizing::new)
        .map_err(|_| error("state_corrupt", "Runtime record cannot be encoded"))
}
/// Decode either archive format.
pub(crate) fn decode(plain: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, RuntimeError> {
    let value: serde_json::Value = serde_json::from_slice(plain)
        .map_err(|_| error("state_corrupt", "Runtime recovery archive is invalid"))?;
    let files: BTreeMap<String, Vec<u8>> =
        if value.get("format").and_then(serde_json::Value::as_str) == Some("base64_v1") {
            let encoded: BTreeMap<String, String> = serde_json::from_value(
                value
                    .get("files")
                    .cloned()
                    .ok_or_else(|| error("state_corrupt", "Runtime recovery files are missing"))?,
            )
            .map_err(|_| error("state_corrupt", "Runtime recovery files are invalid"))?;
            if encoded.len() > MAX_FILES {
                return Err(error(
                    "state_limit",
                    "Runtime recovery file count exceeds bounds",
                ));
            }
            encoded
                .into_iter()
                .map(|(name, text)| {
                    STANDARD
                        .decode(text)
                        .map(|bytes| (name, bytes))
                        .map_err(|_| error("state_corrupt", "Runtime recovery encoding is invalid"))
                })
                .collect::<Result<_, _>>()?
        } else {
            serde_json::from_value(value).map_err(|_| {
                error(
                    "state_corrupt",
                    "Legacy runtime recovery archive is invalid",
                )
            })?
        };
    if files.len() > MAX_FILES || files.values().map(Vec::len).sum::<usize>() > MAX_STATE {
        return Err(error("state_limit", "Runtime recovery exceeds bounds"));
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sqlite_wal_is_captured_coherently_and_legacy_archives_still_decode() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("native.sqlite");
        let database = rusqlite::Connection::open(&path).unwrap();
        database.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE turns(content TEXT NOT NULL); INSERT INTO turns VALUES('saved turn');").unwrap();
        assert!(root.path().join("native.sqlite-wal").exists());
        let captured = capture(root.path(), Capture::skipping(&[".tmp"])).unwrap();
        let files = decode(&captured).unwrap();
        assert!(files.contains_key("native.sqlite"));
        assert!(!files.contains_key("native.sqlite-wal"));
        assert!(!files.contains_key("native.sqlite-shm"));
        database
            .execute("UPDATE turns SET content='later turn'", [])
            .unwrap();
        let restored = root.path().join("restored.sqlite");
        fs::write(&restored, &files["native.sqlite"]).unwrap();
        let recovered = rusqlite::Connection::open(restored).unwrap();
        assert_eq!(
            recovered
                .query_row("SELECT content FROM turns", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "saved turn"
        );
        assert_eq!(
            decode(br#"{"legacy.json":[1,2,3]}"#).unwrap()["legacy.json"],
            vec![1, 2, 3]
        );
    }
    #[test]
    fn busy_sqlite_backup_times_out_with_a_closed_code() {
        let root = tempfile::tempdir().unwrap();
        let database = rusqlite::Connection::open(root.path().join("busy.sqlite")).unwrap();
        database
            .execute_batch("CREATE TABLE turns(content TEXT); BEGIN EXCLUSIVE; INSERT INTO turns VALUES('uncommitted');")
            .unwrap();
        let started = Instant::now();
        let refused = capture(
            root.path(),
            Capture {
                skip: &[],
                sqlite_deadline: Duration::from_millis(300),
            },
        )
        .unwrap_err();
        assert_eq!(refused.code, "state_busy");
        assert!(started.elapsed() < Duration::from_secs(3));
        database.execute_batch("COMMIT").unwrap();
        let files = decode(&capture(root.path(), Capture::skipping(&[])).unwrap()).unwrap();
        assert!(files.contains_key("busy.sqlite"));
        assert!(!files.contains_key("busy.sqlite-journal"));
    }
    #[test]
    fn depth_beyond_the_bound_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let mut deep = root.path().to_owned();
        for _ in 0..=MAX_DEPTH + 1 {
            deep.push("d");
        }
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("leaf.txt"), b"leaf").unwrap();
        assert_eq!(
            capture(root.path(), Capture::skipping(&[]))
                .unwrap_err()
                .code,
            "state_limit"
        );
    }
    #[test]
    fn a_link_or_junction_inside_the_home_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let outside = root.path().join("outside");
        fs::create_dir_all(home.join("sessions")).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("host-secret.txt"), b"outside the home").unwrap();
        #[cfg(windows)]
        {
            let status = std::process::Command::new("cmd.exe")
                .args(["/C", "mklink", "/J"])
                .arg(home.join("sessions").join("linked"))
                .arg(&outside)
                .output()
                .unwrap();
            assert!(status.status.success());
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, home.join("sessions").join("linked")).unwrap();
        assert_eq!(
            capture(&home, Capture::skipping(&[])).unwrap_err().code,
            "state_corrupt"
        );
    }
    #[test]
    fn top_level_exclusions_never_skip_nested_entries() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join(".tmp")).unwrap();
        fs::create_dir_all(root.path().join("sessions/.tmp")).unwrap();
        fs::write(root.path().join(".tmp/arg0"), b"generated").unwrap();
        fs::write(root.path().join("sessions/.tmp/kept"), b"kept").unwrap();
        fs::write(root.path().join("auth.json"), b"{}").unwrap();
        let files =
            decode(&capture(root.path(), Capture::skipping(&[".tmp", "auth.json"])).unwrap())
                .unwrap();
        assert_eq!(
            files.keys().map(String::as_str).collect::<Vec<_>>(),
            ["sessions/.tmp/kept"]
        );
    }
}
