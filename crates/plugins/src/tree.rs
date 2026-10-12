//! Bounded, link-refusing walks of a plugin tree and its deterministic SHA-256 hash.
//!
//! The tree hash covers every regular file (directories carry no content and are not
//! hashed, so a `.zip` with or without explicit directory entries hashes like the
//! folder it came from):
//!
//! ```text
//! SHA-256( "aidex-plugin-tree-v1\n"
//!          || for each file, sorted by its UTF-8 relative path (byte order):
//!             <path> "\0" <size in decimal> "\0" <SHA-256 of the content, hex> "\n" )
//! ```
//!
//! Paths cannot contain NUL or newlines (control characters are refused), so the
//! encoding is unambiguous.

use std::{
    collections::HashSet,
    fs::{self, File},
    io::{ErrorKind, Read},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use crate::{
    PluginError,
    fsutil::hex,
    limits::{MAX_PLUGIN_BYTES, MAX_PLUGIN_DIRS, MAX_PLUGIN_FILES},
    names::{fold, join, validate_relative},
};

const TREE_HASH_DOMAIN: &[u8] = b"aidex-plugin-tree-v1\n";
const READ_CHUNK: usize = 64 * 1024;

/// One regular file of a plugin tree.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TreeFile {
    /// `/`-separated path relative to the plugin root.
    pub path: String,
    pub size: u64,
}

/// The validated listing of a plugin tree.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Tree {
    /// Files sorted by path.
    pub files: Vec<TreeFile>,
    /// Directories in walk order (parents before children).
    pub dirs: Vec<String>,
    pub total_bytes: u64,
}

/// Walk `root` without following links and validate every name and limit.
pub(crate) fn scan(root: &Path) -> Result<Tree, PluginError> {
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(PluginError::unsafe_content(
                ".",
                "the plugin directory is a symbolic link or junction",
            ));
        }
        Ok(metadata) if !metadata.is_dir() => {
            return Err(PluginError::unsafe_content(".", "not a directory"));
        }
        Ok(_) => {}
        Err(error) => return Err(PluginError::io("inspect", root, &error)),
    }
    let mut tree = Tree::default();
    let mut seen = HashSet::new();
    let mut pending: Vec<(PathBuf, String)> = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, relative_dir)) = pending.pop() {
        let entries = fs::read_dir(&dir).map_err(|error| PluginError::io("list", &dir, &error))?;
        for entry in entries {
            let entry = entry.map_err(|error| PluginError::io("list", &dir, &error))?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                return Err(PluginError::unsafe_content(
                    &format!("{relative_dir}/{}", name.to_string_lossy()),
                    "the file name is not valid Unicode",
                ));
            };
            let relative = if relative_dir.is_empty() {
                name.to_owned()
            } else {
                format!("{relative_dir}/{name}")
            };
            validate_relative(&relative)
                .map_err(|reason| PluginError::unsafe_content(&relative, reason))?;
            if !seen.insert(fold(&relative)) {
                return Err(PluginError::unsafe_content(
                    &relative,
                    "another entry has the same name ignoring case",
                ));
            }
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| PluginError::io("inspect", &path, &error))?;
            let kind = metadata.file_type();
            if kind.is_symlink() {
                return Err(PluginError::unsafe_content(
                    &relative,
                    "symbolic links and junctions are not allowed in plugins",
                ));
            } else if kind.is_dir() {
                tree.dirs.push(relative.clone());
                if tree.dirs.len() > MAX_PLUGIN_DIRS {
                    return Err(PluginError::TooLarge(format!(
                        "more than {MAX_PLUGIN_DIRS} directories"
                    )));
                }
                pending.push((path, relative));
            } else if kind.is_file() {
                tree.total_bytes = tree.total_bytes.saturating_add(metadata.len());
                tree.files.push(TreeFile {
                    path: relative,
                    size: metadata.len(),
                });
                check_totals(tree.files.len(), tree.total_bytes)?;
            } else {
                return Err(PluginError::unsafe_content(
                    &relative,
                    "only regular files and directories are allowed in plugins",
                ));
            }
        }
    }
    tree.files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(tree)
}

