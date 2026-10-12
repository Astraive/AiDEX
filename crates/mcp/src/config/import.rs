//! Read-only import of Claude Code style `.mcp.json` files.
//!
//! `{ "mcpServers": { "<name>": { "command", "args", "env", "url", "type", "headers" } } }`
//! entries (for a plugin, also the server map without the `mcpServers` wrapper)
//! become DISABLED server configurations. Environment and header
//! *values* are never kept: each named variable or header becomes a mapping to a
//! suggested vault secret name the user fills in (consumer `mcp:<id>`).

use std::{
    fs,
    io::{ErrorKind, Read},
    path::Path,
};

use serde::Serialize;
use serde_json::{Map, Value};

use super::{
    CONFIG_VERSION, EnvSecret, HeaderSecret, MAX_ID_LEN, McpSource, McpSourceKind,
    RESERVED_HEADERS, ServerConfig, TransportKind,
};
use crate::{McpError, McpErrorKind, redact::bounded_redacted};

/// Largest `.mcp.json` accepted.
pub const MAX_MCP_JSON_BYTES: u64 = 256 * 1024;
/// Most servers one file may declare.
const MAX_IMPORTED: usize = 64;

/// One `.mcp.json` entry. `config` is `None` (with `error`) when the entry cannot
/// become a valid configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ImportedServer {
    /// The entry's key in `mcpServers` (display text only).
    pub original_name: String,
    pub config: Option<ServerConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Environment variables whose values were dropped; each is mapped to a
    /// suggested vault secret in `config.env_from_secrets`.
    pub dropped_env: Vec<String>,
    /// HTTP headers whose values were dropped; mapped like `dropped_env`.
    pub dropped_headers: Vec<String>,
    pub warnings: Vec<String>,
}

/// Parse `path` into disabled configurations attributed to `source`. Nothing is
/// written; symbolic links and oversized files are refused.
pub fn import_mcp_json(path: &Path, source: McpSource) -> Result<Vec<ImportedServer>, McpError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Err(McpError::new(
                McpErrorKind::NotFound,
                "no .mcp.json file was found",
            ));
        }
        Err(error) => return Err(McpError::io("cannot inspect .mcp.json", &error)),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(McpError::invalid_config(
            ".mcp.json must be a regular file, not a link or directory",
        ));
    }
    if metadata.len() > MAX_MCP_JSON_BYTES {
        return Err(McpError::invalid_config(".mcp.json is larger than 256 KiB"));
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|file| file.take(MAX_MCP_JSON_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|error| McpError::io("cannot read .mcp.json", &error))?;
    if bytes.len() as u64 > MAX_MCP_JSON_BYTES {
        return Err(McpError::invalid_config(".mcp.json is larger than 256 KiB"));
    }
    parse_mcp_json(&bytes, &source)
}

fn parse_mcp_json(bytes: &[u8], source: &McpSource) -> Result<Vec<ImportedServer>, McpError> {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let document: Value = serde_json::from_slice(bytes)
        .map_err(|_| McpError::invalid_config(".mcp.json is not valid JSON"))?;
    let Value::Object(top) = &document else {
        return Err(McpError::invalid_config(".mcp.json must be a JSON object"));
    };
    let servers = match top.get("mcpServers") {
        Some(Value::Object(servers)) => servers,
        Some(_) => {
            return Err(McpError::invalid_config(
                ".mcp.json has no `mcpServers` object",
            ));
        }
        // A Claude Code plugin's `.mcp.json` may be the server map itself (the
        // grammar `aidex_plugins` reviews); other sources need the wrapper.
        None if source.kind == McpSourceKind::Plugin => top,
        None => {
            return Err(McpError::invalid_config(
                ".mcp.json has no `mcpServers` object",
            ));
        }
    };
    if servers.len() > MAX_IMPORTED {
        return Err(McpError::invalid_config(format!(
            ".mcp.json declares more than {MAX_IMPORTED} servers"
        )));
    }
    let mut taken: Vec<String> = Vec::new();
    let mut imported = Vec::new();
    for (name, entry) in servers {
        let id = unique(slug_id(name), &taken);
        taken.push(id.clone());
        imported.push(import_entry(name, &id, entry, source));
    }
    Ok(imported)
}

fn import_entry(name: &str, id: &str, entry: &Value, source: &McpSource) -> ImportedServer {
    let mut result = ImportedServer {
        original_name: bounded_redacted(name, 128),
        config: None,
        error: None,
        dropped_env: Vec::new(),
        dropped_headers: Vec::new(),
        warnings: Vec::new(),
    };
    match build(id, name, entry, source, &mut result) {
        Ok(config) => match config.validate() {
            Ok(()) => result.config = Some(config),
            Err(error) => result.error = Some(error.message),
        },
        Err(message) => result.error = Some(message),
    }
    result
}

