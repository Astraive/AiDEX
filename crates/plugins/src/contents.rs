//! What a plugin contains, for the review dialog: skills (`skills/<dir>/SKILL.md`), MCP
//! servers from `.mcp.json` (names, transport, redacted command/arguments/URL, and
//! environment and header *names* — never their values), the README, and Claude Code
//! components AiDEX does not run.

use std::{
    fs::{self, File},
    io::{self, ErrorKind, Read},
    path::Path,
};

use aidex_events::redact_text;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    PluginError,
    fsutil::read_bounded,
    limits::{
        MAX_LISTED_SKILLS, MAX_MCP_JSON_BYTES, MAX_MCP_SERVERS, MAX_README_BYTES,
        MAX_SKILL_HEAD_BYTES, MAX_TEXT_BYTES,
    },
    names::{quote, validate_relative},
    tree::Tree,
};

/// The MCP server file at the plugin root (Claude Code format).
pub const MCP_JSON: &str = ".mcp.json";
/// The skills directory at the plugin root.
pub const SKILLS_DIR: &str = "skills";
/// The skill file name, matched in any ASCII letter case as skill discovery does.
const SKILL_FILE: &str = "skill.md";
const README_FILES: [&str; 3] = ["README.md", "readme.md", "README"];
/// Claude Code plugin components AiDEX never loads or runs.
const UNSUPPORTED_COMPONENTS: [&str; 4] = ["agents", "commands", "hooks", "output-styles"];
const MAX_MCP_TEXT_BYTES: usize = 4096;
const MAX_MCP_ARGS: usize = 64;
const MAX_MCP_KEYS: usize = 64;
/// Command prefixes that name a file inside the plugin.
const PLUGIN_ROOT_VARIABLES: [&str; 2] = ["${CLAUDE_PLUGIN_ROOT}", "${AIDEX_PLUGIN_ROOT}"];

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillEntry {
    /// Directory name under `skills/`.
    pub dir: String,
    /// Plugin-relative path of the skill file.
    pub path: String,
    /// Front-matter `name` (best effort; redacted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Front-matter `description` (best effort; redacted, at most 1 KiB).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpTransport {
    Stdio,
    Http,
    Sse,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpServerEntry {
    pub name: String,
    pub transport: McpTransport,
    /// The command line program (redacted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Arguments (redacted).
    #[serde(default)]
    pub args: Vec<String>,
    /// Remote endpoint (redacted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Environment variable names the server is configured with (values are never read).
    #[serde(default)]
    pub env_keys: Vec<String>,
    /// HTTP header names the server is configured with (values are never read).
    #[serde(default)]
    pub header_keys: Vec<String>,
    /// Plugin-relative path when `command` runs a file shipped in the plugin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundled_command: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginContents {
    pub skills: Vec<SkillEntry>,
    /// More skills exist than are listed.
    #[serde(default)]
    pub skills_truncated: bool,
    pub mcp_servers: Vec<McpServerEntry>,
    /// Manifest keys AiDEX does not support (e.g. `hooks`); nothing they name runs.
    pub ignored_fields: Vec<String>,
    /// Claude Code component directories present but never loaded (`hooks`, ...).
    pub unsupported_components: Vec<String>,
    pub file_count: usize,
    pub total_bytes: u64,
    /// The start of `README.md` (redacted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readme: Option<String>,
    #[serde(default)]
    pub readme_truncated: bool,
}

/// Inspect a validated plugin tree.
pub(crate) fn inspect(
    root: &Path,
    tree: &Tree,
    ignored_fields: Vec<String>,
) -> Result<PluginContents, PluginError> {
    let (skills, skills_truncated) = skills(root)?;
    let (readme, readme_truncated) = readme(root)?;
    let unsupported_components = UNSUPPORTED_COMPONENTS
        .iter()
        .filter(|name| tree.dirs.iter().any(|dir| dir == *name))
        .map(|name| (*name).to_owned())
        .collect();
    Ok(PluginContents {
        skills,
        skills_truncated,
        mcp_servers: read_mcp_json(root)?,
        ignored_fields,
        unsupported_components,
        file_count: tree.files.len(),
        total_bytes: tree.total_bytes,
        readme,
        readme_truncated,
    })
}

/// The servers of `<root>/.mcp.json` (none when the file is absent).
pub(crate) fn read_mcp_json(root: &Path) -> Result<Vec<McpServerEntry>, PluginError> {
    let bytes = read_bounded(&root.join(MCP_JSON), MAX_MCP_JSON_BYTES)
        .map_err(|error| mcp_invalid(&error.to_string()))?;
    bytes.map_or_else(|| Ok(Vec::new()), |bytes| parse_mcp_json(&bytes))
}

fn mcp_invalid(message: &str) -> PluginError {
    PluginError::InvalidManifest(format!("`{MCP_JSON}`: {message}"))
}

/// Parse Claude Code `.mcp.json`: `{"mcpServers": {name: {command, args, env, url,
/// headers, type}}}`, or the same server map without the `mcpServers` wrapper.
pub fn parse_mcp_json(bytes: &[u8]) -> Result<Vec<McpServerEntry>, PluginError> {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let value: Value = serde_json::from_slice(bytes).map_err(|error| {
        mcp_invalid(&format!(
            "not valid JSON (line {}, column {})",
            error.line(),
            error.column()
        ))
    })?;
    let Value::Object(top) = value else {
        return Err(mcp_invalid("must be a JSON object"));
    };
    let servers = match top.get("mcpServers") {
        Some(Value::Object(servers)) => servers,
        Some(_) => return Err(mcp_invalid("`mcpServers` must be an object")),
        None => &top,
    };
    if servers.len() > MAX_MCP_SERVERS {
        return Err(mcp_invalid(&format!(
            "declares more than {MAX_MCP_SERVERS} servers"
        )));
    }
    let mut entries = servers
        .iter()
        .map(|(name, config)| server(name, config))
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

fn server(name: &str, config: &Value) -> Result<McpServerEntry, PluginError> {
    if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
        return Err(mcp_invalid(
            "server names must be 1-128 characters without control characters",
        ));
    }
    let invalid = |message: &str| mcp_invalid(&format!("server {}: {message}", quote(name)));
    let Value::Object(config) = config else {
        return Err(invalid("must be an object"));
    };
    let string = |key: &str| -> Result<Option<String>, PluginError> {
        match config.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(text))
                if text.len() <= MAX_MCP_TEXT_BYTES && !text.chars().any(char::is_control) =>
            {
                Ok(Some(text.clone()))
            }
            Some(_) => Err(invalid(&format!(
                "`{key}` must be a string of at most {MAX_MCP_TEXT_BYTES} bytes"
            ))),
        }
    };
    let command = string("command")?;
    let url = string("url")?;
    let transport = match string("type")?.as_deref() {
        Some("stdio") => McpTransport::Stdio,
        Some("http" | "streamable-http") => McpTransport::Http,
        Some("sse") => McpTransport::Sse,
        Some(_) => return Err(invalid("`type` must be `stdio`, `http`, or `sse`")),
        None if command.is_some() => McpTransport::Stdio,
        None if url.is_some() => McpTransport::Http,
        None => return Err(invalid("needs a `command` or a `url`")),
    };
    match transport {
        McpTransport::Stdio if command.is_none() => {
            return Err(invalid("a stdio server needs a `command`"));
        }
        McpTransport::Http | McpTransport::Sse if url.is_none() => {
            return Err(invalid("an HTTP server needs a `url`"));
        }
        _ => {}
    }
    let args = match config.get("args") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(args)) if args.len() <= MAX_MCP_ARGS => args
            .iter()
            .map(|arg| match arg {
                // Arguments may hold tabs or line breaks (`-c` scripts); the review
                // shows them with control characters replaced (see `display`).
                Value::String(arg) if arg.len() <= MAX_MCP_TEXT_BYTES => Ok(display(arg)),
                _ => Err(invalid(&format!(
                    "`args` must be strings of at most {MAX_MCP_TEXT_BYTES} bytes"
                ))),
            })
            .collect::<Result<_, _>>()?,
        Some(_) => {
            return Err(invalid(&format!(
                "`args` must be a list of at most {MAX_MCP_ARGS} strings"
            )));
        }
    };
    Ok(McpServerEntry {
        name: name.to_owned(),
        transport,
        bundled_command: command.as_deref().and_then(bundled_path),
        command: command.as_deref().map(display),
        args,
        url: url.as_deref().map(display),
        env_keys: key_names(config.get("env"), "env", &invalid)?,
        header_keys: key_names(config.get("headers"), "headers", &invalid)?,
    })
}

