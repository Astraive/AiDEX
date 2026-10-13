//! Platform path rules: Windows verbatim-prefix normalization, case-insensitive
//! comparison, and reserved component spellings.
//!
//! Comparison is component-wise on canonical paths. Windows compares components
//! case-insensitively (NTFS/exFAT default) and treats `\\?\C:\x` and `C:\x` (and
//! `\\?\UNC\s\x` and `\\s\x`) as the same location. Other platforms compare exactly;
//! an exact comparison can only reject, never wrongly accept, a containment check.

use std::{
    ffi::{OsStr, OsString},
    path::{Component, Path, PathBuf, Prefix},
};

/// Windows `MAX_PATH`; longer paths keep their verbatim prefix when displayed.
const MAX_PATH: usize = 260;

/// Device names Windows resolves in every directory, with or without an extension.
const RESERVED_DEVICE_NAMES: [&str; 6] = ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"];

/// Why a path component cannot be used safely on Windows, or `None` when it can.
///
/// Rejects device names (`CON`, `PRN`, `AUX`, `NUL`, `COM0`-`COM9`, `LPT0`-`LPT9`,
/// including superscript-digit forms, `CONIN$`, `CONOUT$`) with any extension,
/// trailing dots or spaces (Windows strips them, aliasing another name), and the
/// characters `< > : " | ? *` and control characters. This is a pure function so the
/// rules are tested on every platform; they are enforced only on Windows.
pub fn windows_component_issue(component: &str) -> Option<&'static str> {
    if component.ends_with('.') || component.ends_with(' ') {
        return Some("names ending in a dot or space are aliased by Windows");
    }
    if component
        .chars()
        .any(|ch| ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
    {
        return Some("the name contains a character Windows does not allow");
    }
    let stem = component
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    let numbered = ["COM", "LPT"].iter().any(|device| {
        stem.strip_prefix(device).is_some_and(|digit| {
            matches!(
                digit,
                "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"
            ) || matches!(digit, "\u{b9}" | "\u{b2}" | "\u{b3}")
        })
    });
    if numbered || RESERVED_DEVICE_NAMES.contains(&stem.as_str()) {
        return Some("the name is a reserved Windows device name");
    }
    None
}

/// Whether the platform-specific component rules apply on this build.
pub(crate) const ENFORCE_WINDOWS_RULES: bool = cfg!(windows);

/// Normalized comparison keys, one per path component.
///
/// A path prefix becomes one key (`c:` for both `C:` and `\\?\C:`, `\\server\share`
/// for both UNC spellings), the root separator becomes `/`, and on Windows every
/// key is lowercased.
pub(crate) fn component_keys(path: &Path) -> Vec<OsString> {
    path.components()
        .filter_map(|component| match component {
            Component::Prefix(prefix) => Some(prefix_key(prefix.kind())),
            Component::RootDir => Some(OsString::from("/")),
            Component::CurDir => None,
            Component::ParentDir => Some(OsString::from("..")),
            Component::Normal(name) => Some(fold_case(name)),
        })
        .collect()
}

fn prefix_key(prefix: Prefix<'_>) -> OsString {
    let text = match prefix {
        Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
            format!("{}:", char::from(letter).to_ascii_lowercase())
        }
        Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => format!(
            r"\\{}\{}",
            server.to_string_lossy(),
            share.to_string_lossy()
        )
        .to_lowercase(),
        Prefix::Verbatim(name) => format!(r"\\?\{}", name.to_string_lossy()).to_lowercase(),
        Prefix::DeviceNS(name) => format!(r"\\.\{}", name.to_string_lossy()).to_lowercase(),
    };
    OsString::from(text)
}

fn fold_case(name: &OsStr) -> OsString {
    if cfg!(windows) {
        OsString::from(name.to_string_lossy().to_lowercase())
    } else {
        name.to_os_string()
    }
}

/// Whether `path` is `root` or lies beneath it (both should be canonical).
pub(crate) fn is_within(path: &Path, root: &Path) -> bool {
    component_keys(path).starts_with(&component_keys(root))
}

/// Whether two spellings name the same location under [`component_keys`].
pub(crate) fn same_location(left: &Path, right: &Path) -> bool {
    component_keys(left) == component_keys(right)
}

/// The components of `path` below `root`, in their original spelling, or `None`
/// when `path` is not within `root`.
pub(crate) fn strip_root(path: &Path, root: &Path) -> Option<Vec<OsString>> {
    let root_len = component_keys(root).len();
    if !is_within(path, root) {
        return None;
    }
    let originals: Vec<OsString> = path
        .components()
        .filter(|component| !matches!(component, Component::CurDir))
        .map(|component| component.as_os_str().to_os_string())
        .collect();
    Some(originals.into_iter().skip(root_len).collect())
}

/// A stable key for duplicate detection of host roots.
pub(crate) fn root_key(path: &Path) -> String {
    let keys = component_keys(path);
    let mut text = String::new();
    for (index, key) in keys.iter().enumerate() {
        let key = key.to_string_lossy();
        if index > 0 && !text.ends_with('/') && key != "/" {
            text.push('/');
        }
        text.push_str(&key);
    }
    text
}

/// A canonical path for users and persistence: the Windows verbatim prefix is
/// removed when the plain spelling denotes the same file; other platforms are
/// returned unchanged.
pub fn display_path(path: &Path) -> PathBuf {
    if cfg!(windows)
        && let Some(simplified) = simplify_verbatim(&path.to_string_lossy())
    {
        return PathBuf::from(simplified);
    }
    path.to_path_buf()
}

