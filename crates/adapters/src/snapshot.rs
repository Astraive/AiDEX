//! Bounded compact recovery archives and coherent copies of vendor SQLite databases.
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
const MAX_RAW: usize = MAX_STATE / 2;
fn sqlite_copy(source: &Path) -> Result<Zeroizing<Vec<u8>>, RuntimeError> {
    for suffix in ["-wal", "-shm"] {
        let sibling = std::path::PathBuf::from(format!("{}{suffix}", source.to_string_lossy()));
        if fs::symlink_metadata(&sibling).is_ok_and(|m| unsafe_link(&m)) {
            return Err(error(
                "state_corrupt",
                "Native SQLite sidecar contains a link or reparse point",
            ));
        }
    }
    let input =
        rusqlite::Connection::open_with_flags(source, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| {
            error(
                "state_unavailable",
                "Native SQLite recovery source unavailable",
            )
        })?;
    let temp = tempfile::NamedTempFile::new().map_err(|_| {
        error(
            "state_unavailable",
            "Native SQLite recovery staging unavailable",
        )
    })?;
    let mut output = rusqlite::Connection::open(temp.path()).map_err(|_| {
        error(
            "state_unavailable",
            "Native SQLite recovery staging unavailable",
        )
    })?;
    let backup = rusqlite::backup::Backup::new(&input, &mut output).map_err(|_| {
        error(
            "state_unavailable",
            "Native SQLite recovery checkpoint unavailable",
        )
    })?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match backup.step(128).map_err(|_| {
            error(
                "state_unavailable",
                "Native SQLite recovery checkpoint failed",
            )
        })? {
            rusqlite::backup::StepResult::Done => break,
            _ => {
                if Instant::now() >= deadline {
                    return Err(error(
                        "state_unavailable",
                        "Native SQLite recovery checkpoint remained busy",
                    ));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
    drop(backup);
    drop(output);
    if fs::metadata(temp.path())
        .map_err(|_| {
            error(
                "state_unavailable",
                "Native SQLite recovery checkpoint unavailable",
            )
        })?
        .len()
        > MAX_RAW as u64
    {
        return Err(error(
            "state_limit",
            "Native SQLite recovery checkpoint exceeds bounds",
        ));
    }
    fs::read(temp.path()).map(Zeroizing::new).map_err(|_| {
        error(
            "state_unavailable",
            "Native SQLite recovery checkpoint unavailable",
        )
    })
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
fn collect(
    root: &Path,
    dir: &Path,
    files: &mut BTreeMap<String, Zeroizing<Vec<u8>>>,
    backups: &mut BTreeSet<String>,
    size: &mut usize,
    skip_codex_tmp: bool,
    depth: usize,
) -> Result<(), RuntimeError> {
    if depth > 32 {
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
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
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
            if skip_codex_tmp && dir == root && entry.file_name() == ".tmp" {
                continue;
            }
            collect(root, &path, files, backups, size, skip_codex_tmp, depth + 1)?;
        } else if metadata.is_file() {
            let name = relative(root, &path)?;
            if ["-wal", "-shm"].iter().any(|suffix| {
                name.strip_suffix(suffix)
                    .is_some_and(|base| backups.contains(base))
            }) {
                continue;
            }
            if files.len() >= 4096 {
                return Err(error(
                    "state_limit",
                    "Runtime recovery file count exceeds bounds",
                ));
            }
            let mut signature = [0u8; 16];
            let count = fs::File::open(&path)
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
            let bytes =
                if count == 16 && signature == *b"SQLite format 3\0" {
                    let bytes = sqlite_copy(&path)?;
                    backups.insert(name.clone());
                    bytes
                } else {
                    if metadata.len() > MAX_RAW.saturating_sub(*size) as u64 {
                        return Err(error("state_limit", "Runtime recovery exceeds bounds"));
                    }
                    Zeroizing::new(fs::read(&path).map_err(|_| {
                        error("state_unavailable", "Runtime recovery file unavailable")
                    })?)
                };
            *size += bytes.len();
            if *size > MAX_RAW {
                return Err(error("state_limit", "Runtime recovery exceeds bounds"));
            }
            files.insert(name, bytes);
        }
    }
    Ok(())
}
pub(crate) fn capture(
    home: &Path,
    skip_codex_tmp: bool,
    excluded: &[Zeroizing<String>],
) -> Result<Zeroizing<Vec<u8>>, RuntimeError> {
    let mut files = BTreeMap::new();
    let mut size = 0;
    collect(
        home,
        home,
        &mut files,
        &mut BTreeSet::new(),
        &mut size,
        skip_codex_tmp,
        0,
    )?;
    if files.iter().any(|(name, bytes)| {
        excluded.iter().any(|value| {
            !value.is_empty()
                && (name.contains(value.as_str())
                    || bytes
                        .windows(value.len())
                        .any(|window| window == value.as_bytes()))
        })
    }) {
        return Err(error(
            "bridge_state_unsupported",
            "Vendor recovery files contain ephemeral bridge credentials; recovery publication refused",
        ));
    }
    let encoded: BTreeMap<_, _> = files
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
pub(crate) fn decode(plain: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, RuntimeError> {
    let value: serde_json::Value = serde_json::from_slice(plain)
        .map_err(|_| error("state_corrupt", "Runtime recovery archive is invalid"))?;
    if value.get("format").and_then(serde_json::Value::as_str) == Some("base64_v1") {
        let encoded: BTreeMap<String, String> = serde_json::from_value(
            value
                .get("files")
                .cloned()
                .ok_or_else(|| error("state_corrupt", "Runtime recovery files are missing"))?,
        )
        .map_err(|_| error("state_corrupt", "Runtime recovery files are invalid"))?;
        if encoded.len() > 4096 {
            return Err(error(
                "state_limit",
                "Runtime recovery file count exceeds bounds",
            ));
        }
        let mut total = 0;
        encoded
            .into_iter()
            .map(|(name, text)| {
                let bytes = STANDARD
                    .decode(text)
                    .map_err(|_| error("state_corrupt", "Runtime recovery encoding is invalid"))?;
                total += bytes.len();
                if total > MAX_RAW {
                    return Err(error("state_limit", "Runtime recovery exceeds bounds"));
                }
                Ok((name, bytes))
            })
            .collect()
    } else {
        serde_json::from_value(value).map_err(|_| {
            error(
                "state_corrupt",
                "Legacy runtime recovery archive is invalid",
            )
        })
    }
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
        let captured = capture(root.path(), true, &[]).unwrap();
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
}
