//! Plugins carry skills and MCP configuration, not native code: files that the
//! operating system can run directly (by extension or by executable magic bytes) are
//! refused unless a `.mcp.json` server declares that exact file as its command.
//! Interpreted scripts (`.py`, `.js`, `.sh`, ...) are allowed as data; installed files
//! never keep an executable bit except declared MCP commands.

use std::{collections::HashSet, fs::File, io::Read, path::Path};

use crate::{
    PluginError,
    names::{fold, join},
    tree::Tree,
};

/// Extensions the host can launch directly or through a default association.
const EXECUTABLE_EXTENSIONS: [&str; 35] = [
    "app", "appimage", "appx", "bat", "cmd", "com", "cpl", "deb", "dll", "dmg", "drv", "exe",
    "hta", "jar", "jse", "lnk", "msc", "msi", "msix", "msp", "ocx", "pif", "pkg", "ps1", "psd1",
    "psm1", "reg", "rpm", "scr", "so", "sys", "vbe", "vbs", "wsf", "wsh",
];

/// Leading bytes of PE (`MZ`), ELF, Mach-O (both byte orders, 32/64-bit), and fat
/// Mach-O / Java class files.
fn has_executable_magic(head: &[u8]) -> bool {
    head.starts_with(b"MZ")
        || head.starts_with(b"\x7fELF")
        || [
            [0xfe, 0xed, 0xfa, 0xce],
            [0xfe, 0xed, 0xfa, 0xcf],
            [0xce, 0xfa, 0xed, 0xfe],
            [0xcf, 0xfa, 0xed, 0xfe],
            [0xca, 0xfe, 0xba, 0xbe],
        ]
        .iter()
        .any(|magic| head.starts_with(magic))
}

fn has_executable_extension(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.').is_some_and(|(stem, extension)| {
        !stem.is_empty()
            && EXECUTABLE_EXTENSIONS
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(extension))
    })
}

/// Refuse the first executable in `tree` that is not one of `declared` (plugin-relative
/// paths of MCP server commands).
pub(crate) fn check(root: &Path, tree: &Tree, declared: &[String]) -> Result<(), PluginError> {
    let declared: HashSet<String> = declared.iter().map(|path| fold(path)).collect();
    for file in &tree.files {
        if declared.contains(&fold(&file.path)) {
            continue;
        }
        if has_executable_extension(&file.path) || is_binary_executable(root, &file.path)? {
            return Err(PluginError::Executable {
                path: file.path.clone(),
            });
        }
    }
    Ok(())
}

fn is_binary_executable(root: &Path, relative: &str) -> Result<bool, PluginError> {
    let path = join(root, relative);
    let mut head = [0_u8; 4];
    let mut file = File::open(&path).map_err(|error| PluginError::io("read", &path, &error))?;
    let mut filled = 0;
    while filled < head.len() {
        match file.read(&mut head[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) => return Err(PluginError::io("read", &path, &error)),
        }
    }
    Ok(has_executable_magic(&head[..filled]))
}

/// Give declared MCP command files an executable bit (Unix only; elsewhere a no-op).
#[cfg(unix)]
pub(crate) fn mark_declared(root: &Path, declared: &[String]) -> Result<(), PluginError> {
    use std::os::unix::fs::PermissionsExt;
    for relative in declared {
        let path = join(root, relative);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() => {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                    .map_err(|error| PluginError::io("mark executable", &path, &error))?;
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn mark_declared(_root: &Path, _declared: &[String]) -> Result<(), PluginError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executables_are_recognized_by_extension_and_magic() {
        for path in [
            "bin/x.EXE",
            "a.dll",
            "run.cmd",
            "x.ps1",
            "lib/libz.so",
            "tool.jar",
        ] {
            assert!(has_executable_extension(path), "{path}");
        }
        for path in ["server.js", "main.py", "run.sh", "README.md", ".exe", "exe"] {
            assert!(!has_executable_extension(path), "{path}");
        }
        assert!(has_executable_magic(b"MZ\x90\x00"));
        assert!(has_executable_magic(b"\x7fELF"));
        assert!(has_executable_magic(&[0xcf, 0xfa, 0xed, 0xfe]));
        assert!(!has_executable_magic(b"# ti"));
        assert!(!has_executable_magic(b""));
    }
}
