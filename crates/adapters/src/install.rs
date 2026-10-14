//! Verified local artifact placement with contained, non-link destination directories.
use crate::{
    Driver,
    manager::InstallResult,
    state::{error, unsafe_link},
};
use aidex_native_sessions::RuntimeError;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};
const MAX_ARTIFACT: u64 = 256 * 1024 * 1024;
fn contained(root: &Path, path: &Path) -> Result<(), RuntimeError> {
    if !path.starts_with(root) {
        return Err(error(
            "integrity_failed",
            "Runtime installation escapes the drive",
        ));
    }
    let mut current = root.to_owned();
    for component in path.strip_prefix(root).unwrap().components() {
        if !matches!(component, std::path::Component::Normal(_)) {
            return Err(error(
                "integrity_failed",
                "Invalid runtime installation path",
            ));
        }
        current.push(component);
        if let Ok(metadata) = fs::symlink_metadata(&current)
            && unsafe_link(&metadata)
        {
            return Err(error(
                "integrity_failed",
                "Runtime installation contains a link or reparse point",
            ));
        }
    }
    if path.exists()
        && !path
            .canonicalize()
            .map_err(|_| {
                error(
                    "installer_unavailable",
                    "Runtime installation path unavailable",
                )
            })?
            .starts_with(root)
    {
        return Err(error(
            "integrity_failed",
            "Runtime installation escapes the drive",
        ));
    }
    Ok(())
}
/// Give the staged copy the source's permission bits plus owner read, write
/// and execute: the staging file is created `0600`, and the placed component
/// is a program every launch spawns. A drive whose filesystem keeps no modes
/// (FAT, exFAT) may refuse the change while its mount options already grant
/// execution, so only a copy its owner still cannot execute fails.
#[cfg(unix)]
fn executable(staged: &fs::File, source: &fs::Metadata) -> Result<(), RuntimeError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = (source.permissions().mode() & 0o777) | 0o700;
    let _ = staged.set_permissions(fs::Permissions::from_mode(mode));
    let placed = staged
        .metadata()
        .map_err(|_| error("installer_unavailable", "Runtime artifact cannot be staged"))?;
    if placed.permissions().mode() & 0o100 == 0 {
        return Err(error(
            "installer_unavailable",
            "Runtime artifact cannot be made executable on this drive",
        ));
    }
    Ok(())
}
/// Windows runs a program by its name, not a mode bit.
#[cfg(not(unix))]
fn executable(_staged: &fs::File, _source: &fs::Metadata) -> Result<(), RuntimeError> {
    Ok(())
}
pub(crate) fn place(
    root: &Path,
    driver: Driver,
    relative: &str,
    expected: &str,
) -> Result<InstallResult, RuntimeError> {
    let root = root.canonicalize().map_err(|_| {
        error(
            "installer_unavailable",
            "Runtime installation drive unavailable",
        )
    })?;
    let root = root.as_path();
    let source = root.join(relative);
    contained(root, &source)?;
    let metadata = fs::symlink_metadata(&source).map_err(|_| {
        error(
            "installer_unavailable",
            "Bundled runtime artifact is absent",
        )
    })?;
    if !metadata.is_file() || metadata.len() > MAX_ARTIFACT {
        return Err(error(
            "integrity_failed",
            "Runtime artifact exceeds package limits or is not a file",
        ));
    }
    let dir = root
        .join("runtimes")
        .join(format!("{:?}", driver).to_lowercase());
    for directory in [root.join("runtimes"), dir.clone()] {
        contained(root, &directory)?;
        match fs::create_dir(&directory) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => {
                return Err(error(
                    "installer_unavailable",
                    "Runtime installation directory unavailable",
                ));
            }
        }
        contained(root, &directory)?;
        if !directory.is_dir() {
            return Err(error(
                "integrity_failed",
                "Runtime installation parent is not a directory",
            ));
        }
    }
    let name = source
        .file_name()
        .ok_or_else(|| error("installer_unavailable", "Runtime artifact filename invalid"))?;
    let dest = dir.join(name);
    contained(root, &dest)?;
    if dest.exists() {
        return Err(error(
            "conflict",
            "Runtime component already exists; update requires explicit replacement",
        ));
    }
    let mut input = fs::File::open(&source)
        .map_err(|_| error("installer_unavailable", "Runtime artifact cannot be read"))?;
    contained(root, &dir)?;
    let mut output = tempfile::NamedTempFile::new_in(&dir)
        .map_err(|_| error("installer_unavailable", "Runtime artifact cannot be staged"))?;
    let mut hash = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 16384];
    loop {
        let n = input
            .read(&mut buffer)
            .map_err(|_| error("installer_unavailable", "Runtime artifact cannot be read"))?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > MAX_ARTIFACT {
            return Err(error(
                "integrity_failed",
                "Runtime artifact exceeds package limits",
            ));
        }
        hash.update(&buffer[..n]);
        output
            .write_all(&buffer[..n])
            .map_err(|_| error("installer_unavailable", "Runtime artifact cannot be staged"))?;
    }
    let digest = format!("{:x}", hash.finalize());
    if digest != expected {
        return Err(error(
            "integrity_failed",
            "Runtime artifact does not match the declared component digest",
        ));
    }
    executable(output.as_file(), &metadata)?;
    output
        .as_file()
        .sync_all()
        .map_err(|_| error("installer_unavailable", "Runtime artifact cannot be synced"))?;
    contained(root, &dir)?;
    contained(root, &dest)?;
    output.persist_noclobber(&dest).map_err(|e| {
        if e.error.kind() == std::io::ErrorKind::AlreadyExists {
            error("conflict", "Runtime component already exists")
        } else {
            error(
                "installer_unavailable",
                "Runtime artifact cannot be committed",
            )
        }
    })?;
    Ok(InstallResult {
        driver,
        executable: dest
            .strip_prefix(root)
            .unwrap()
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
        verified_sha256: digest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installation_refuses_a_destination_link_without_writing_outside_drive() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("drive");
        let outside = parent.path().join("outside");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(root.join("fixture.exe"), b"fixture artifact").unwrap();
        let digest = aidex_native_sessions::digest(b"fixture artifact");
        #[cfg(windows)]
        {
            let status = std::process::Command::new("cmd.exe")
                .args(["/C", "mklink", "/J"])
                .arg(root.join("runtimes"))
                .arg(&outside)
                .output()
                .unwrap();
            assert!(status.status.success());
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, root.join("runtimes")).unwrap();
        assert_eq!(
            place(&root, Driver::Codex, "fixture.exe", &digest)
                .unwrap_err()
                .code,
            "integrity_failed"
        );
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
    }
    #[test]
    fn digest_mismatch_never_publishes_a_component() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("fixture.exe"), b"fixture").unwrap();
        assert!(place(root.path(), Driver::Codex, "fixture.exe", "changed").is_err());
        assert!(!root.path().join("runtimes/codex/fixture.exe").exists());
        let result = place(
            root.path(),
            Driver::Codex,
            "fixture.exe",
            &aidex_native_sessions::digest(b"fixture"),
        )
        .unwrap();
        let placed = root.path().join(result.executable);
        assert_eq!(fs::read(&placed).unwrap(), b"fixture");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_ne!(
                fs::metadata(&placed).unwrap().permissions().mode() & 0o100,
                0
            );
        }
    }
    /// The staging file is created `0600`; the placed component must run.
    #[cfg(unix)]
    #[test]
    fn a_placed_component_is_executable() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let script = b"#!/bin/sh\nexit 0\n";
        let source = root.path().join("fixture.sh");
        fs::write(&source, script).unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
        let result = place(
            root.path(),
            Driver::Codex,
            "fixture.sh",
            &aidex_native_sessions::digest(script),
        )
        .unwrap();
        let placed = root.path().join(result.executable);
        assert_ne!(
            fs::metadata(&placed).unwrap().permissions().mode() & 0o100,
            0
        );
        // Another test thread forking while a file is open for writing can make
        // a fresh executable briefly busy (ETXTBSY); that is not this check.
        let mut attempts = 0;
        let status = loop {
            match std::process::Command::new(&placed).status() {
                Err(e) if e.raw_os_error() == Some(26) && attempts < 50 => {
                    attempts += 1;
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                status => break status.unwrap(),
            }
        };
        assert!(status.success());
    }
}
