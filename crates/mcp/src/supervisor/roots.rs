//! Where an imported stdio server's paths point, worked out at launch. The
//! stored configuration keeps them as written, so a portable drive keeps working
//! under another drive letter and a plugin's trusted files are never rewritten.
//!
//! - Plugin servers name bundled files as Claude Code plugins do, and as the
//!   plugin's install review reads them: `${CLAUDE_PLUGIN_ROOT}/...` or
//!   `${AIDEX_PLUGIN_ROOT}/...` in the command or the arguments, or a command
//!   starting `./`. They resolve inside `<plugins>/<plugin name>`, and both
//!   variables are set in the server's environment. Any other relative command
//!   (`bin/server`) is refused, since the review did not treat it as bundled.
//! - A plugin's files are trusted by their tree hash, so its servers run in
//!   their scratch folder and Python is told not to write bytecode caches next
//!   to the plugin's modules. Nothing else stops a server writing into the
//!   plugin's folder (the sandbox level confines no files); if one does, the
//!   plugin loses its trust and contributes nothing until it is trusted again.
//! - Workspace servers run in their workspace's root, as Claude Code runs a
//!   project's `.mcp.json` servers, and a relative command resolves there.
//! - A relative command or plugin-root path that climbs out of its root is
//!   refused.

use std::{
    fmt, fs,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use crate::{McpError, McpErrorKind, McpSourceKind, ServerConfig, redact::bounded_redacted};

/// The variables a plugin's `.mcp.json` uses for the plugin's own folder.
const PLUGIN_ROOT_VARIABLES: [&str; 2] = ["${CLAUDE_PLUGIN_ROOT}", "${AIDEX_PLUGIN_ROOT}"];
/// Environment variables set to the plugin's folder for its servers.
const PLUGIN_ROOT_ENV: [&str; 2] = ["CLAUDE_PLUGIN_ROOT", "AIDEX_PLUGIN_ROOT"];
/// Keeps Python from writing `__pycache__` into the plugin's hashed files.
const NO_BYTECODE_ENV: (&str, &str) = ("PYTHONDONTWRITEBYTECODE", "1");

/// The root folder of a workspace by id; `None` when it is unknown or
/// unavailable.
pub(super) type WorkspaceRoots = Arc<dyn Fn(&str) -> Option<PathBuf> + Send + Sync>;

/// The folders imported servers' paths resolve against.
#[derive(Clone, Default)]
pub(super) struct SourceRoots {
    /// Plugin `<name>` is installed in `<plugins>/<name>` (`DrivePaths::plugins()`).
    pub plugins: Option<PathBuf>,
    pub workspaces: Option<WorkspaceRoots>,
}

impl fmt::Debug for SourceRoots {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SourceRoots")
            .field("plugins", &self.plugins)
            .field("workspaces", &self.workspaces.is_some())
            .finish()
    }
}

/// A stdio server's program, arguments, working folder, and extra environment
/// once its source's paths are resolved.
#[derive(Debug, Eq, PartialEq)]
pub(super) struct Launch {
    /// As written (looked up on the sandbox PATH), or an absolute path.
    pub program: String,
    pub args: Vec<String>,
    /// The folder the server runs in; `None` runs it in its scratch folder.
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
}

impl Launch {
    fn as_written(command: &str, config: &ServerConfig) -> Self {
        Self {
            program: command.to_owned(),
            args: config.args.clone(),
            cwd: None,
            env: Vec::new(),
        }
    }
}

impl SourceRoots {
    /// How to launch `config`, whose command is `command`.
    pub fn launch(&self, config: &ServerConfig, command: &str) -> Result<Launch, McpError> {
        let owner = config.source.owner.as_deref().unwrap_or_default();
        match config.source.kind {
            McpSourceKind::Drive => Ok(Launch::as_written(command, config)),
            McpSourceKind::Plugin => self.plugin(owner, config, command),
            McpSourceKind::Workspace => self.workspace(owner, config, command),
        }
    }

