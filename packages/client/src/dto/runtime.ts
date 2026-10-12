import type { ConversationMode, PermissionMode } from "./sessions";

/** Mirrors aidex-adapters (crates/adapters/src/catalog.rs) and aidexd::methods::runtime. */
export type RuntimeDriver =
  | "codex"
  | "claude_code"
  | "cursor"
  | "grok"
  | "opencode"
  | "antigravity"
  | "pi"
  | "muse"
  | "acp"
  | "omp";

/** Credentials stay in the vault; only the secret name and target environment variable are stored. */
export type RuntimeAuth =
  | { kind: "none" }
  | { kind: "vault"; secret_name: string; env_name: string }
  | { kind: "codex_oauth"; provider_id: string };

export type RuntimeConfig = {
  id: string;
  driver: RuntimeDriver;
  executable: string;
  argv: string[];
  profile: string | null;
  enabled: boolean;
  auth: RuntimeAuth;
};

/** `runtime.catalog` entry: what a driver speaks and what it can do. */
export type RuntimeDriverInfo = {
  driver: RuntimeDriver;
  name: string;
  protocol: string;
  command: string[];
  package: string | null;
  modes: ConversationMode[];
  permission_modes: PermissionMode[];
  limitations: string[];
  auth_methods: string[];
};

export type RuntimeHealthStatus =
  | "ready"
  | "binary_missing"
  | "auth_required"
  | "installed_auth_unverified"
  | "protocol_ready_auth_unverified";

export type RuntimeHealth = {
  id: string;
  installed: boolean;
  protocol_ready: boolean;
  authenticated: boolean;
  status: RuntimeHealthStatus | string;
  limitations: string[];
};

export type RuntimeModel = { id: string; name: string };

export type RuntimeInstallResult = {
  driver: RuntimeDriver;
  executable: string;
  verified_sha256: string;
};

/** Runtime sessions record this placeholder provider id; the harness ignores it for external runtimes. */
export const RUNTIME_PROVIDER_ID = "runtime";
/** A runtime model id that lets the runtime use its own configured default model. */
export const RUNTIME_DEFAULT_MODEL = "default";
/** Drivers whose protocol can list models (`runtime.models`). */
export const RUNTIME_DISCOVERY_DRIVERS: readonly RuntimeDriver[] = ["codex", "pi", "omp", "muse"];
