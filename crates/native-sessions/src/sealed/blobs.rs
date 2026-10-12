//! Encrypted blobs beside the database: `<db stem>-artifacts/` for artifacts and
//! `<db stem>-attachments/` for chat attachments.
//!
//! A blob file holds sealed segments only; its name is the SHA-256 of those
//! sealed bytes, so names reveal nothing about the plaintext. Files are written
//! to a fresh temporary name, synced, and renamed into place before the row that
//! references them commits; unreferenced files are swept when the store opens.
//! Each domain has its own directory and its own sweep, so neither sweep (in
//! this daemon or an older one) can delete the other's blobs.
use super::crypto::Domain;
use crate::StoreError;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

pub(crate) struct BlobDir {
    root: PathBuf,
    domain: Domain,
}

impl BlobDir {
    /// `<db stem>-artifacts` or `<db stem>-attachments` beside `database`.
    pub(crate) fn beside(database: &Path, domain: Domain) -> Self {
        let stem = database
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "native".into());
        let suffix = match domain {
            Domain::Artifact => "artifacts",
            Domain::Attachment => "attachments",
        };
        Self {
            root: database.with_file_name(format!("{stem}-{suffix}")),
            domain,
        }
    }

    fn label(&self) -> &'static str {
        self.domain.label()
    }

    fn check_root(&self) -> Result<(), StoreError> {
        if fs::symlink_metadata(&self.root).is_ok_and(|m| !m.is_dir()) {
            return Err(format!(
                "{} blob directory is not a plain directory",
                self.label()
            ));
        }
        Ok(())
    }

    fn path(&self, address: &str) -> Result<PathBuf, StoreError> {
        if !valid_address(address) {
            // Not "invalid artifact ...": that prefix marks caller mistakes.
            return Err(format!("malformed encrypted {} blob address", self.label()));
        }
        Ok(self.root.join(address))
    }

    /// A new temporary file in this directory that collects sealed bytes and
    /// becomes a blob on [`BlobWriter::finish`]; dropped unfinished, it is deleted.
    pub(crate) fn writer(&self) -> Result<BlobWriter, StoreError> {
        self.check_root()?;
        fs::create_dir_all(&self.root).map_err(|e| e.to_string())?;
        self.check_root()?;
        let mut suffix = [0u8; 8];
        getrandom::fill(&mut suffix).map_err(|e| e.to_string())?;
        let temporary = self.root.join(format!(
            "{}.tmp",
            suffix
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        ));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|e| format!("{} blob write failed: {e}", self.label()))?;
        Ok(BlobWriter {
            root: self.root.clone(),
            temporary: Some(temporary),
            file: Some(file),
            hasher: Sha256::new(),
            written: 0,
            label: self.label(),
        })
    }

    /// Store sealed bytes and return their content address.
    pub(crate) fn write(&self, sealed: &[u8]) -> Result<String, StoreError> {
        let mut writer = self.writer()?;
        writer.write(sealed)?;
        writer.finish()
    }

    /// Read `len` sealed bytes at `start` from a blob of `expected` sealed bytes.
    pub(crate) fn read(
        &self,
        address: &str,
        expected: u64,
        start: u64,
        len: u64,
    ) -> Result<Vec<u8>, StoreError> {
        self.check_root()?;
        let path = self.path(address)?;
        let metadata = fs::symlink_metadata(&path)
            .map_err(|_| format!("encrypted {} blob is missing", self.label()))?;
        if !metadata.is_file() {
            return Err(format!(
                "encrypted {} blob is not a regular file",
                self.label()
            ));
        }
        if metadata.len() != expected || start.saturating_add(len) > expected {
            return Err(self.domain.truncated());
        }
        let mut file = File::open(&path).map_err(|e| e.to_string())?;
        file.seek(SeekFrom::Start(start))
            .map_err(|e| e.to_string())?;
        let mut out = vec![0u8; len as usize];
        file.read_exact(&mut out)
            .map_err(|_| self.domain.truncated())?;
        Ok(out)
    }

    /// Remove a blob after the row that referenced it is gone (best effort; a
    /// leftover file is swept on the next open).
    pub(crate) fn remove(&self, address: &str) {
        if self.check_root().is_ok()
            && let Ok(path) = self.path(address)
        {
            let _ = fs::remove_file(path);
        }
    }

    /// Delete temporary files and blobs no row references.
    pub(crate) fn sweep(&self, referenced: impl Fn(&str) -> Result<bool, StoreError>) {
        if self.check_root().is_err() {
            return;
        }
        let Ok(entries) = fs::read_dir(&self.root) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_file = entry.file_type().is_ok_and(|t| t.is_file());
            let orphan = if name.ends_with(".tmp") {
                true
            } else {
                valid_address(&name) && referenced(&name).is_ok_and(|used| !used)
            };
            if is_file && orphan {
                let _ = fs::remove_file(entry.path());
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
}

/// Sealed bytes on their way into a blob: a temporary file plus the running
/// SHA-256 that becomes the blob's address.
pub(crate) struct BlobWriter {
    root: PathBuf,
    temporary: Option<PathBuf>,
    file: Option<File>,
    hasher: Sha256,
    written: u64,
    label: &'static str,
}

impl BlobWriter {
    fn file(&mut self) -> Result<&mut File, StoreError> {
        self.file
            .as_mut()
            .ok_or_else(|| format!("{} blob writer is closed", self.label))
    }

    /// Append sealed bytes.
    pub(crate) fn write(&mut self, sealed: &[u8]) -> Result<(), StoreError> {
        let label = self.label;
        let file = self.file()?;
        file.seek(SeekFrom::End(0))
            .and_then(|_| file.write_all(sealed))
            .map_err(|e| format!("{label} blob write failed: {e}"))?;
        self.hasher.update(sealed);
        self.written += sealed.len() as u64;
        Ok(())
    }

    /// Sealed bytes written so far.
    pub(crate) fn written(&self) -> u64 {
        self.written
    }

    /// Read back `len` sealed bytes at `start`.
    pub(crate) fn read_at(&mut self, start: u64, len: u64) -> Result<Vec<u8>, StoreError> {
        if start.saturating_add(len) > self.written {
            return Err(format!("truncated encrypted {}", self.label));
        }
        let label = self.label;
        let file = self.file()?;
        let mut out = vec![0u8; len as usize];
        file.seek(SeekFrom::Start(start))
            .and_then(|_| file.read_exact(&mut out))
            .map_err(|_| format!("truncated encrypted {label}"))?;
        Ok(out)
    }

    /// Sync and rename the file into place; returns its content address.
    pub(crate) fn finish(mut self) -> Result<String, StoreError> {
        let address = format!("{:x}", std::mem::take(&mut self.hasher).finalize());
        let label = self.label;
        let file = self.file.take();
        let temporary = self.temporary.take();
        let (Some(file), Some(temporary)) = (file, temporary) else {
            return Err(format!("{label} blob writer is closed"));
        };
        let target = self.root.join(&address);
        let done = file.sync_all().and_then(|()| {
            drop(file);
            fs::rename(&temporary, &target)
        });
        if let Err(error) = done {
            let _ = fs::remove_file(&temporary);
            return Err(format!("{label} blob write failed: {error}"));
        }
        Ok(address)
    }
}

impl Drop for BlobWriter {
    fn drop(&mut self) {
        drop(self.file.take());
        if let Some(temporary) = self.temporary.take() {
            let _ = fs::remove_file(temporary);
        }
    }
}

fn valid_address(address: &str) -> bool {
    address.len() == 64
        && address
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blobs_are_content_addressed_and_confined() {
        let dir = tempfile::tempdir().unwrap();
        let blobs = BlobDir::beside(&dir.path().join("native.sqlite3"), Domain::Artifact);
        assert_eq!(blobs.root(), dir.path().join("native-artifacts"));
        let address = blobs.write(b"sealed fixture bytes").unwrap();
        assert_eq!(
            address,
            format!("{:x}", Sha256::digest(b"sealed fixture bytes"))
        );
        assert_eq!(blobs.read(&address, 20, 7, 7).unwrap(), b"fixture");
        assert!(blobs.read(&address, 21, 0, 1).is_err());
        assert!(blobs.read(&address, 20, 15, 6).is_err());
        assert!(blobs.read("../native.sqlite3", 20, 0, 1).is_err());
        std::fs::write(blobs.root().join("stale.tmp"), b"x").unwrap();
        blobs.sweep(|_| Ok(true));
        assert!(!blobs.root().join("stale.tmp").exists());
        assert!(blobs.root().join(&address).exists());
        blobs.sweep(|_| Err("database unavailable".into()));
        assert!(blobs.root().join(&address).exists());
        blobs.sweep(|_| Ok(false));
        assert!(!blobs.root().join(&address).exists());
    }

    #[test]
    fn writers_stream_into_their_own_directory_and_clean_up() {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("native.sqlite3");
        let attachments = BlobDir::beside(&database, Domain::Attachment);
        assert_eq!(attachments.root(), dir.path().join("native-attachments"));
        let mut writer = attachments.writer().unwrap();
        writer.write(b"sealed ").unwrap();
        writer.write(b"segments").unwrap();
        assert_eq!(writer.written(), 15);
        assert_eq!(writer.read_at(7, 8).unwrap(), b"segments");
        assert!(writer.read_at(8, 8).is_err());
        let address = writer.finish().unwrap();
        assert_eq!(address, format!("{:x}", Sha256::digest(b"sealed segments")));
        assert_eq!(
            std::fs::read(attachments.root().join(&address)).unwrap(),
            b"sealed segments"
        );
        let abandoned = attachments.writer().unwrap();
        drop(abandoned);
        let names: Vec<_> = std::fs::read_dir(attachments.root())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec![address], "an unfinished writer leaves nothing");
        assert!(!dir.path().join("native-artifacts").exists());
    }
}