    fn plugin(&self, name: &str, config: &ServerConfig, command: &str) -> Result<Launch, McpError> {
        let bundled = plugin_file(command);
        if bundled.is_none() && relative_command(command).is_some() {
            return Err(McpError::invalid_config(format!(
                "the plugin server command `{}` is a relative path; a file the plugin ships is \
                 named `${{CLAUDE_PLUGIN_ROOT}}/...` or `./...`",
                bounded_redacted(command, 200)
            )));
        }
        let root = self
            .plugins
            .as_ref()
            .filter(|_| is_single_name(name))
            .and_then(|plugins| Root::open(&plugins.join(name)));
        let Some(root) = root else {
            let needs_root =
                bundled.is_some() || config.args.iter().any(|arg| mentions_plugin_root(arg));
            if needs_root {
                return Err(McpError::new(
                    McpErrorKind::LaunchFailed,
                    format!(
                        "the folder of plugin `{}` is not available, so its bundled server cannot start",
                        bounded_redacted(name, 128)
                    ),
                ));
            }
            return Ok(Launch::as_written(command, config));
        };
        let program = match &bundled {
            Some(relative) => root.file(relative, command, "plugin")?,
            None => command.to_owned(),
        };
        let args = config
            .args
            .iter()
            .map(|arg| root.expand(arg))
            .collect::<Result<Vec<_>, _>>()?;
        let env = PLUGIN_ROOT_ENV
            .iter()
            .map(|name| (*name, root.text.as_str()))
            .chain([NO_BYTECODE_ENV])
            // A secret mapped to the same name wins.
            .filter(|(name, _)| {
                !config
                    .env_from_secrets
                    .iter()
                    .any(|mapping| mapping.env.eq_ignore_ascii_case(name))
            })
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .collect();
        Ok(Launch {
            program,
            args,
            cwd: None,
            env,
        })
    }

    fn workspace(
        &self,
        id: &str,
        config: &ServerConfig,
        command: &str,
    ) -> Result<Launch, McpError> {
        let Some(resolve) = &self.workspaces else {
            return Ok(Launch::as_written(command, config));
        };
        let root = resolve(id)
            .and_then(|root| Root::open(&root))
            .ok_or_else(|| {
                McpError::new(
                    McpErrorKind::LaunchFailed,
                    format!(
                        "workspace `{}` of this server is not available; reconnect its folder or remove the server",
                        bounded_redacted(id, 128)
                    ),
                )
            })?;
        let program = match relative_command(command) {
            Some(relative) => root.file(relative, command, "workspace")?,
            None => command.to_owned(),
        };
        Ok(Launch {
            program,
            args: config.args.clone(),
            cwd: Some(root.path),
            env: Vec::new(),
        })
    }
}

/// A root folder, canonicalized.
struct Root {
    /// Canonical (verbatim on Windows), for containment checks.
    canonical: PathBuf,
    /// The folder as the server sees it (no `\\?\` prefix on local drives).
    path: PathBuf,
    text: String,
}

impl Root {
    fn open(dir: &Path) -> Option<Self> {
        let canonical = fs::canonicalize(dir).ok()?;
        if !fs::metadata(&canonical).ok()?.is_dir() {
            return None;
        }
        let path = simplify(canonical.clone());
        let text = path.to_str()?.to_owned();
        Some(Self {
            canonical,
            path,
            text,
        })
    }