/// The names of an object's keys (sorted); the values are never inspected.
fn key_names(
    value: Option<&Value>,
    key: &str,
    invalid: &dyn Fn(&str) -> PluginError,
) -> Result<Vec<String>, PluginError> {
    let map: &Map<String, Value> = match value {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Object(map)) if map.len() <= MAX_MCP_KEYS => map,
        Some(_) => {
            return Err(invalid(&format!(
                "`{key}` must be an object with at most {MAX_MCP_KEYS} entries"
            )));
        }
    };
    let mut names: Vec<String> = map
        .keys()
        .map(|name| {
            let valid = !name.is_empty()
                && name.len() <= 128
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
            if valid {
                Ok(name.clone())
            } else {
                Err(invalid(&format!(
                    "`{key}` names must be 1-128 characters of A-Z, a-z, 0-9, `_`, `-`, and `.`"
                )))
            }
        })
        .collect::<Result<_, _>>()?;
    names.sort();
    Ok(names)
}

/// The plugin-relative file a command runs, when it names one inside the plugin:
/// `${CLAUDE_PLUGIN_ROOT}/bin/server`, `${AIDEX_PLUGIN_ROOT}/...`, or `./bin/server`.
pub(crate) fn bundled_path(command: &str) -> Option<String> {
    let command = command.trim();
    let relative = PLUGIN_ROOT_VARIABLES
        .iter()
        .find_map(|variable| command.strip_prefix(variable))
        .map(|rest| rest.trim_start_matches(['/', '\\']))
        .or_else(|| {
            command
                .strip_prefix("./")
                .or_else(|| command.strip_prefix(".\\"))
        })?;
    let relative = relative.replace('\\', "/");
    validate_relative(&relative).ok().map(|()| relative)
}

