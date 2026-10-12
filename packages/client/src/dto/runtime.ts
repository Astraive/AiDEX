import type { ConversationMode, PermissionMode } from "./sessions";

/**
 * Mirrors aidex-adapters (crates/adapters/src/catalog.rs and profiles.rs) and
 * aidexd::methods::runtime. No credential values.
 */
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

/**
 * A managed runtime account (`runtime.account.status`, `runtime.profile.import`,
 * `runtime.login.logout`). Only Codex is supported; other drivers fail with
 * `data.reason = "auth_unsupported"`. `authenticated` needs both the official CLI's `login status`
 * success and native credential evidence: cached credentials, not proof of inference
 * entitlement. `evidence` is a stable code (`native_credential_and_cli_exit_status`,
 * `native_credential_missing`, `official_cli_login_pending`, ...). Results never carry
 * credential bytes.
 */
export type RuntimeAccountStatus = {
  id: string;
  driver: RuntimeDriver;
  profile: string | null;
  authenticated: boolean;
  credential_present: boolean;
  evidence: string;
  status: RuntimeAccountState | string;
};

export type RuntimeAccountState =
  | "authenticated_cached"
  | "auth_required"
  | "auth_pending"
  | "signed_out";

/** Where an official CLI browser login is (`runtime.login.*`). */
export type RuntimeLoginState = "pending" | "connected" | "cancelled" | "failed";

/**
 * A browser login run by the runtime's official CLI in an app-owned temporary home
 * (`runtime.login.start`, `runtime.login.status`, `runtime.login.cancel`). Poll `status` with the
 * `flow_id` until the state leaves `pending`; `error_code` is set on some failures (for example
 * `process_shutdown_failed`). `authorization_url` is what the CLI printed, once it has.
 */
export type RuntimeLoginStatus = {
  id: string;
  flow_id: string;
  state: RuntimeLoginState;
  profile: string;
  authenticated: boolean;
  authorization_url: string | null;
  message: string;
  error_code: string | null;
};

/** Runtime sessions record this placeholder provider id; the harness ignores it for external runtimes. */
export const RUNTIME_PROVIDER_ID = "runtime";
/** A runtime model id that lets the runtime use its own configured default model. */
export const RUNTIME_DEFAULT_MODEL = "default";
/** Drivers whose protocol can list models (`runtime.models`). */
export const RUNTIME_DISCOVERY_DRIVERS: readonly RuntimeDriver[] = ["codex", "pi", "omp", "muse"];
