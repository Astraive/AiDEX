// Mirrors the `plugin.*` methods in crates/aidexd/src/methods/plugins.rs and the
// `aidex_plugins` wire types (crates/plugins/src/store.rs, manifest.rs, contents.rs).
// Plugins are bundles of skills and MCP servers; they install disabled and enable only
// after the user trusts their exact tree hash. Environment and header values from a
// plugin's `.mcp.json` are never returned (only their names).

/** How a plugin's files compare with its recorded tree hash. */
export type PluginIntegrity = "intact" | "modified" | "missing" | "invalid";

/** One installed plugin as listed. */
export type PluginSummary = {
  name: string;
  version: string;
  description?: string;
  integrity: PluginIntegrity;
  /** The current tree hash; null when the files are missing or invalid. */
  tree_sha256: string | null;
  /** The recorded tree hash (at install, or when a changed tree was last trusted). */
  installed_sha256: string;
  /** The exact tree hash the user trusted. */
  trusted_sha256: string | null;
  /** The user trusted exactly the current files. */
  trusted: boolean;
  /** Enabled and trusted: the plugin contributes its skills and (disabled) MCP servers. */
  enabled: boolean;
  /** RFC 3339. */
  installed_at: string;
  /** The folder or archive it was installed from. */
  source: string;
  /** Drive-relative directory, e.g. `plugins/code-review`. */
  location: string;
  skill_count: number;
  mcp_server_count: number;
  /** Why the plugin cannot be used as installed. */
  error?: string;
};

export type PluginManifestFile = "aidex-plugin.json" | ".claude-plugin/plugin.json";

export type PluginAuthor = { name: string; url?: string };

/** Permissions the manifest declares (shown for review; not enforced as grants). */
export type PluginPermissions = {
  network: string[];
  secrets: string[];
  shell: boolean;
};

export type PluginManifest = {
  name: string;
  version: string;
  description: string;
  author?: PluginAuthor;
  homepage?: string;
  license?: string;
  permissions: PluginPermissions;
};

export type PluginSkillEntry = {
  /** Directory under `skills/`. */
  dir: string;
  /** Plugin-relative path of the skill file. */
  path: string;
  name?: string;
  description?: string;
};

export type PluginMcpTransport = "stdio" | "http" | "sse";

/** One `.mcp.json` server as the review dialog shows it. */
export type PluginMcpServerEntry = {
  name: string;
  transport: PluginMcpTransport;
  /** Redacted command. */
  command?: string;
  args: string[];
  url?: string;
  /** Environment variable names only (values are never kept). */
  env_keys: string[];
  /** Header names only. */
  header_keys: string[];
  /** The plugin-relative executable the command runs, when bundled. */
  bundled_command?: string;
};

export type PluginContents = {
  skills: PluginSkillEntry[];
  skills_truncated: boolean;
  mcp_servers: PluginMcpServerEntry[];
  ignored_fields: string[];
  unsupported_components: string[];
  file_count: number;
  total_bytes: number;
  readme?: string;
  readme_truncated: boolean;
};

/** Result of `plugin.show`: everything the review dialog shows. */
export type PluginDetail = PluginSummary & {
  manifest_file: PluginManifestFile | null;
  manifest: PluginManifest | null;
  contents: PluginContents | null;
};

/** Result of `plugin.list`. */
export type PluginList = {
  plugins: PluginSummary[];
  /** Directories under `plugins/` with no provenance record; they never contribute. */
  unregistered: string[];
};

/** Params of `plugin.install`: an absolute folder or `.zip` path on this computer. */
export type PluginInstallParams = { source_path: string; replace?: boolean };

/** Result of `plugin.install` (installed disabled). */
export type PluginInstallOutcome = PluginDetail & { replaced: boolean };

/** Params of `plugin.trust`; `enable` makes "Trust and enable" one call. */
export type PluginTrustParams = {
  name: string;
  tree_sha256: string;
  enable?: boolean;
};

export type PluginSetEnabledParams = { name: string; enabled: boolean };

/** Result of `plugin.uninstall`. */
export type PluginUninstalled = {
  name: string;
  removed_files: boolean;
  was_enabled: boolean;
};
