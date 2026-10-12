//! Staging an install source: a local folder is copied and a `.zip` is extracted into
//! a fresh scratch directory on the drive, both bounded by the plugin limits, without
//! following or creating links, and with every name validated before it is used.
//! Copies never keep permission bits from the source.

use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{self, BufReader, ErrorKind, Read},
    path::{Path, PathBuf},
};

use aidex_drive::DrivePaths;
use zip::ZipArchive;

use crate::{
    PluginError,
    limits::{MAX_ARCHIVE_BYTES, MAX_ARCHIVE_ENTRIES, MAX_PLUGIN_BYTES},
    manifest::{self, AIDEX_MANIFEST, CLAUDE_MANIFEST},
    names::{fold, join, validate_relative},
    tree::{self, check_totals},
};

/// macOS archive metadata, skipped when extracting.
const MACOS_METADATA_DIR: &str = "__MACOSX";
const S_IFMT: u32 = 0o170_000;
const S_IFREG: u32 = 0o100_000;
const S_IFDIR: u32 = 0o040_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SourceKind {
    Folder,
    Zip,
}

/// Classify an install source: an absolute path to a real folder or a `.zip` file.
pub(crate) fn source_kind(source: &Path) -> Result<SourceKind, PluginError> {
    if !source.is_absolute() {
        return Err(PluginError::InvalidRequest(
            "`source_path` must be an absolute path".into(),
        ));
    }
    let metadata = fs::symlink_metadata(source).map_err(|error| match error.kind() {
        ErrorKind::NotFound => PluginError::InvalidRequest(format!(
            "install source `{}` does not exist",
            source.display()
        )),
        _ => PluginError::io("inspect", source, &error),
    })?;
    let is_zip = source
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"));
    if metadata.file_type().is_symlink() {
        Err(PluginError::unsafe_content(
            ".",
            "the install source is a symbolic link or junction",
        ))
    } else if metadata.is_dir() {
        Ok(SourceKind::Folder)
    } else if metadata.is_file() && is_zip {
        Ok(SourceKind::Zip)
    } else {
        Err(PluginError::InvalidRequest(format!(
            "install source `{}` must be a folder or a .zip file",
            source.display()
        )))
    }
}

/// Refuse sources that are, or contain, the drive's private directories.
pub(crate) fn refuse_private_source(paths: &DrivePaths, source: &Path) -> Result<(), PluginError> {
    let source =
        fs::canonicalize(source).map_err(|error| PluginError::io("resolve", source, &error))?;
    let private = [
        paths.vault(),
        paths.identity(),
        paths.sessions(),
        paths.config(),
        paths.memory(),
        paths.index(),
        paths.runtime_state(),
    ];
    for dir in private {
        let Ok(dir) = fs::canonicalize(&dir) else {
            continue;
        };
        if source.starts_with(&dir) || dir.starts_with(&source) {
            return Err(PluginError::InvalidRequest(
                "plugins cannot be installed from the drive's private directories".into(),
            ));
        }
    }
    Ok(())
}

/// Copy or extract `source` into `stage`, which must not exist yet.
pub(crate) fn stage(source: &Path, kind: SourceKind, stage: &Path) -> Result<(), PluginError> {
    match kind {
        SourceKind::Folder => copy_folder(source, stage),
        SourceKind::Zip => extract_zip(source, stage),
    }
}

/// The plugin root inside `stage`: `stage` itself when it holds a manifest, or its
/// single top-level directory when that does.
pub(crate) fn locate_root(stage: &Path) -> Result<PathBuf, PluginError> {
    if manifest::find(stage).is_some() {
        return Ok(stage.to_path_buf());
    }
    let mut entries = fs::read_dir(stage)
        .map_err(|error| PluginError::io("list", stage, &error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| PluginError::io("list", stage, &error))?;
    if entries.len() == 1 {
        let only = entries.remove(0).path();
        let is_dir = fs::symlink_metadata(&only)
            .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink());
        if is_dir && manifest::find(&only).is_some() {
            return Ok(only);
        }
    }
    Err(PluginError::InvalidManifest(format!(
        "no `{AIDEX_MANIFEST}` or `{CLAUDE_MANIFEST}` at the root of the plugin or of its single top-level folder"
    )))
}

