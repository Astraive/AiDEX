use crate::{ExtensionError, Result};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
};

pub(crate) const MAX_SKILL: usize = 256 * 1024;
pub(crate) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub(crate) fn valid_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 64
        || !id.as_bytes()[0].is_ascii_lowercase()
        || !id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'_')
    {
        return Err(ExtensionError::Invalid(
            "id must be lowercase letters/digits/hyphens, at most 64 bytes".into(),
        ));
    }
    Ok(())
}
pub(crate) fn grants(ids: &[String], target: &str) -> bool {
    ids.iter().any(|id| id == "*" || id == target)
}
pub(crate) fn validate_grants(ids: &[String]) -> Result<()> {
    if ids.len() > 128
        || ids
            .iter()
            .any(|s| s.is_empty() || s.len() > 128 || s.chars().any(char::is_control))
    {
        return Err(ExtensionError::Invalid("invalid scope grants".into()));
    }
    Ok(())
}
fn link(meta: &fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    false
}
pub(crate) fn safe_path(root: &Path, relative: &str) -> Result<PathBuf> {
    let path = Path::new(relative);
    if path.is_absolute()
        || relative.contains('\\')
        || relative.contains(':')
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        || relative.is_empty()
    {
        return Err(ExtensionError::Invalid(
            "path must be a contained relative path".into(),
        ));
    }
    let mut current = root.to_path_buf();
    if fs::symlink_metadata(&current).is_ok_and(|m| link(&m)) {
        return Err(ExtensionError::Forbidden(
            "linked extension directory".into(),
        ));
    }
    for part in path.components() {
        current.push(part);
        if fs::symlink_metadata(&current).is_ok_and(|m| link(&m)) {
            return Err(ExtensionError::Forbidden("linked extension path".into()));
        }
    }
    Ok(current)
}
pub(crate) fn read(path: &Path, max: usize) -> Result<String> {
    let meta = fs::symlink_metadata(path)?;
    if link(&meta) || !meta.is_file() {
        return Err(ExtensionError::Forbidden(
            "extension must be a regular file".into(),
        ));
    }
    if meta.len() > max as u64 {
        return Err(ExtensionError::TooLarge);
    }
    let bytes = fs::read(path)?;
    if bytes.len() > max {
        return Err(ExtensionError::TooLarge);
    }
    String::from_utf8(bytes)
        .map_err(|_| ExtensionError::Invalid("extension files must be UTF-8".into()))
}
pub(crate) fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or(ExtensionError::Io)?;
    fs::create_dir_all(parent)?;
    let id = aidex_core::EntityId::generate("write").map_err(|_| ExtensionError::Io)?;
    let tmp = parent.join(format!(".{id}.tmp"));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}