/// Redacted, bounded text for display. Control characters (terminal escapes, carriage
/// returns, ...) become U+FFFD so plugin text cannot drive a terminal or hide lines.
fn display(text: &str) -> String {
    let visible: String = redact_text(text)
        .chars()
        .map(|c| if c.is_control() { '\u{fffd}' } else { c })
        .collect();
    truncate(&visible, MAX_TEXT_BYTES)
}

fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// Skill directories under `skills/` that hold a skill file, chosen exactly as skill
/// discovery (`aidex_skills`) chooses it, so the review lists every skill that loads:
/// hidden directories are skipped, and the file is `SKILL.md`, else `skill.md`, else
/// another letter case of it.
fn skills(root: &Path) -> Result<(Vec<SkillEntry>, bool), PluginError> {
    let dir = root.join(SKILLS_DIR);
    if !is_real_dir(&dir) {
        return Ok((Vec::new(), false));
    }
    let mut names = sorted_names(&dir)?;
    names.retain(|name| !name.starts_with('.') && is_real_dir(&dir.join(name)));
    let mut skills = Vec::new();
    let mut truncated = false;
    for name in names {
        let skill_dir = dir.join(&name);
        let present = sorted_names(&skill_dir)?;
        let Some(file) = skill_file(&present) else {
            continue;
        };
        if skills.len() == MAX_LISTED_SKILLS {
            truncated = true;
            break;
        }
        let head = read_head(&skill_dir.join(file), MAX_SKILL_HEAD_BYTES)
            .map_err(|error| PluginError::io("read", &skill_dir.join(file), &error))?;
        let (skill_name, description) = head
            .map(|(bytes, _)| front_matter(&String::from_utf8_lossy(&bytes)))
            .unwrap_or_default();
        skills.push(SkillEntry {
            path: format!("{SKILLS_DIR}/{name}/{file}"),
            dir: name,
            name: skill_name,
            description,
        });
    }
    Ok((skills, truncated))
}

