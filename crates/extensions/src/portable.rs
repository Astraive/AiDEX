//! Drive-relative program/config references survive a removable-drive letter change.
use crate::{ExtensionError, McpServerConfig, McpTransport, Result, storage::safe_path};
use std::path::{Path, PathBuf};
fn relative(root: &Path, value: &str) -> Option<String> {
    let root = root
        .to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_owned();
    let value = value.replace('\\', "/");
    let prefix = format!("{root}/");
    let matches = if cfg!(windows) {
        value
            .to_ascii_lowercase()
            .starts_with(&prefix.to_ascii_lowercase())
    } else {
        value.starts_with(&prefix)
    };
    matches.then(|| value[prefix.len()..].to_owned())
}
pub(crate) fn normalize(root: &Path, config: &mut McpServerConfig) -> Result<()> {
    let McpTransport::Stdio { command, args, cwd } = &mut config.transport else {
        return Ok(());
    };
    *command = normalize_value(root, command)?;
    if !Path::new(command).is_absolute()
        && !command.starts_with("drive:")
        && (command.contains('/') || command.contains('\\'))
    {
        let path = command.replace('\\', "/");
        safe_path(root, &path)?;
        *command = format!("drive:{path}");
    }
    for arg in args {
        if let Some((key, value)) = arg.split_once('=') {
            let normalized = normalize_value(root, value)?;
            *arg = format!("{key}={normalized}");
        } else {
            *arg = normalize_value(root, arg)?
        }
    }
    if let Some(cwd) = cwd {
        *cwd = normalize_value(root, cwd)?;
    }
    Ok(())
}
fn normalize_value(root: &Path, value: &str) -> Result<String> {
    let root_text = root
        .to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_owned();
    let value_text = value.replace('\\', "/").trim_end_matches('/').to_owned();
    if if cfg!(windows) {
        root_text.eq_ignore_ascii_case(&value_text)
    } else {
        root_text == value_text
    } {
        return Ok("drive:.".into());
    }
    if value == "drive:." {
        return Ok(value.into());
    }
    if let Some(relative) = value.strip_prefix("drive:") {
        safe_path(root, relative)?;
        return Ok(value.into());
    }
    if let Some(relative) = relative(root, value) {
        safe_path(root, &relative)?;
        return Ok(format!("drive:{relative}"));
    }
    Ok(value.into())
}
pub(crate) fn resolve(root: Option<&Path>, value: &str) -> Result<String> {
    if value == "drive:." {
        return root
            .map(|root| root.to_string_lossy().into_owned())
            .ok_or_else(|| {
                ExtensionError::Unavailable(
                    "portable reference requires a drive-aware connector".into(),
                )
            });
    }
    if let Some(relative) = value.strip_prefix("drive:") {
        let root = root.ok_or_else(|| {
            ExtensionError::Unavailable(
                "portable MCP command requires a drive-aware connector".into(),
            )
        })?;
        return Ok(safe_path(root, relative)?.to_string_lossy().into_owned());
    }
    if let Some((key, value)) = value.split_once('=')
        && value.starts_with("drive:")
    {
        return Ok(format!("{key}={}", resolve(root, value)?));
    }
    Ok(value.into())
}
pub(crate) fn cwd(root: Option<&Path>, workspace: &Path, value: Option<&str>) -> Result<PathBuf> {
    match value {
        Some(value) if value.starts_with("drive:") => Ok(PathBuf::from(resolve(root, value)?)),
        Some(value) if Path::new(value).is_absolute() => Ok(PathBuf::from(value)),
        Some(value) => safe_path(workspace, &value.replace('\\', "/")),
        None => Ok(workspace.to_path_buf()),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drive_references_resolve_at_the_new_root() {
        let old = tempfile::tempdir().unwrap();
        let new = tempfile::tempdir().unwrap();
        let mut config = McpServerConfig {
            id: "portable".into(),
            name: "Portable".into(),
            enabled: true,
            trusted: true,
            workspace_ids: vec!["*".into()],
            profiles: vec!["*".into()],
            transport: McpTransport::Stdio {
                command: old
                    .path()
                    .join("runtime/server.exe")
                    .to_string_lossy()
                    .into_owned(),
                args: vec![format!(
                    "--config={}",
                    old.path().join("mcp/server.json").display()
                )],
                cwd: Some(
                    old.path()
                        .join("workspaces/repo")
                        .to_string_lossy()
                        .into_owned(),
                ),
            },
            secret_refs: Default::default(),
            timeout_ms: 1000,
            max_output_bytes: 4096,
        };
        normalize(old.path(), &mut config).unwrap();
        let McpTransport::Stdio { command, args, cwd } = &config.transport else {
            panic!()
        };
        assert_eq!(command, "drive:runtime/server.exe");
        assert_eq!(
            PathBuf::from(resolve(Some(new.path()), command).unwrap()),
            new.path().join("runtime").join("server.exe")
        );
        assert_eq!(args[0], "--config=drive:mcp/server.json");
        assert_eq!(cwd.as_deref(), Some("drive:workspaces/repo"));
    }
}
