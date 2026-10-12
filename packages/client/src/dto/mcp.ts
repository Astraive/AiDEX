// Mirrors the `mcp.*` methods in crates/aidexd/src/methods/mcp.rs and the `aidex_mcp`
// wire types (crates/mcp/src/config/mod.rs, config/import.rs, supervisor/mod.rs,
// protocol.rs, logs.rs, client/mod.rs). Configuration names vault secrets only; no
// method accepts or returns a secret value (store values with `vault.secret.set` for
// consumer `mcp:<id>`).

export type McpTransport = "stdio" | "http";

export type McpStatus = "stopped" | "starting" | "running" | "failed" | "needs_auth";

export type McpSourceKind = "drive" | "workspace" | "plugin";

/** Stable `aidex_mcp::McpErrorKind` names (also `data.reason` of `mcp.*` errors). */
export type McpErrorKind =
  | "invalid_config"
  | "invalid_request"
  | "not_found"
  | "conflict"
  | "disabled"
  | "io"
  | "vault_locked"
  | "needs_auth"
  | "offline_refused"
  | "policy_denied"
  | "launch_failed"
  | "protocol"
  | "output_limit"
  | "server_error"
  | "disconnected"
  | "session_expired"
  | "timeout"
  | "cancelled"
  | "unavailable";

/** An HTTP header whose value is the named vault secret. */
export type McpHeaderSecret = { header: string; secret_name: string };

/** An environment variable whose value is the named vault secret. */
export type McpEnvSecret = { env: string; secret_name: string };

/** Provenance of a server file; omitted for servers created in AiDEX (`drive`). */
export type McpSource = { kind: McpSourceKind; owner?: string };

/** One server file (`mcp/<id>.toml`, closed schema version 1). */
export type McpServerConfig = {
  version: number;
  id: string;
  name: string;
  transport: McpTransport;
  command?: string;
  args: string[];
  /** Parent environment variable names copied into the server. */
  env_allowlist: string[];
  url?: string;
  enabled: boolean;
  /** Workspace ids the server is offered in; empty means every workspace. */
  workspaces: string[];
  /** Tools the harness may call without approval in Supervised mode. */
  auto_approve_tools: string[];
  headers_from_secrets?: McpHeaderSecret[];
  env_from_secrets?: McpEnvSecret[];
  source?: McpSource;
};

/** Params of `mcp.add` and `mcp.update`: no version, no provenance, no secret values. */
export type McpServerSpec = {
  id: string;
  /** Display name; defaults to the id. */
  name?: string | null;
  transport: McpTransport;
  command?: string | null;
  args?: string[];
  env_allowlist?: string[];
  url?: string | null;
  headers_from_secrets?: McpHeaderSecret[];
  env_from_secrets?: McpEnvSecret[];
  enabled?: boolean;
  workspaces?: string[];
  auto_approve_tools?: string[];
};

/** The last failure of a server. */
export type McpStatusError = { reason: McpErrorKind; message: string };

/** What the server reported in `initialize` (bounded, redacted display text). */
export type McpServerIdentity = {
  name: string;
  version: string;
  protocol_version: string;
  /** The server declared the `tools` capability. */
  tools: boolean;
};

/** A tool as `mcp.list` shows it (description bounded to 256 bytes). */
export type McpToolSummary = { name: string; description: string };

/** One server as `mcp.list`, `mcp.add`, `mcp.update`, `mcp.set_enabled`, and `mcp.start|stop|restart` return it. */
export type McpServer = {
  id: string;
  name: string;
  transport: McpTransport;
  /** The command line (redacted, bounded) or the URL origin. */
  endpoint: string;
  enabled: boolean;
  status: McpStatus;
  error?: McpStatusError;
  tool_count: number;
  tools: McpToolSummary[];
  source: McpSourceKind;
  /** The importing workspace id or contributing plugin name. */
  source_owner?: string;
  /** Vault secret names the server needs (consumer `mcp:<id>`). */
  secret_names: string[];
  config: McpServerConfig;
  server?: McpServerIdentity;
  /** RFC 3339. */
  started_at?: string;
};

/** A server file that could not be loaded. */
export type McpInvalidServer = { id: string; error: string };

/** Result of `mcp.list`. Plugin servers appear (disabled) once their plugin is trusted and enabled. */
export type McpServerList = {
  servers: McpServer[];
  invalid: McpInvalidServer[];
  /** Tool lists were left out to fit one message (`mcp.tools` has them). */
  tools_omitted: boolean;
};

/** One tool a server offers (description is untrusted text). */
export type McpToolSpec = {
  name: string;
  description: string;
  /** JSON schema of the arguments object. */
  input_schema: unknown;
};

/** Result of `mcp.tools` (starts the server on demand). */
export type McpToolsResult = {
  id: string;
  tools: McpToolSpec[];
  total: number;
  /** Later tools were left out to fit one message. */
  truncated: boolean;
  notes: string[];
};

export type McpLogStream = "stderr" | "server" | "client";

export type McpLogLine = {
  /** RFC 3339. */
  at: string;
  stream: McpLogStream;
  text: string;
};

/** Result of `mcp.logs`. */
export type McpLogsResult = { id: string; lines: McpLogLine[]; dropped: number };

export type McpImportStatus = "added" | "new" | "exists" | "invalid";

/** One `.mcp.json` entry as `mcp.import` handled it. Environment and header values are never imported. */
export type McpImportEntry = {
  status: McpImportStatus;
  /** The entry's key in `mcpServers` (display text only). */
  original_name: string;
  config: McpServerConfig | null;
  error?: string;
  /** Environment variables whose values were dropped (each mapped to a suggested secret). */
  dropped_env: string[];
  dropped_headers: string[];
  warnings: string[];
};

/** Result of `mcp.import`. */
export type McpImportResult = {
  workspace_id: string;
  path: string;
  dry_run: boolean;
  servers: McpImportEntry[];
};