/// `\\?\C:\x` → `C:\x` and `\\?\UNC\s\share\x` → `\\s\share\x`, when every component
/// is an ordinary Windows name and the result fits `MAX_PATH`; otherwise `None`.
pub(crate) fn simplify_verbatim(text: &str) -> Option<String> {
    let simplified = if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else {
        let rest = text.strip_prefix(r"\\?\")?;
        let bytes = rest.as_bytes();
        if bytes.len() < 3 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' {
            return None;
        }
        if bytes[2] != b'\\' {
            return None;
        }
        rest.to_owned()
    };
    let ordinary = simplified
        .trim_start_matches('\\')
        .split('\\')
        .skip(1)
        .filter(|component| !component.is_empty())
        .all(|component| windows_component_issue(component).is_none());
    (ordinary && simplified.len() < MAX_PATH).then_some(simplified)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_device_names_are_rejected_with_any_extension() {
        for name in [
            "CON",
            "con",
            "Prn",
            "aux",
            "NUL",
            "nul.txt",
            "COM1",
            "com9.log",
            "LPT1",
            "lpt3.c",
            "COM0",
            "CON .txt",
            "conin$",
            "CONOUT$",
            "COM\u{b9}",
        ] {
            assert!(windows_component_issue(name).is_some(), "{name}");
        }
        for name in [
            "CONSOLE",
            "console.txt",
            "COM10",
            "LPT",
            "nul_file",
            "aux-data",
            "readme.md",
            ".gitignore",
            "con-fig",
        ] {
            assert_eq!(windows_component_issue(name), None, "{name}");
        }
    }

    #[test]
    fn trailing_dots_spaces_and_forbidden_characters_are_rejected() {
        for name in [
            "file.", "file ", "dir..", "a?b", "a*b", "a<b", "a|b", "a\"b",
        ] {
            assert!(windows_component_issue(name).is_some(), "{name}");
        }
        assert_eq!(windows_component_issue(".hidden"), None);
    }

    #[test]
    fn verbatim_prefixes_simplify_only_when_equivalent() {
        assert_eq!(
            simplify_verbatim(r"\\?\C:\Users\dev\repo").as_deref(),
            Some(r"C:\Users\dev\repo")
        );
        assert_eq!(
            simplify_verbatim(r"\\?\UNC\server\share\repo").as_deref(),
            Some(r"\\server\share\repo")
        );
        assert_eq!(simplify_verbatim(r"C:\plain"), None);
        assert_eq!(simplify_verbatim(r"\\?\C:\dir\name."), None);
        assert_eq!(simplify_verbatim(r"\\?\C:\dir\CON"), None);
        assert_eq!(simplify_verbatim(r"\\?\Volume{1234}\x"), None);
        let long = format!(r"\\?\C:\{}", "a".repeat(300));
        assert_eq!(simplify_verbatim(&long), None);
    }

    #[test]
    fn containment_is_component_wise() {
        let root = Path::new("/work/repo");
        assert!(is_within(Path::new("/work/repo"), root));
        assert!(is_within(Path::new("/work/repo/src/main.rs"), root));
        assert!(!is_within(Path::new("/work/repo2/src"), root));
        assert!(!is_within(Path::new("/work"), root));
        assert_eq!(
            strip_root(Path::new("/work/repo/src/main.rs"), root),
            Some(vec![OsString::from("src"), OsString::from("main.rs")])
        );
        assert_eq!(strip_root(Path::new("/elsewhere"), root), None);
        assert!(same_location(Path::new("/work/./repo/"), root));
        assert!(!same_location(Path::new("/work/repo/src"), root));
        assert!(!same_location(Path::new("/work"), root));
    }

    #[cfg(windows)]
    #[test]
    fn windows_containment_ignores_case_and_verbatim_prefixes() {
        let root = Path::new(r"\\?\C:\Work\Repo");
        assert!(is_within(Path::new(r"C:\work\repo\SRC\a.rs"), root));
        assert!(is_within(Path::new(r"\\?\c:\WORK\REPO"), root));
        assert!(!is_within(Path::new(r"D:\Work\Repo"), root));
        assert_eq!(
            strip_root(Path::new(r"c:\work\repo\Src\A.rs"), root),
            Some(vec![OsString::from("Src"), OsString::from("A.rs")])
        );
        assert_eq!(
            root_key(Path::new(r"\\?\C:\Work\Repo")),
            root_key(Path::new(r"c:\work\repo"))
        );
        assert!(is_within(
            Path::new(r"\\?\UNC\Server\Share\x"),
            Path::new(r"\\server\share")
        ));
        assert!(same_location(Path::new(r"c:\work\REPO"), root));
        assert!(same_location(
            Path::new(r"\\?\UNC\Server\Share\x"),
            Path::new(r"\\server\share\X")
        ));
        assert!(!same_location(Path::new(r"C:\Work\Repo\src"), root));
        assert!(!same_location(Path::new(r"D:\Work\Repo"), root));
    }

    #[cfg(unix)]
    #[test]
    fn unix_containment_is_case_sensitive() {
        assert!(!is_within(
            Path::new("/Work/Repo/a"),
            Path::new("/work/repo")
        ));
        assert_ne!(root_key(Path::new("/A")), root_key(Path::new("/a")));
        assert!(!same_location(Path::new("/A"), Path::new("/a")));
    }
}