fn build(
    id: &str,
    name: &str,
    entry: &Value,
    source: &McpSource,
    result: &mut ImportedServer,
) -> Result<ServerConfig, String> {
    let entry = entry
        .as_object()
        .ok_or_else(|| "the entry is not a JSON object".to_owned())?;
    let kind = entry.get("type").and_then(Value::as_str);
    let transport = match kind {
        Some("stdio") => TransportKind::Stdio,
        Some("http" | "streamable-http" | "streamable_http" | "streamableHttp") => {
            TransportKind::Http
        }
        Some("sse") => {
            result.warnings.push(
                "the legacy SSE transport is imported as Streamable HTTP; the server must support POST requests".into(),
            );
            TransportKind::Http
        }
        Some(other) => {
            return Err(format!(
                "transport type `{}` is not supported",
                bounded_redacted(other, 32)
            ));
        }
        None if entry.contains_key("command") => TransportKind::Stdio,
        None if entry.contains_key("url") => TransportKind::Http,
        None => return Err("the entry has neither a command nor a url".into()),
    };
    let display: String = name.chars().filter(|ch| !ch.is_control()).collect();
    let display = display.trim();
    let mut config = ServerConfig {
        version: CONFIG_VERSION,
        id: id.to_owned(),
        name: if display.is_empty() {
            id.to_owned()
        } else {
            display.chars().take(128).collect()
        },
        transport,
        command: None,
        args: Vec::new(),
        env_allowlist: Vec::new(),
        url: None,
        enabled: false,
        workspaces: Vec::new(),
        auto_approve_tools: Vec::new(),
        headers_from_secrets: Vec::new(),
        env_from_secrets: Vec::new(),
        source: source.clone(),
    };
    // A workspace's servers are offered in that workspace only.
    if source.kind == McpSourceKind::Workspace
        && let Some(owner) = &source.owner
    {
        config.workspaces = vec![owner.clone()];
    }
    for key in entry.keys() {
        if !matches!(
            key.as_str(),
            "type" | "command" | "args" | "env" | "url" | "headers"
        ) {
            result
                .warnings
                .push(format!("`{}` is not imported", bounded_redacted(key, 32)));
        }
    }
    match transport {
        TransportKind::Stdio => {
            let command = entry
                .get("command")
                .and_then(Value::as_str)
                .ok_or_else(|| "a stdio entry needs a `command` string".to_owned())?;
            config.command = Some(command.to_owned());
            if let Some(args) = entry.get("args") {
                let args = args
                    .as_array()
                    .ok_or_else(|| "`args` must be an array of strings".to_owned())?;
                for arg in args {
                    config.args.push(
                        arg.as_str()
                            .ok_or_else(|| "`args` must be an array of strings".to_owned())?
                            .to_owned(),
                    );
                }
            }
            for env in names(entry.get("env"), "env")? {
                let mapping = EnvSecret {
                    secret_name: suggested_secret(id, &env),
                    env: env.clone(),
                };
                result.dropped_env.push(env);
                config.env_from_secrets.push(mapping);
            }
            if entry.contains_key("headers") {
                result
                    .warnings
                    .push("`headers` apply only to http servers".into());
            }
        }
        TransportKind::Http => {
            let url = entry
                .get("url")
                .and_then(Value::as_str)
                .ok_or_else(|| "an http entry needs a `url` string".to_owned())?;
            config.url = Some(url.to_owned());
            for header in names(entry.get("headers"), "headers")? {
                if RESERVED_HEADERS.contains(&header.to_ascii_lowercase().as_str()) {
                    result.warnings.push(format!(
                        "header `{header}` is managed by AiDEX and was dropped"
                    ));
                    continue;
                }
                let mapping = HeaderSecret {
                    secret_name: suggested_secret(id, &header),
                    header: header.clone(),
                };
                result.dropped_headers.push(header);
                config.headers_from_secrets.push(mapping);
            }
            if entry.contains_key("command") || entry.contains_key("env") {
                result
                    .warnings
                    .push("`command` and `env` apply only to stdio servers".into());
            }
        }
    }
    if !result.dropped_env.is_empty() || !result.dropped_headers.is_empty() {
        result.warnings.push(
            "values from .mcp.json were not imported; store each mapped secret in the vault with consumer mcp:<id>".into(),
        );
    }
    Ok(config)
}

/// The keys of an optional JSON object of string values (values are dropped).
fn names(value: Option<&Value>, field: &str) -> Result<Vec<String>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let map: &Map<String, Value> = value
        .as_object()
        .ok_or_else(|| format!("`{field}` must be an object"))?;
    Ok(map.keys().cloned().collect())
}

/// A vault secret name suggestion for `variable` of server `id`.
fn suggested_secret(id: &str, variable: &str) -> String {
    let tail: String = variable
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let mut name = format!("mcp-{id}-{}", tail.trim_matches('-'));
    name.truncate(128);
    name.trim_end_matches('-').to_owned()
}

