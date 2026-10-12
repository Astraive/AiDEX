//! MCP server tools as dynamic model tools named `mcp__<server>__<tool>`.
use super::MCP_PREFIX;
use crate::extensions::McpToolset;
use serde_json::Value;
use std::collections::BTreeSet;

/// Longest model tool name providers accept.
const MAX_NAME: usize = 64;
/// Most MCP tools offered in one request (the request cap is 256 tools).
pub(crate) const MAX_MCP_TOOLS: usize = 128;
/// Longest tool description sent to the model.
const MAX_DESCRIPTION: usize = 1024;

/// One MCP tool bound to a model-facing name.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct McpBinding {
    /// `mcp__<server>__<tool>`, sanitised to `[A-Za-z0-9_-]{1,64}`.
    pub name: String,
    pub server_id: String,
    pub server_name: String,
    /// The tool's own name on the server.
    pub tool: String,
    pub description: String,
    pub schema: Value,
    /// Listed in the server's `auto_approve_tools`.
    pub auto_approve: bool,
}

/// The server tool one turn offered under a model-facing name. Names depend
/// on which servers are listed, so calls resolve against what was offered.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct McpTarget {
    pub name: String,
    pub server_id: String,
    pub tool: String,
}

impl From<&McpBinding> for McpTarget {
    fn from(binding: &McpBinding) -> Self {
        Self {
            name: binding.name.clone(),
            server_id: binding.server_id.clone(),
            tool: binding.tool.clone(),
        }
    }
}

fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn cut(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// Bind every tool of `toolsets` to a unique model name. Servers and tools
/// are ordered by id and name so names are stable between turns; a name that
/// is already taken gets `_2`, `_3`, ... Tools without an object schema are
/// left out.
pub(crate) fn bind(mut toolsets: Vec<McpToolset>) -> Vec<McpBinding> {
    toolsets.sort_by(|a, b| a.server_id.cmp(&b.server_id));
    let mut taken = BTreeSet::new();
    let mut bindings = Vec::new();
    for set in toolsets {
        let server = sanitize(&set.server_id);
        let mut tools = set.tools;
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        for tool in tools {
            if bindings.len() >= MAX_MCP_TOOLS {
                return bindings;
            }
            if tool.input_schema.get("type").and_then(Value::as_str) != Some("object") {
                continue;
            }
            let prefix = format!("{MCP_PREFIX}{}__", cut(&server, 24));
            let base = format!(
                "{prefix}{}",
                cut(&sanitize(&tool.name), MAX_NAME - prefix.len())
            );
            let mut name = base.clone();
            let mut n = 2;
            while taken.contains(&name) {
                let suffix = format!("_{n}");
                name = format!("{}{suffix}", cut(&base, MAX_NAME - suffix.len()));
                n += 1;
            }
            taken.insert(name.clone());
            let description = cut(
                &format!("[MCP server {}] {}", set.server_name, tool.description),
                MAX_DESCRIPTION,
            );
            bindings.push(McpBinding {
                name,
                server_id: set.server_id.clone(),
                server_name: set.server_name.clone(),
                auto_approve: set.auto_approve_tools.iter().any(|t| t == &tool.name),
                tool: tool.name,
                description,
                schema: tool.input_schema,
            });
        }
    }
    bindings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::McpTool;
    use serde_json::json;

    fn tool(name: &str) -> McpTool {
        McpTool {
            name: name.into(),
            description: "d".into(),
            input_schema: json!({"type":"object"}),
        }
    }

    #[test]
    fn names_are_sanitised_bounded_and_unique() {
        let long = "x".repeat(80);
        let bindings = bind(vec![
            McpToolset {
                server_id: "zeta".into(),
                server_name: "Zeta".into(),
                auto_approve_tools: vec!["read.file".into()],
                tools: vec![tool("read.file"), tool("read_file"), tool(&long)],
            },
            McpToolset {
                server_id: "alpha".into(),
                server_name: "Alpha".into(),
                auto_approve_tools: vec![],
                tools: vec![
                    tool("search"),
                    McpTool {
                        name: "no-schema".into(),
                        description: String::new(),
                        input_schema: json!({"type":"string"}),
                    },
                ],
            },
        ]);
        let names: Vec<_> = bindings.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names[0], "mcp__alpha__search");
        assert_eq!(names[1], "mcp__zeta__read_file");
        assert_eq!(names[2], "mcp__zeta__read_file_2");
        assert!(names[3].starts_with("mcp__zeta__xxx") && names[3].len() == 64);
        assert_eq!(bindings.len(), 4);
        assert!(bindings[1].auto_approve && bindings[1].tool == "read.file");
        assert!(!bindings[2].auto_approve && bindings[2].tool == "read_file");
        for name in names {
            assert!(
                name.len() <= 64
                    && name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            );
        }
    }
}