/// Refuse a tree with too many files or bytes.
pub(crate) fn check_totals(files: usize, bytes: u64) -> Result<(), PluginError> {
    if files > MAX_PLUGIN_FILES {
        return Err(PluginError::TooLarge(format!(
            "more than {MAX_PLUGIN_FILES} files"
        )));
    }
    if bytes > MAX_PLUGIN_BYTES {
        return Err(PluginError::TooLarge(format!(
            "more than {} MiB of files",
            MAX_PLUGIN_BYTES / (1024 * 1024)
        )));
    }
    Ok(())
}

/// The tree hash of a scanned tree; each file must still have its scanned size.
pub(crate) fn hash(root: &Path, tree: &Tree) -> Result<String, PluginError> {
    let mut hasher = Sha256::new();
    hasher.update(TREE_HASH_DOMAIN);
    for file in &tree.files {
        let digest = hash_file(&join(root, &file.path), file)?;
        hasher.update(file.path.as_bytes());
        hasher.update(b"\0");
        hasher.update(file.size.to_string().as_bytes());
        hasher.update(b"\0");
        hasher.update(digest.as_bytes());
        hasher.update(b"\n");
    }
    Ok(hex(&hasher.finalize()))
}

/// Scan and hash `root`: the deterministic SHA-256 tree hash of a plugin directory.
pub fn tree_sha256(root: &Path) -> Result<String, PluginError> {
    let tree = scan(root)?;
    hash(root, &tree)
}

fn hash_file(path: &Path, file: &TreeFile) -> Result<String, PluginError> {
    let changed = || PluginError::unsafe_content(&file.path, "the file changed while being read");
    let metadata = fs::symlink_metadata(path).map_err(|error| match error.kind() {
        ErrorKind::NotFound => changed(),
        _ => PluginError::io("inspect", path, &error),
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(changed());
    }
    let mut input = File::open(path)
        .map_err(|error| PluginError::io("read", path, &error))?
        .take(file.size + 1);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; READ_CHUNK];
    let mut total = 0_u64;
    loop {
        let read = input
            .read(&mut buffer)
            .map_err(|error| PluginError::io("read", path, &error))?;
        if read == 0 {
            break;
        }
        total += read as u64;
        hasher.update(&buffer[..read]);
    }
    if total != file.size {
        return Err(changed());
    }
    Ok(hex(&hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_independent_of_creation_order_and_directories() {
        let one = tempfile::tempdir().unwrap();
        fs::create_dir_all(one.path().join("b/c")).unwrap();
        fs::write(one.path().join("b/c/x.txt"), "x").unwrap();
        fs::write(one.path().join("a.txt"), "a").unwrap();
        fs::create_dir(one.path().join("empty")).unwrap();

        let two = tempfile::tempdir().unwrap();
        fs::write(two.path().join("a.txt"), "a").unwrap();
        fs::create_dir_all(two.path().join("b/c")).unwrap();
        fs::write(two.path().join("b/c/x.txt"), "x").unwrap();

        let first = tree_sha256(one.path()).unwrap();
        assert_eq!(first, tree_sha256(two.path()).unwrap());
        assert_eq!(first, tree_sha256(one.path()).unwrap());
        assert_eq!(first.len(), 64);

        fs::write(two.path().join("b/c/x.txt"), "y").unwrap();
        assert_ne!(first, tree_sha256(two.path()).unwrap());
        fs::write(two.path().join("b/c/x.txt"), "x").unwrap();
        fs::rename(two.path().join("a.txt"), two.path().join("A2.txt")).unwrap();
        assert_ne!(first, tree_sha256(two.path()).unwrap());
    }

    #[test]
    fn scans_refuse_case_collisions_and_count_bytes() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("one.txt"), "12345").unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub/two.txt"), "67").unwrap();
        let tree = scan(dir.path()).unwrap();
        assert_eq!(tree.total_bytes, 7);
        assert_eq!(
            tree.files
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            ["one.txt", "sub/two.txt"]
        );
        assert_eq!(tree.dirs, ["sub"]);
        assert!(check_totals(MAX_PLUGIN_FILES + 1, 0).is_err());
        assert!(check_totals(1, MAX_PLUGIN_BYTES + 1).is_err());
    }
}