fn copy_folder(source: &Path, stage: &Path) -> Result<(), PluginError> {
    let tree = tree::scan(source)?;
    fs::create_dir(stage).map_err(|error| PluginError::io("create", stage, &error))?;
    for dir in &tree.dirs {
        let path = join(stage, dir);
        fs::create_dir_all(&path).map_err(|error| PluginError::io("create", &path, &error))?;
    }
    for file in &tree.files {
        let from = join(source, &file.path);
        match fs::symlink_metadata(&from) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
            _ => {
                return Err(PluginError::unsafe_content(
                    &file.path,
                    "the file changed while it was copied",
                ));
            }
        }
        let mut input = File::open(&from)
            .map_err(|error| PluginError::io("read", &from, &error))?
            .take(file.size + 1);
        let written = write_new(&join(stage, &file.path), &mut input)?;
        if written != file.size {
            return Err(PluginError::unsafe_content(
                &file.path,
                "the file changed while it was copied",
            ));
        }
    }
    Ok(())
}

/// Create `path` (which must not exist) from `input`; returns the bytes written.
fn write_new(path: &Path, input: &mut dyn Read) -> Result<u64, PluginError> {
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| PluginError::io("create", path, &error))?;
    let written = io::copy(input, &mut output)
        .and_then(|written| output.sync_all().map(|()| written))
        .map_err(|error| PluginError::io("write", path, &error))?;
    Ok(written)
}

fn extract_zip(archive: &Path, stage: &Path) -> Result<(), PluginError> {
    let length = fs::metadata(archive)
        .map_err(|error| PluginError::io("inspect", archive, &error))?
        .len();
    if length > MAX_ARCHIVE_BYTES {
        return Err(PluginError::TooLarge(format!(
            "the archive is larger than {} MiB",
            MAX_ARCHIVE_BYTES / (1024 * 1024)
        )));
    }
    let not_zip = |error: &dyn std::fmt::Display| {
        PluginError::InvalidRequest(format!(
            "`{}` is not a readable .zip archive: {error}",
            archive.display()
        ))
    };
    let file = File::open(archive).map_err(|error| PluginError::io("open", archive, &error))?;
    let mut zip = ZipArchive::new(BufReader::new(file)).map_err(|error| not_zip(&error))?;
    if zip.len() > MAX_ARCHIVE_ENTRIES {
        return Err(PluginError::TooLarge(format!(
            "the archive has more than {MAX_ARCHIVE_ENTRIES} entries"
        )));
    }
    fs::create_dir(stage).map_err(|error| PluginError::io("create", stage, &error))?;
    let mut seen = HashSet::new();
    let (mut files, mut total) = (0_usize, 0_u64);
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(|error| not_zip(&error))?;
        let raw = entry.name().to_owned();
        let is_dir = entry.is_dir();
        let name = if is_dir {
            raw.strip_suffix('/').unwrap_or(&raw)
        } else {
            raw.as_str()
        };
        if name == MACOS_METADATA_DIR || name.starts_with("__MACOSX/") {
            continue;
        }
        if raw.contains('\\') {
            return Err(PluginError::unsafe_content(
                &raw,
                "archive names must use `/` separators",
            ));
        }
        validate_relative(name).map_err(|reason| PluginError::unsafe_content(&raw, reason))?;
        if entry.is_symlink() {
            return Err(PluginError::unsafe_content(
                name,
                "symbolic links are not allowed in plugins",
            ));
        }
        if let Some(mode) = entry.unix_mode()
            && !matches!(mode & S_IFMT, 0 | S_IFREG | S_IFDIR)
        {
            return Err(PluginError::unsafe_content(
                name,
                "only regular files and directories are allowed in plugins",
            ));
        }
        let path = join(stage, name);
        if is_dir {
            fs::create_dir_all(&path).map_err(|error| PluginError::io("create", &path, &error))?;
            continue;
        }
        if !seen.insert(fold(name)) {
            return Err(PluginError::unsafe_content(
                name,
                "another entry has the same name ignoring case",
            ));
        }
        let declared = entry.size();
        files += 1;
        check_totals(files, total.saturating_add(declared))?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| PluginError::io("create", parent, &error))?;
        }
        let budget = (MAX_PLUGIN_BYTES - total).min(declared);
        let written = write_new(&path, &mut (&mut entry).take(budget + 1))?;
        if written != declared {
            return Err(PluginError::unsafe_content(
                name,
                "the extracted size does not match the size the archive declares",
            ));
        }
        total += written;
    }
    Ok(())
}
