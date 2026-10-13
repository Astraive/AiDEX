//! Crash-safe file handling for one vault directory.
//!
//! Writes go to a uniquely named temporary file in the destination directory, are
//! flushed with `fsync`, and then renamed over the target (followed by a directory
//! `fsync` on Unix). Readers ignore temporary files, so an interrupted write leaves the
//! last committed state intact. Symbolic links and junctions are never followed for
//! vault-managed paths, and reads are size-bounded before parsing.

use crate::{
    VaultError,
    crypto::random_array,
    format::{BlobId, HEADER_FILE, MAX_HEADER_BYTES, MAX_RECORD_FILE_BYTES, RECORDS_DIR},
};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, ErrorKind, Read, Write},
    path::{Path, PathBuf},
};

const TEMP_SUFFIX: &str = ".tmp";

/// A validated vault directory (a real directory, not a link).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct VaultDir {
    root: PathBuf,
}

impl VaultDir {
    /// An existing vault directory; a missing directory is [`VaultError::NotFound`].
    pub(crate) fn existing(root: &Path) -> Result<Self, VaultError> {
        match fs::symlink_metadata(root) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(Self {
                root: root.to_path_buf(),
            }),
            Ok(_) => Err(VaultError::UnsafePath(root.to_path_buf())),
            Err(error) if error.kind() == ErrorKind::NotFound => Err(VaultError::NotFound),
            Err(error) => Err(VaultError::io("inspect vault directory", root, error)),
        }
    }

    /// Use `root`, creating it (but not its parents) when missing.
    pub(crate) fn create_or_existing(root: &Path) -> Result<Self, VaultError> {
        match Self::existing(root) {
            Err(VaultError::NotFound) => {
                create_private_dir(root)?;
                Self::existing(root)
            }
            other => other,
        }
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn header_path(&self) -> PathBuf {
        self.root.join(HEADER_FILE)
    }

    fn records_dir(&self) -> PathBuf {
        self.root.join(RECORDS_DIR)
    }

    fn record_path(&self, blob: &BlobId) -> PathBuf {
        self.records_dir().join(blob.file_name())
    }

    /// The committed header bytes, or `None` when there is no header file.
    pub(crate) fn read_header(&self) -> Result<Option<Vec<u8>>, VaultError> {
        read_bounded(&self.header_path(), MAX_HEADER_BYTES, || {
            VaultError::CorruptHeader("header file is too large".into())
        })
    }

    /// A record file, or `None` when it does not exist.
    pub(crate) fn read_record(&self, blob: &BlobId) -> Result<Option<Vec<u8>>, VaultError> {
        read_bounded(&self.record_path(blob), MAX_RECORD_FILE_BYTES, || {
            VaultError::CorruptRecord("record file is too large".into())
        })
    }

    pub(crate) fn write_header(&self, bytes: &[u8]) -> Result<(), VaultError> {
        write_atomic(&self.root, HEADER_FILE, bytes)
    }

    pub(crate) fn write_record(&self, blob: &BlobId, bytes: &[u8]) -> Result<(), VaultError> {
        let records = self.records_dir();
        match fs::symlink_metadata(&records) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => return Err(VaultError::UnsafePath(records)),
            Err(error) if error.kind() == ErrorKind::NotFound => {
                create_private_dir(&records)?;
                sync_dir(&self.root)?;
            }
            Err(error) => return Err(VaultError::io("inspect records directory", records, error)),
        }
        write_atomic(&records, &blob.file_name(), bytes)
    }

    /// Remove a record file; a missing file is not an error.
    pub(crate) fn remove_record(&self, blob: &BlobId) -> Result<(), VaultError> {
        let path = self.record_path(blob);
        match fs::remove_file(&path) {
            Ok(()) => sync_dir(&self.records_dir()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(VaultError::io("remove record file", path, error)),
        }
    }

    /// Well-formed record files present on disk (temporary and foreign files excluded).
    /// When a limit is supplied, reject excess entries before collecting them.
    pub(crate) fn record_blobs(&self, limit: Option<usize>) -> Result<Vec<BlobId>, VaultError> {
        let records = self.records_dir();
        match fs::symlink_metadata(&records) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => return Err(VaultError::UnsafePath(records)),
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(VaultError::io("inspect records directory", records, error)),
        }
        let mut blobs = Vec::new();
        for entry in fs::read_dir(&records)
            .map_err(|error| VaultError::io("list records directory", &records, error))?
        {
            let entry =
                entry.map_err(|error| VaultError::io("list records directory", &records, error))?;
            if let Some(blob) = entry.file_name().to_str().and_then(BlobId::from_file_name) {
                if limit.is_some_and(|limit| blobs.len() >= limit) {
                    return Err(VaultError::LimitExceeded(
                        "too many record files to restore",
                    ));
                }
                blobs.push(blob);
            }
        }
        blobs.sort();
        Ok(blobs)
    }

    /// Whether the directory holds any vault artifact (header or record files).
    pub(crate) fn has_vault_data(&self) -> Result<bool, VaultError> {
        match fs::symlink_metadata(self.header_path()) {
            Ok(_) => return Ok(true),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(VaultError::io(
                    "inspect vault header",
                    self.header_path(),
                    error,
                ));
            }
        }
        Ok(!self.record_blobs(None)?.is_empty())
    }

    /// Best-effort removal of temporary files left by interrupted writes.
    pub(crate) fn remove_temp_files(&self) {
        for dir in [self.root.clone(), self.records_dir()] {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let is_temp = entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with('.') && name.ends_with(TEMP_SUFFIX));
                let is_file = entry.file_type().is_ok_and(|kind| kind.is_file());
                if is_temp && is_file {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
    }
}

