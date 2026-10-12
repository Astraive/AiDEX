//! `aidex-plugins`: portable plugin bundles of skills and MCP servers (Claude Code
//! plugin layout compatible), never arbitrary native code.
//!
//! - [`PluginStore`]: install a local folder or `.zip` into `DrivePaths::plugins()/<name>/`
//!   (32 MiB, no links, no traversal, no undeclared executables), record provenance in
//!   `private_config/plugins.toml`, trust an exact SHA-256 tree hash, enable, disable,
//!   uninstall, and [`PluginStore::enabled_contributions`] for skills and MCP discovery.
//! - [`PluginManifest`]: `aidex-plugin.json` or `.claude-plugin/plugin.json`.
//! - [`PluginContents`]: skills, MCP servers (without environment or header values), and
//!   the README, for review before trusting.
//! - [`tree_sha256`]: the deterministic tree hash trust is bound to.

mod contents;
mod error;
mod executables;
mod fsutil;
mod install;
mod limits;
mod manifest;
mod names;
mod provenance;
mod store;
mod tree;

pub use contents::{
    MCP_JSON, McpServerEntry, McpTransport, PluginContents, SKILLS_DIR, SkillEntry, parse_mcp_json,
};
pub use error::PluginError;
pub use limits::{
    MAX_ARCHIVE_BYTES, MAX_ARCHIVE_ENTRIES, MAX_INSTALLED_PLUGINS, MAX_MANIFEST_BYTES,
    MAX_MCP_JSON_BYTES, MAX_PLUGIN_BYTES, MAX_PLUGIN_DIRS, MAX_PLUGIN_FILES,
};
pub use manifest::{
    AIDEX_MANIFEST, CLAUDE_MANIFEST, LoadedManifest, ManifestFile, PluginAuthor, PluginManifest,
    PluginPermissions, parse_manifest,
};
pub use names::{MAX_DEPTH, MAX_NAME_LEN, MAX_PATH_LEN, is_tree_hash, validate_name};
pub use provenance::{PROVENANCE_FILE, PROVENANCE_VERSION, PluginRecord};
pub use store::{
    InstallOutcome, Integrity, PluginContribution, PluginDetail, PluginList, PluginStore,
    PluginSummary, Uninstalled,
};
pub use tree::tree_sha256;
