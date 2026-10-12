//! Small filesystem helpers: bounded reads that refuse links, crash-safe atomic
//! writes, retrying renames, and self-removing scratch directories.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, ErrorKind, Read, Write},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use crate::PluginError;

/// Renames are retried this often: on Windows a scanner or indexer can briefly hold
/// a handle inside a freshly written directory.
const RENAME_ATTEMPTS: u32 = 20;
const RENAME_BACKOFF: Duration = Duration::from_millis(50);

/// Random lowercase hex for temporary names.
pub(crate) fn random_suffix() -> Result<String, PluginError> {
    let mut bytes = [0_u8; 8];
    getrandom::fill(&mut bytes)
        .map_err(|error| PluginError::Io(format!("randomness unavailable: {error}")))?;
    Ok(hex(&bytes))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

/// Read a regular file of at most `max` bytes; `Ok(None)` when it does not exist.
/// Links and non-files are refused with [`ErrorKind::InvalidInput`], larger files
/// with [`ErrorKind::FileTooLarge`].
pub(crate) fn read_bounded(path: &Path, max: u64) -> io::Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "not a regular file (links are refused)",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    }
    let mut bytes = Vec::new();
    File::open(path)?.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(io::Error::new(
            ErrorKind::FileTooLarge,
            format!("larger than {max} bytes"),
        ));
    }
    Ok(Some(bytes))
}

/// Replace `path` with `bytes` through a synced temporary sibling and a rename.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), PluginError> {
    if let Ok(metadata) = fs::symlink_metadata(path)
        && (metadata.file_type().is_symlink() || !metadata.is_file())
    {
        return Err(PluginError::Io(format!(
            "`{}` must be a regular file, not a link or directory",
            path.display()
        )));
    }
    let parent = path
        .parent()
        .ok_or_else(|| PluginError::Io(format!("`{}` has no parent", path.display())))?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp = parent.join(format!(".{name}.{}.tmp", random_suffix()?));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        rename_retry(&temp, path)?;
        sync_dir(parent)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temp);
        return Err(PluginError::io("write", path, &error));
    }
    Ok(())
}

/// `fs::rename`, retried briefly on transient permission errors.
pub(crate) fn rename_retry(from: &Path, to: &Path) -> io::Result<()> {
    let mut attempt = 0;
    loop {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(error)
                if attempt + 1 < RENAME_ATTEMPTS && error.kind() == ErrorKind::PermissionDenied =>
            {
                attempt += 1;
                thread::sleep(RENAME_BACKOFF);
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(unix)]
pub(crate) fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
pub(crate) fn sync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}

/// A scratch directory removed (best effort) when dropped.
#[derive(Debug)]
pub(crate) struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    /// Create `<parent>/<prefix>-<random>`; `parent` must already exist.
    pub(crate) fn create(parent: &Path, prefix: &str) -> Result<Self, PluginError> {
        let path = parent.join(format!("{prefix}-{}", random_suffix()?));
        fs::create_dir(&path).map_err(|error| PluginError::io("create", &path, &error))?;
        Ok(Self { path })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        // `remove_dir_all` does not follow links, so a link placed inside cannot
        // redirect the removal outside the scratch directory.
        let _ = fs::remove_dir_all(&self.path);
    }
}
