use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CallScope {
    pub workspace_id: String,
    pub workspace_root: PathBuf,
    pub profile: String,
    pub full_access: bool,
    pub network_allowed: bool,
    #[serde(default)]
    pub selected_skill_ids: Vec<String>,
}

impl CallScope {
    pub(crate) fn mcp_scope(&self) -> Self {
        let mut scope = self.clone();
        scope.selected_skill_ids.clear();
        scope
    }
    pub(crate) fn same_mcp_scope(&self, other: &Self) -> bool {
        self.mcp_scope() == other.mcp_scope()
    }
    pub(crate) fn can_read_skill(&self, skill: &SkillSummary) -> bool {
        skill.model_invocable
            || (skill.user_invocable && self.selected_skill_ids.contains(&skill.id))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillSummary {
    pub id: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub trusted: bool,
    pub user_invocable: bool,
    pub model_invocable: bool,
    pub workspace_ids: Vec<String>,
    pub profiles: Vec<String>,
    pub digest: String,
    pub plugin_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SkillDocument {
    #[serde(flatten)]
    pub summary: SkillSummary,
    pub content: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum McpTransport {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        cwd: Option<String>,
    },
    Https {
        url: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpServerConfig {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub trusted: bool,
    #[serde(default)]
    pub workspace_ids: Vec<String>,
    #[serde(default)]
    pub profiles: Vec<String>,
    pub transport: McpTransport,
    #[serde(default)]
    pub secret_refs: BTreeMap<String, String>,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    #[serde(default = "default_output")]
    pub max_output_bytes: usize,
}
fn default_timeout() -> u64 {
    30_000
}
fn default_output() -> usize {
    256 * 1024
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "inputSchema", alias = "input_schema")]
    pub input_schema: Value,
    #[serde(rename = "outputSchema", alias = "output_schema", default)]
    pub output_schema: Option<Value>,
    #[serde(default)]
    pub annotations: Option<Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct McpServerSummary {
    #[serde(flatten)]
    pub config: McpServerConfig,
    pub connected: bool,
    #[serde(default)]
    pub connecting: bool,
    pub catalog_revision: String,
    pub tools: Vec<McpTool>,
    pub last_error: Option<String>,
    pub plugin_id: Option<String>,
    pub owner_session_id: Option<String>,
    pub call_tool_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginSkill {
    pub path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub trusted: bool,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub workspace_ids: Vec<String>,
    #[serde(default)]
    pub profiles: Vec<String>,
    #[serde(default)]
    pub skills: Vec<PluginSkill>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginSummary {
    #[serde(flatten)]
    pub manifest: PluginManifest,
    pub digest: String,
    pub manifest_digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CatalogSnapshot {
    pub revision: String,
    pub skills: Vec<SkillSummary>,
    pub mcp_servers: Vec<McpServerSummary>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarketplaceEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub package_digest: String,
    pub skill_names: Vec<String>,
    pub permissions: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarketplaceCatalog {
    pub schema_version: u32,
    pub catalog_version: String,
    pub entries: Vec<MarketplaceEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarketplaceInstall {
    pub id: String,
    pub version: String,
    pub package_digest: String,
    pub trusted: bool,
    pub workspace_ids: Vec<String>,
    pub profiles: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SkillRecord {
    pub summary: SkillSummary,
    pub path: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ServerRecord {
    pub config: McpServerConfig,
    pub plugin_id: Option<String>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct State {
    #[serde(default)]
    pub skills: BTreeMap<String, SkillRecord>,
    #[serde(default)]
    pub servers: BTreeMap<String, ServerRecord>,
    #[serde(default)]
    pub plugins: BTreeMap<String, PluginSummary>,
}