/// Read at most `max` bytes from a regular file; links and oversize files are rejected.
fn read_bounded(
    path: &Path,
    max: u64,
    too_large: impl FnOnce() -> VaultError,
) -> Result<Option<Vec<u8>>, VaultError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Err(VaultError::UnsafePath(path.to_path_buf())),
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(VaultError::io("inspect vault file", path, error)),
    }
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(VaultError::io("open vault file", path, error)),
    };
    let mut bytes = Vec::new();
    file.take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| VaultError::io("read vault file", path, error))?;
    if bytes.len() as u64 > max {
        return Err(too_large());
    }
    Ok(Some(bytes))
}

/// Write `bytes` to `dir/name` atomically and durably.
pub(crate) fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), VaultError> {
    let target = dir.join(name);
    match fs::symlink_metadata(&target) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Err(VaultError::UnsafePath(target)),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(VaultError::io("inspect vault file", target, error)),
    }
    let suffix: String = random_array::<8>()?
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let temporary = dir.join(format!(".{name}.{suffix}{TEMP_SUFFIX}"));
    let written = write_new_file(&temporary, bytes)
        .and_then(|()| {
            #[cfg(any(test, feature = "test-hooks"))]
            test_write_failure(dir, WriteFailure::BeforeRename)?;
            fs::rename(&temporary, &target)
        })
        .map_err(|error| VaultError::io("write vault file", &target, error));
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written?;
    sync_dir_io(dir).map_err(|source| {
        if name == HEADER_FILE {
            VaultError::CommitUncertain {
                path: target,
                source,
            }
        } else {
            VaultError::io("sync vault directory", dir, source)
        }
    })
}

fn write_new_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Create one directory level, private to the current user on Unix.
pub(crate) fn create_private_dir(path: &Path) -> Result<(), VaultError> {
    #[cfg(unix)]
    let builder = {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder
    };
    #[cfg(not(unix))]
    let builder = fs::DirBuilder::new();
    match builder.create(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(VaultError::io("create vault directory", path, error)),
    }
}

/// Persist directory entries (renames, creations) on Unix. Windows has no directory
/// handle flush through `std`; NTFS journals metadata, FAT/exFAT do not (see FORMAT.md).
pub(crate) fn sync_dir(dir: &Path) -> Result<(), VaultError> {
    sync_dir_io(dir).map_err(|error| VaultError::io("sync vault directory", dir, error))
}

fn sync_dir_io(dir: &Path) -> io::Result<()> {
    #[cfg(any(test, feature = "test-hooks"))]
    test_write_failure(dir, WriteFailure::AfterRename)?;
    #[cfg(unix)]
    {
        File::open(dir)?.sync_all()?;
    }
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// Where an injected failure strikes a write: before the temporary file is renamed
/// into place, or after the rename, when the directory is synced (which makes a
/// header commit uncertain).
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteFailure {
    BeforeRename,
    AfterRename,
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static WRITE_FAILURE: std::cell::RefCell<Option<(PathBuf, WriteFailure)>> =
        const { std::cell::RefCell::new(None) };
}

/// Inject one filesystem failure on this thread, without affecting other vaults.
#[cfg(any(test, feature = "test-hooks"))]
pub fn fail_next_write(dir: &Path, stage: WriteFailure) {
    WRITE_FAILURE.with(|failure| *failure.borrow_mut() = Some((dir.to_path_buf(), stage)));
}

#[cfg(any(test, feature = "test-hooks"))]
fn test_write_failure(dir: &Path, stage: WriteFailure) -> io::Result<()> {
    WRITE_FAILURE.with(|failure| {
        let mut failure = failure.borrow_mut();
        if failure
            .as_ref()
            .is_some_and(|(path, at)| path == dir && *at == stage)
        {
            *failure = None;
            Err(io::Error::other("injected vault filesystem failure"))
        } else {
            Ok(())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_writes_replace_files_and_leave_no_temporaries() {
        let dir = tempfile::tempdir().unwrap();
        write_atomic(dir.path(), "a.json", b"one").unwrap();
        write_atomic(dir.path(), "a.json", b"two").unwrap();
        assert_eq!(fs::read(dir.path().join("a.json")).unwrap(), b"two");
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["a.json"]);
    }

    #[test]
    fn bounded_reads_reject_oversize_files_and_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big");
        fs::write(&path, vec![0_u8; 11]).unwrap();
        let error = read_bounded(&path, 10, || VaultError::CorruptHeader("big".into()));
        assert!(matches!(error, Err(VaultError::CorruptHeader(_))));
        assert_eq!(
            read_bounded(&path, 11, || VaultError::NotFound)
                .unwrap()
                .unwrap()
                .len(),
            11
        );
        assert!(
            read_bounded(&dir.path().join("none"), 1, || VaultError::NotFound)
                .unwrap()
                .is_none()
        );
        assert!(matches!(
            read_bounded(dir.path(), 1, || VaultError::NotFound),
            Err(VaultError::UnsafePath(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_vault_paths_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(matches!(
            VaultDir::existing(&link),
            Err(VaultError::UnsafePath(_))
        ));
        let file = real.join("target.json");
        fs::write(&file, b"x").unwrap();
        std::os::unix::fs::symlink(&file, real.join(HEADER_FILE)).unwrap();
        let vault = VaultDir::existing(&real).unwrap();
        assert!(matches!(
            vault.read_header(),
            Err(VaultError::UnsafePath(_))
        ));
        assert!(matches!(
            vault.write_header(b"{}"),
            Err(VaultError::UnsafePath(_))
        ));
    }
}
