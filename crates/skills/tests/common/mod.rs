//! Fixtures shared by the skill integration tests.
#![allow(dead_code)]

use std::{
    fs,
    path::{Path, PathBuf},
};

use aidex_core::EntityId;
use aidex_drive::DrivePaths;
use aidex_skills::SkillWorkspace;

/// A temporary, initialized AiDEX drive and its paths.
pub struct Drive {
    pub dir: tempfile::TempDir,
    pub paths: DrivePaths,
}

pub fn drive() -> Drive {
    let dir = tempfile::tempdir().expect("temp dir");
    aidex_drive::init_with(
        dir.path(),
        aidex_drive::InitOptions {
            require_removable: false,
        },
    )
    .expect("initialize test drive");
    let paths = DrivePaths::open(dir.path()).expect("open drive paths");
    Drive { dir, paths }
}

/// A temporary workspace root registered under `id`.
pub fn workspace(id: &str) -> (tempfile::TempDir, SkillWorkspace) {
    let dir = tempfile::tempdir().expect("temp dir");
    let workspace = SkillWorkspace {
        id: EntityId::new(id).unwrap(),
        root: dir.path().to_path_buf(),
    };
    (dir, workspace)
}

/// `---\nname: <name>\ndescription: <description>\n---\n\n<body>\n`.
pub fn skill_text(name: &str, description: &str, body: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}\n")
}

/// Write `<root>/<dir>/<file>` with `text`, creating directories.
pub fn write(root: &Path, dir: &str, file: &str, text: &str) -> PathBuf {
    let dir = root.join(dir);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(file);
    fs::write(&path, text).unwrap();
    path
}

/// Write a valid `SKILL.md` for `name` in `<root>/<name>/`.
pub fn skill(root: &Path, name: &str, description: &str) -> PathBuf {
    write(
        root,
        name,
        "SKILL.md",
        &skill_text(name, description, &format!("Instructions of {name}.")),
    )
}

/// A fake credential that the redactor recognises (assembled at run time).
pub fn fake_token() -> String {
    format!("{}{}", "ghp_", "FAKE0fake1FAKE2fake3FAKE4fake5")
}

/// A directory link: a symbolic link on Unix, a junction on Windows.
pub fn dir_link(target: &Path, link: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).expect("create symlink");
        true
    }
    #[cfg(windows)]
    {
        let status = std::process::Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(link)
            .arg(target)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        let created = status.is_ok_and(|status| status.success());
        if !created {
            eprintln!("note: could not create a junction; skipping link assertions");
        }
        created
    }
}

/// A file symbolic link; on Windows this needs Developer Mode or elevation.
pub fn file_link(target: &Path, link: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).expect("create symlink");
        true
    }
    #[cfg(windows)]
    {
        match std::os::windows::fs::symlink_file(target, link) {
            Ok(()) => true,
            Err(error) => {
                eprintln!("note: file symlinks are not permitted here ({error}); skipping");
                false
            }
        }
    }
}
