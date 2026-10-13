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
        assert_eq!(
            fs::read(root.path().join(result.executable)).unwrap(),
            b"fixture"
        );
    }
}