/// The skill file among a directory's entry names: `SKILL.md`, else `skill.md`, else
/// any other ASCII letter case of `skill.md` (the first in name order).
fn skill_file(present: &[String]) -> Option<&str> {
    present
        .iter()
        .filter(|entry| entry.eq_ignore_ascii_case(SKILL_FILE))
        .min_by_key(|entry| match entry.as_str() {
            "SKILL.md" => 0,
            "skill.md" => 1,
            _ => 2,
        })
        .map(String::as_str)
}

/// Best-effort `name` and `description` scalars from YAML front matter.
fn front_matter(text: &str) -> (Option<String>, Option<String>) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return (None, None);
    }
    let (mut name, mut description) = (None, None);
    for line in lines {
        if line.trim_end() == "---" {
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = unquote(value.trim());
        if value.is_empty() || matches!(value, "|" | ">" | "|-" | ">-") {
            continue;
        }
        match key.trim() {
            "name" => name = Some(display(value)),
            "description" => description = Some(display(value)),
            _ => {}
        }
    }
    (name, description)
}

fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}

fn readme(root: &Path) -> Result<(Option<String>, bool), PluginError> {
    let present = sorted_names(root)?;
    let Some(file) = README_FILES
        .iter()
        .find(|file| present.iter().any(|name| name == *file))
    else {
        return Ok((None, false));
    };
    let path = root.join(file);
    let Some((bytes, truncated)) = read_head(&path, MAX_README_BYTES as u64)
        .map_err(|e| PluginError::io("read", &path, &e))?
    else {
        return Ok((None, false));
    };
    let text = String::from_utf8_lossy(&bytes);
    // A cut multi-byte character becomes U+FFFD; drop it.
    let text = text.trim_end_matches('\u{fffd}').replace("\r\n", "\n");
    // Keep line breaks and tabs; other control characters (terminal escapes, lone
    // carriage returns) become U+FFFD.
    let text: String = redact_text(&text)
        .chars()
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                '\u{fffd}'
            } else {
                c
            }
        })
        .collect();
    Ok((Some(text), truncated))
}

/// Up to `max` leading bytes of a regular file and whether more exist.
fn read_head(path: &Path, max: u64) -> io::Result<Option<(Vec<u8>, bool)>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => Ok(None),
        Ok(_) => {
            let mut bytes = Vec::new();
            File::open(path)?.take(max + 1).read_to_end(&mut bytes)?;
            let truncated = bytes.len() as u64 > max;
            bytes.truncate(usize::try_from(max).unwrap_or(usize::MAX));
            Ok(Some((bytes, truncated)))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn is_real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
}

