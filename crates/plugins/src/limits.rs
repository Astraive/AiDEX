//! Size and count limits applied to every plugin tree, archive, and file read.

/// Largest installed plugin: the sum of its file sizes.
pub const MAX_PLUGIN_BYTES: u64 = 32 * 1024 * 1024;
/// Most files in one plugin.
pub const MAX_PLUGIN_FILES: usize = 4096;
/// Most directories in one plugin.
pub const MAX_PLUGIN_DIRS: usize = 4096;
/// Largest `.zip` accepted as an install source.
pub const MAX_ARCHIVE_BYTES: u64 = 48 * 1024 * 1024;
/// Most entries (files and directories) in an install archive.
pub const MAX_ARCHIVE_ENTRIES: usize = MAX_PLUGIN_FILES + MAX_PLUGIN_DIRS;
/// Largest plugin manifest.
pub const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
/// Largest `.mcp.json`.
pub const MAX_MCP_JSON_BYTES: u64 = 256 * 1024;
/// Most plugins recorded in `private_config/plugins.toml`.
pub const MAX_INSTALLED_PLUGINS: usize = 256;

/// Most MCP servers listed for review.
pub(crate) const MAX_MCP_SERVERS: usize = 64;
/// Most skills listed for review.
pub(crate) const MAX_LISTED_SKILLS: usize = 256;
/// Bytes of a `SKILL.md` read to find its front matter.
pub(crate) const MAX_SKILL_HEAD_BYTES: u64 = 64 * 1024;
/// Bytes of `README.md` returned for review.
pub(crate) const MAX_README_BYTES: usize = 16 * 1024;
/// Longest description or display text returned for review.
pub(crate) const MAX_TEXT_BYTES: usize = 1024;
