/** Mirrors aidex-extensions::model; credentials are vault references only. */
export type SkillSummary = {
  id: string;
  name: string;
  description: string;
  enabled: boolean;
  trusted: boolean;
  user_invocable: boolean;
  model_invocable: boolean;
  workspace_ids: string[];
  profiles: string[];
  digest: string;
  plugin_id: string | null;
};
export type SkillDocument = SkillSummary & { content: string };
export type McpTransport =
  | { kind: "stdio"; command: string; args: string[]; cwd: string | null }
  | { kind: "https"; url: string };
export type McpServerConfig = {
  id: string;
  name: string;
  enabled: boolean;
  trusted: boolean;
  workspace_ids: string[];
  profiles: string[];
  transport: McpTransport;
  secret_refs: Record<string, string>;
  timeout_ms: number;
  max_output_bytes: number;
};
export type McpTool = {
  name: string;
  description: string;
  inputSchema: unknown;
  outputSchema: unknown | null;
  annotations: unknown | null;
};
export type McpServerSummary = McpServerConfig & {
  call_tool_name: string;
  owner_session_id: string | null;
  connected: boolean;
  connecting: boolean;
  catalog_revision: string;
  tools: McpTool[];
  last_error: string | null;
  plugin_id: string | null;
};
export type PluginManifest = {
  id: string;
  name: string;
  version: string;
  description: string;
  trusted: boolean;
  enabled: boolean;
  workspace_ids: string[];
  profiles: string[];
  skills: { path: string }[];
  mcp_servers: McpServerConfig[];
};
export type PluginSummary = PluginManifest & {
  digest: string;
  manifest_digest: string;
};
export type ExtensionScope = {
  workspace_id: string;
  profile: string;
  full_access: boolean;
};
export type CatalogSnapshot = {
  revision: string;
  skills: SkillSummary[];
  mcp_servers: McpServerSummary[];
};
export type MarketplaceEntry = {
  id: string;
  name: string;
  version: string;
  description: string;
  package_digest: string;
  skill_names: string[];
  permissions: string[];
};
export type MarketplaceCatalog = {
  schema_version: 1;
  catalog_version: string;
  entries: MarketplaceEntry[];
};