/// Entry names of a directory (valid Unicode only), sorted.
fn sorted_names(dir: &Path) -> Result<Vec<String>, PluginError> {
    let entries = fs::read_dir(dir).map_err(|error| PluginError::io("list", dir, &error))?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| PluginError::io("list", dir, &error))?;
        if let Ok(name) = entry.file_name().into_string() {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn mcp_servers_list_commands_without_secret_values() {
        let servers = parse_mcp_json(
            json!({"mcpServers": {
                "db": {
                    "command": "${CLAUDE_PLUGIN_ROOT}/bin/db-server.exe",
                    "args": ["--config", "${CLAUDE_PLUGIN_ROOT}/config.json", "--api-key=sk-live-abcdefghijklmnopqrstuvwxyz0123456789"],
                    "env": {"DB_TOKEN": "ghp_abcdefghijklmnopqrstuvwxyz0123456789", "DB_PATH": "x"}
                },
                "remote": {"type": "http", "url": "https://user:hunter2@mcp.example.com/mcp", "headers": {"Authorization": "Bearer abc.def.ghi"}},
                "node": {"command": "node", "args": ["./server.js"]}
            }})
            .to_string()
            .as_bytes(),
        )
        .unwrap();
        let text = serde_json::to_string(&servers).unwrap();
        for secret in ["sk-live-abc", "ghp_abc", "hunter2", "Bearer abc"] {
            assert!(!text.contains(secret), "{secret} leaked: {text}");
        }
        assert_eq!(
            servers.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["db", "node", "remote"]
        );
        let db = &servers[0];
        assert_eq!(db.transport, McpTransport::Stdio);
        assert_eq!(db.bundled_command.as_deref(), Some("bin/db-server.exe"));
        assert_eq!(db.env_keys, ["DB_PATH", "DB_TOKEN"]);
        assert_eq!(servers[1].bundled_command, None);
        assert_eq!(servers[2].transport, McpTransport::Http);
        assert_eq!(servers[2].header_keys, ["Authorization"]);

        let bare = parse_mcp_json(br#"{"x": {"command": "./run.cmd"}}"#).unwrap();
        assert_eq!(bare[0].bundled_command.as_deref(), Some("run.cmd"));
        // Control characters cannot disguise what the review shows.
        let escapes = json!({"mcpServers": {"x": {
            "command": "sh",
            "args": ["-c", "evil\r  x (stdio): npx\u{1b}[2K", "a\tb"]
        }}});
        let shown = parse_mcp_json(escapes.to_string().as_bytes()).unwrap();
        assert_eq!(
            shown[0].args,
            [
                "-c",
                "evil\u{fffd}  x (stdio): npx\u{fffd}[2K",
                "a\u{fffd}b"
            ]
        );
        for bad in [
            &br#"[]"#[..],
            br#"{"mcpServers": []}"#,
            br#"{"mcpServers": {"x": {}}}"#,
            br#"{"mcpServers": {"x": {"type": "stdio", "url": "https://a"}}}"#,
            br#"{"mcpServers": {"x": {"command": "a", "type": "carrier-pigeon"}}}"#,
            br#"{"mcpServers": {"x": {"command": "a", "args": [1]}}}"#,
            br#"{"mcpServers": {"x": {"command": "a", "env": {"BAD KEY": "v"}}}}"#,
            br#"{"mcpServers": {"x": {"command": "a""#,
        ] {
            assert_eq!(
                parse_mcp_json(bad).unwrap_err().reason(),
                "invalid_manifest",
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
    }

    #[test]
    fn bundled_paths_stay_inside_the_plugin() {
        assert_eq!(
            bundled_path("${CLAUDE_PLUGIN_ROOT}\\bin\\x.exe").as_deref(),
            Some("bin/x.exe")
        );
        assert_eq!(bundled_path("${AIDEX_PLUGIN_ROOT}/x").as_deref(), Some("x"));
        assert_eq!(bundled_path("node"), None);
        assert_eq!(bundled_path("/usr/bin/node"), None);
        assert_eq!(bundled_path("${CLAUDE_PLUGIN_ROOT}/../escape.exe"), None);
        assert_eq!(bundled_path("./a/../../b"), None);
    }

    #[test]
    fn front_matter_is_read_best_effort() {
        assert_eq!(
            front_matter(
                "---\nname: review\ndescription: \"Checks code\"\nallowed-tools: [a]\n---\nbody"
            ),
            (Some("review".into()), Some("Checks code".into()))
        );
        assert_eq!(front_matter("no front matter"), (None, None));
        assert_eq!(
            front_matter("---\ndescription: >\n  folded\n---\n"),
            (None, None)
        );
        assert_eq!(
            front_matter("---\nname: a\u{1b}[2Kb\ndescription: x\u{7}y\n---\n"),
            (Some("a\u{fffd}[2Kb".into()), Some("x\u{fffd}y".into()))
        );
    }

    #[test]
    fn skill_files_match_any_letter_case() {
        let names = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            skill_file(&names(&["Skill.md", "notes.md"])),
            Some("Skill.md")
        );
        assert_eq!(
            skill_file(&names(&["SKILL.md", "skill.md"])),
            Some("SKILL.md")
        );
        assert_eq!(
            skill_file(&names(&["Skill.md", "skill.md"])),
            Some("skill.md")
        );
        assert_eq!(skill_file(&names(&["README.md", "skill.mdx"])), None);
    }
}