    /// The file `relative` names inside the root, as an absolute path.
    fn file(&self, relative: &str, command: &str, what: &str) -> Result<String, McpError> {
        let leaves = || {
            McpError::invalid_config(format!(
                "the server command `{}` leaves its {what} folder",
                bounded_redacted(command, 200)
            ))
        };
        let not_found = || {
            McpError::new(
                McpErrorKind::LaunchFailed,
                format!(
                    "`{}` was not found in its {what} folder",
                    bounded_redacted(command, 200)
                ),
            )
        };
        let relative = Path::new(relative);
        let mut names_a_file = false;
        for component in relative.components() {
            match component {
                Component::Normal(_) => names_a_file = true,
                Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(leaves());
                }
            }
        }
        if !names_a_file {
            return Err(not_found());
        }
        let resolved = fs::canonicalize(self.canonical.join(relative)).map_err(|_| not_found())?;
        // A link inside the root may still point outside it.
        if !resolved.starts_with(&self.canonical) {
            return Err(leaves());
        }
        if !fs::metadata(&resolved).is_ok_and(|metadata| metadata.is_file()) {
            return Err(not_found());
        }
        simplify(resolved)
            .into_os_string()
            .into_string()
            .map_err(|_| not_found())
    }

    /// `arg` with every plugin-root variable replaced by the root. A path after
    /// a variable that climbs out of the root is refused.
    fn expand(&self, arg: &str) -> Result<String, McpError> {
        if !mentions_plugin_root(arg) {
            return Ok(arg.to_owned());
        }
        for variable in PLUGIN_ROOT_VARIABLES {
            for (index, _) in arg.match_indices(variable) {
                let path = arg[index + variable.len()..]
                    .split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ',' | ';' | '='))
                    .next()
                    .unwrap_or_default();
                if path.split(['/', '\\']).any(|segment| segment == "..") {
                    return Err(McpError::invalid_config(format!(
                        "the server argument `{}` leaves its plugin folder",
                        bounded_redacted(arg, 200)
                    )));
                }
            }
        }
        let mut expanded = arg.to_owned();
        for variable in PLUGIN_ROOT_VARIABLES {
            expanded = expanded.replace(variable, &self.text);
        }
        Ok(expanded)
    }
}

/// The plugin-relative file a plugin server's command runs, named as the
/// install review (`aidex_plugins`) reads it: the rest after a plugin-root
/// variable or after `./`, with `/` separators.
fn plugin_file(command: &str) -> Option<String> {
    PLUGIN_ROOT_VARIABLES
        .iter()
        .find_map(|variable| command.strip_prefix(variable))
        .map(|rest| rest.trim_start_matches(['/', '\\']))
        .or_else(|| {
            command
                .strip_prefix("./")
                .or_else(|| command.strip_prefix(".\\"))
        })
        .map(|relative| relative.replace('\\', "/"))
}

/// A command that is a relative path with a separator (`./bin/server`,
/// `bin/server`), which names a file in its source's root. Bare program names
/// (`node`) are looked up on the PATH, and absolute paths are used as they are.
fn relative_command(command: &str) -> Option<&str> {
    let path = Path::new(command);
    let anchored = path
        .components()
        .any(|component| matches!(component, Component::Prefix(_) | Component::RootDir));
    let separated = command.contains('/') || command.contains('\\');
    (separated && !anchored && path.is_relative()).then_some(command)
}

fn mentions_plugin_root(text: &str) -> bool {
    PLUGIN_ROOT_VARIABLES
        .iter()
        .any(|variable| text.contains(variable))
}

/// Whether `name` is one plain folder name.
fn is_single_name(name: &str) -> bool {
    let mut components = Path::new(name).components();
    matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(_)), None)
    ) && !name.contains(['/', '\\'])
}

