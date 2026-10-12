//! Link-refusing inspection, bounded reads, and crash-safe writes.

use std::{
    fs::{self, File, Metadata, OpenOptions},
    io::{self, ErrorKind, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Whether metadata describes a symbolic link, junction, or other reparse point.
pub(crate) fn is_link(metadata: &Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

/// What is at a path, without following links.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Entry {
    Missing,
    Dir,
    File(u64),
    Link,
    Other,
}

pub(crate) fn inspect(path: &Path) -> io::Result<Entry> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if is_link(&metadata) => Ok(Entry::Link),
        Ok(metadata) if metadata.is_dir() => Ok(Entry::Dir),
        Ok(metadata) if metadata.is_file() => Ok(Entry::File(metadata.len())),
        Ok(_) => Ok(Entry::Other),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(Entry::Missing),
        Err(error) => Err(error),
    }
}

/// Walk `segments` below `base` (trusted), refusing links and non-directories.
/// `Ok(None)` when a component does not exist.
pub(crate) fn real_dir_beneath(
    base: &Path,
    segments: &[&str],
) -> Result<Option<PathBuf>, (PathBuf, Option<io::Error>)> {
    let mut current = base.to_path_buf();
    for segment in segments {
        current.push(segment);
        match inspect(&current) {
            Ok(Entry::Dir) => {}
            Ok(Entry::Missing) => return Ok(None),
            Ok(_) => return Err((current, None)),
            Err(error) => return Err((current, Some(error))),
        }
    }
    Ok(Some(current))
}

pub(crate) enum ReadFailure {
    TooLarge,
    NotUtf8,
    Io(io::Error),
}

fn not_regular() -> io::Error {
    io::Error::new(
        ErrorKind::InvalidInput,
        "not a regular file (symbolic links and junctions are refused)",
    )
}

/// Open a regular file without following a link at its final component.
#[cfg(windows)]
fn open_regular(path: &Path) -> io::Result<(File, Metadata)> {
    use std::os::windows::fs::OpenOptionsExt;
    // Open a reparse point itself rather than its target, then refuse it.
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = file.metadata()?;
    if is_link(&metadata) || !metadata.is_file() {
        return Err(not_regular());
    }
    Ok((file, metadata))
}

/// Open a regular file without following a link at its final component.
#[cfg(not(windows))]
fn open_regular(path: &Path) -> io::Result<(File, Metadata)> {
    let before = fs::symlink_metadata(path)?;
    if is_link(&before) || !before.is_file() {
        return Err(not_regular());
    }
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // The path was swapped for a link (or another file) after the check.
        if (before.dev(), before.ino()) != (metadata.dev(), metadata.ino()) {
            return Err(not_regular());
        }
    }
    if !metadata.is_file() {
        return Err(not_regular());
    }
    Ok((file, metadata))
}

/// Read a regular file of at most `max` bytes as UTF-8 text.
pub(crate) fn read_text(path: &Path, max: u64) -> Result<String, ReadFailure> {
    let (file, metadata) = open_regular(path).map_err(ReadFailure::Io)?;
    if metadata.len() > max {
        return Err(ReadFailure::TooLarge);
    }
    let mut bytes = Vec::new();
    file.take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(ReadFailure::Io)?;
    if bytes.len() as u64 > max {
        return Err(ReadFailure::TooLarge);
    }
    String::from_utf8(bytes).map_err(|_| ReadFailure::NotUtf8)
}

/// Write `bytes` to `destination` atomically: an exclusive temporary sibling,
/// fsync, then rename over the destination (which must not be a link).
pub(crate) fn write_atomic(destination: &Path, bytes: &[u8]) -> io::Result<()> {
    if matches!(
        inspect(destination)?,
        Entry::Link | Entry::Dir | Entry::Other
    ) {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "the destination is a link or not a regular file",
        ));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "path has no parent"))?;
    let name = destination
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (temporary, mut file) = loop {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".{name}.{}-{sequence}.tmp", std::process::id()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => break (candidate, file),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, destination)?;
        sync_dir(parent);
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(unix)]
fn sync_dir(dir: &Path) {
    if let Ok(dir) = File::open(dir) {
        let _ = dir.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) {}

/// Path components after `base` joined with `/`, or `None` when `path` is not
/// beneath `base`.
pub(crate) fn relative_slash(base: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(base).ok()?;
    Some(
        relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_writes_replace_and_leave_no_temporaries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("SKILL.md");
        write_atomic(&path, b"one").unwrap();
        write_atomic(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn bounded_reads_refuse_large_and_binary_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a");
        fs::write(&path, "12345").unwrap();
        assert_eq!(read_text(&path, 5).ok().as_deref(), Some("12345"));
        assert!(matches!(read_text(&path, 4), Err(ReadFailure::TooLarge)));
        fs::write(&path, [0xff, 0xfe]).unwrap();
        assert!(matches!(read_text(&path, 5), Err(ReadFailure::NotUtf8)));
    }
}