/// A server id derived from a display name: lowercase `[a-z0-9_-]`, starting
/// with a letter or digit, at most 64 characters (`server` when nothing is left).
pub fn slug_id(name: &str) -> String {
    let mut slug = String::new();
    for ch in name.chars() {
        let mapped = if ch.is_ascii_alphanumeric() {
            ch.to_ascii_lowercase()
        } else if ch == '_' {
            '_'
        } else {
            '-'
        };
        if mapped == '-' && slug.ends_with('-') {
            continue;
        }
        slug.push(mapped);
    }
    let slug = slug.trim_start_matches(['-', '_']).trim_end_matches('-');
    let mut slug: String = slug.chars().take(MAX_ID_LEN).collect();
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "server".into()
    } else {
        slug
    }
}

fn unique(base: String, taken: &[String]) -> String {
    if !taken.contains(&base) {
        return base;
    }
    (2..)
        .map(|n| {
            let suffix = format!("-{n}");
            let mut stem = base.clone();
            stem.truncate(MAX_ID_LEN - suffix.len());
            format!("{stem}{suffix}")
        })
        .find(|candidate| !taken.contains(candidate))
        .unwrap_or(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_valid_ids() {
        assert_eq!(slug_id("GitHub"), "github");
        assert_eq!(slug_id("My Server!!"), "my-server");
        assert_eq!(slug_id("--x"), "x");
        assert_eq!(slug_id("日本"), "server");
        assert_eq!(slug_id(&"a".repeat(100)).len(), 64);
        assert_eq!(unique("a".into(), &["a".into(), "a-2".into()]), "a-3");
    }

    #[test]
    fn entries_import_disabled_without_values() {
        let json = br#"{"mcpServers":{
            "GitHub":{"command":"npx","args":["-y","pkg"],"env":{"GITHUB_TOKEN":"ghp_secretvalue123"}},
            "remote":{"type":"http","url":"https://mcp.example.com/mcp","headers":{"Authorization":"Bearer abc","Host":"x"}},
            "legacy":{"type":"sse","url":"https://old.example.com/sse"},
            "broken":{"type":"stdio"},
            "github":{"command":"uvx","cwd":"/tmp"}
        }}"#;
        let servers = parse_mcp_json(json, &McpSource::workspace("ws_1")).unwrap();
        assert_eq!(servers.len(), 5);
        let text = serde_json::to_string(&servers).unwrap();
        assert!(!text.contains("ghp_secretvalue123"));
        assert!(!text.contains("Bearer abc"));
        let github = servers
            .iter()
            .find(|s| s.original_name == "GitHub")
            .unwrap();
        let config = github.config.as_ref().unwrap();
        assert!(!config.enabled);
        assert_eq!(config.source, McpSource::workspace("ws_1"));
        assert_eq!(config.workspaces, ["ws_1"]);
        assert_eq!(github.dropped_env, ["GITHUB_TOKEN"]);
        assert_eq!(
            config.env_from_secrets[0].secret_name,
            "mcp-github-github-token"
        );
        let remote = servers
            .iter()
            .find(|s| s.original_name == "remote")
            .unwrap();
        let config = remote.config.as_ref().unwrap();
        assert_eq!(config.transport, TransportKind::Http);
        assert_eq!(config.headers_from_secrets.len(), 1);
        let legacy = servers
            .iter()
            .find(|s| s.original_name == "legacy")
            .unwrap();
        assert!(legacy.config.is_some());
        assert!(!legacy.warnings.is_empty());
        let broken = servers
            .iter()
            .find(|s| s.original_name == "broken")
            .unwrap();
        assert!(broken.config.is_none() && broken.error.is_some());
        let ids: Vec<&str> = servers
            .iter()
            .filter_map(|s| s.config.as_ref().map(|c| c.id.as_str()))
            .collect();
        assert!(ids.contains(&"github") && ids.contains(&"github-2"));
    }

    #[test]
    fn malformed_documents_are_refused() {
        let source = McpSource::drive();
        assert!(parse_mcp_json(b"not json", &source).is_err());
        assert!(parse_mcp_json(b"{}", &source).is_err());
        assert!(parse_mcp_json(br#"{"mcpServers":[]}"#, &source).is_err());
        let insecure = br#"{"mcpServers":{"x":{"url":"http://example.com/mcp"}}}"#;
        let servers = parse_mcp_json(insecure, &source).unwrap();
        assert!(servers[0].config.is_none());
    }

    #[test]
    fn plugin_files_may_omit_the_wrapper() {
        let bare = b"\xef\xbb\xbf{\"github\": {\"command\": \"npx\", \"args\": [\"-y\", \"pkg\"]}}";
        let servers = parse_mcp_json(bare, &McpSource::plugin("review")).unwrap();
        assert_eq!(servers.len(), 1);
        let config = servers[0].config.as_ref().unwrap();
        assert_eq!(config.command.as_deref(), Some("npx"));
        assert!(!config.enabled);
        // Workspace and drive files keep the Claude Code project format.
        assert!(parse_mcp_json(bare, &McpSource::workspace("ws_1")).is_err());
        assert!(parse_mcp_json(br#"{"mcpServers": []}"#, &McpSource::plugin("review")).is_err());
        assert!(parse_mcp_json(b"[]", &McpSource::plugin("review")).is_err());
    }
}