/// Remove a `\\?\` prefix from a local drive path (programs such as `cmd.exe`
/// reject verbatim paths). UNC and device paths keep it.
fn simplify(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(rest) = path.to_str().and_then(|text| text.strip_prefix(r"\\?\")) {
            let bytes = rest.as_bytes();
            let is_drive_path = bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && bytes[2] == b'\\';
            if is_drive_path && rest.len() < 260 {
                return PathBuf::from(rest);
            }
        }
        path
    }
    #[cfg(not(windows))]
    {
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EnvSecret, McpSource, ServerSpec, TransportKind};

    fn config(source: McpSource, command: &str, args: &[&str]) -> ServerConfig {
        ServerSpec {
            id: "server".into(),
            name: None,
            transport: TransportKind::Stdio,
            command: Some(command.into()),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            env_allowlist: Vec::new(),
            url: None,
            headers_from_secrets: Vec::new(),
            env_from_secrets: Vec::new(),
            enabled: true,
            workspaces: Vec::new(),
            auto_approve_tools: Vec::new(),
        }
        .into_config(source)
    }

    fn touch(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"x").unwrap();
    }

    fn launch(roots: &SourceRoots, config: &ServerConfig) -> Result<Launch, McpError> {
        roots.launch(config, config.command.as_deref().unwrap())
    }

    #[test]
    fn plugin_paths_resolve_inside_the_plugin_folder() {
        let drive = tempfile::tempdir().unwrap();
        let plugins = drive.path().join("plugins");
        touch(&plugins.join("kit/bin/server"));
        touch(&plugins.join("kit/servers/echo.py"));
        touch(&drive.path().join("outside"));
        let roots = SourceRoots {
            plugins: Some(plugins.clone()),
            workspaces: None,
        };
        let root = Root::open(&plugins.join("kit")).unwrap();
        let server = simplify(fs::canonicalize(plugins.join("kit/bin/server")).unwrap());
        let server = server.to_str().unwrap();

        let bundled = config(
            McpSource::plugin("kit"),
            "${CLAUDE_PLUGIN_ROOT}/bin/server",
            &["${AIDEX_PLUGIN_ROOT}/servers/echo.py", "--mode=normal"],
        );
        let launched = launch(&roots, &bundled).unwrap();
        assert_eq!(launched.program, server);
        assert_eq!(
            launched.args,
            [
                format!("{}/servers/echo.py", root.text),
                "--mode=normal".into()
            ]
        );
        assert_eq!(launched.cwd, None);
        assert_eq!(
            launched.env,
            [
                ("CLAUDE_PLUGIN_ROOT".to_owned(), root.text.clone()),
                ("AIDEX_PLUGIN_ROOT".to_owned(), root.text.clone()),
                ("PYTHONDONTWRITEBYTECODE".to_owned(), "1".to_owned()),
            ]
        );
        for relative in [
            "./bin/server",
            ".\\bin\\server",
            "${AIDEX_PLUGIN_ROOT}\\bin\\server",
        ] {
            let launched =
                launch(&roots, &config(McpSource::plugin("kit"), relative, &[])).unwrap();
            assert_eq!(launched.program, server, "{relative}");
        }
        // Interpreters on the PATH keep their name; a secret keeps its variable.
        let mut python = config(
            McpSource::plugin("kit"),
            "python",
            &["${CLAUDE_PLUGIN_ROOT}/servers/echo.py"],
        );
        python.env_from_secrets = vec![EnvSecret {
            env: "AIDEX_PLUGIN_ROOT".into(),
            secret_name: "kit-root".into(),
        }];
        let launched = launch(&roots, &python).unwrap();
        assert_eq!(launched.program, "python");
        let names: Vec<&str> = launched.env.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["CLAUDE_PLUGIN_ROOT", "PYTHONDONTWRITEBYTECODE"]);

        // A relative command the install review did not read as bundled.
        let unreviewed =
            launch(&roots, &config(McpSource::plugin("kit"), "bin/server", &[])).unwrap_err();
        assert_eq!(unreviewed.kind, McpErrorKind::InvalidConfig);
        assert!(
            unreviewed.message.contains("relative path"),
            "{unreviewed:?}"
        );

        // Paths that climb out of the plugin folder are refused.
        for (command, args) in [
            ("${CLAUDE_PLUGIN_ROOT}/../outside", vec![]),
            ("./../outside", vec![]),
            ("./bin/../../outside", vec![]),
            ("python", vec!["${CLAUDE_PLUGIN_ROOT}/../outside"]),
            ("python", vec!["--config=${AIDEX_PLUGIN_ROOT}\\..\\outside"]),
        ] {
            let error =
                launch(&roots, &config(McpSource::plugin("kit"), command, &args)).unwrap_err();
            assert_eq!(
                error.kind,
                McpErrorKind::InvalidConfig,
                "{command} {args:?}"
            );
        }
        let missing = launch(
            &roots,
            &config(
                McpSource::plugin("kit"),
                "${CLAUDE_PLUGIN_ROOT}/bin/none",
                &[],
            ),
        )
        .unwrap_err();
        assert_eq!(missing.kind, McpErrorKind::LaunchFailed);
        let folder = launch(
            &roots,
            &config(McpSource::plugin("kit"), "${CLAUDE_PLUGIN_ROOT}/bin", &[]),
        )
        .unwrap_err();
        assert_eq!(folder.kind, McpErrorKind::LaunchFailed);

        // A plugin that is not installed cannot start a bundled server.
        let gone = launch(
            &roots,
            &config(
                McpSource::plugin("gone"),
                "${CLAUDE_PLUGIN_ROOT}/bin/server",
                &[],
            ),
        )
        .unwrap_err();
        assert_eq!(gone.kind, McpErrorKind::LaunchFailed);
        let escape = launch(
            &roots,
            &config(
                McpSource::plugin(".."),
                "${CLAUDE_PLUGIN_ROOT}/outside",
                &[],
            ),
        )
        .unwrap_err();
        assert_eq!(escape.kind, McpErrorKind::LaunchFailed);
        let unbundled = launch(&roots, &config(McpSource::plugin("gone"), "npx", &["-y"])).unwrap();
        assert_eq!(unbundled.program, "npx");

        // Servers the user configured are launched exactly as written.
        let drive_server = config(McpSource::drive(), "${CLAUDE_PLUGIN_ROOT}/bin/server", &[]);
        let launched = launch(&roots, &drive_server).unwrap();
        assert_eq!(launched.program, "${CLAUDE_PLUGIN_ROOT}/bin/server");
        assert!(launched.env.is_empty());
    }

    #[test]
    fn workspace_servers_run_in_their_workspace_root() {
        let drive = tempfile::tempdir().unwrap();
        let workspace = drive.path().join("project");
        touch(&workspace.join("tools/server"));
        touch(&drive.path().join("outside"));
        let known = workspace.clone();
        let roots = SourceRoots {
            plugins: None,
            workspaces: Some(Arc::new(move |id: &str| {
                (id == "ws_1").then(|| known.clone())
            })),
        };
        let root = Root::open(&workspace).unwrap();

        let script = config(McpSource::workspace("ws_1"), "python", &["scripts/mcp.py"]);
        let launched = launch(&roots, &script).unwrap();
        assert_eq!(launched.program, "python");
        assert_eq!(launched.args, ["scripts/mcp.py"]);
        assert_eq!(launched.cwd.as_deref(), Some(root.path.as_path()));
        assert!(launched.env.is_empty());

        let bundled = launch(
            &roots,
            &config(McpSource::workspace("ws_1"), "./tools/server", &[]),
        )
        .unwrap();
        let server = simplify(fs::canonicalize(workspace.join("tools/server")).unwrap());
        assert_eq!(bundled.program, server.to_str().unwrap());
        let escape = launch(
            &roots,
            &config(McpSource::workspace("ws_1"), "../outside", &[]),
        )
        .unwrap_err();
        assert_eq!(escape.kind, McpErrorKind::InvalidConfig);
        // Plugin variables mean nothing in a workspace file.
        let literal = launch(
            &roots,
            &config(
                McpSource::workspace("ws_1"),
                "python",
                &["${CLAUDE_PLUGIN_ROOT}/x"],
            ),
        )
        .unwrap();
        assert_eq!(literal.args, ["${CLAUDE_PLUGIN_ROOT}/x"]);

        let unknown =
            launch(&roots, &config(McpSource::workspace("ws_2"), "python", &[])).unwrap_err();
        assert_eq!(unknown.kind, McpErrorKind::LaunchFailed);
        // Without a workspace lookup (embedders), the server runs as written.
        let launched = launch(
            &SourceRoots::default(),
            &config(McpSource::workspace("ws_1"), "python", &[]),
        )
        .unwrap();
        assert_eq!(launched.cwd, None);
    }
}
