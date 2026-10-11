import type {
  ConversationMode,
  PermissionMode,
  SessionSummary,
} from "./sessions";

/** Mirrors aidex-adapters::catalog and aidex-native-sessions::runtime. No credential values. */
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
export type RuntimeDriverInfo = {
  supports_mcp_bridge: boolean;
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
export type RuntimeDescriptor = {
  id: string;
  name: string;
  modes: ConversationMode[];
  permission_modes: PermissionMode[];
  supports_resume: boolean;
  supports_approvals: boolean;
};
export type RuntimeHealth = {
  id: string;
  installed: boolean;
  protocol_ready: boolean;
  authenticated: boolean;
  status: string;
  limitations: string[];
};
export type RuntimeModel = { id: string; name: string };
export type RuntimeAccountStatus = {
  id: string;
  driver: RuntimeDriver;
  profile: string | null;
  authenticated: boolean;
  credential_present: boolean;
  evidence: string;
  status: string;
};
export type RuntimeLoginStatus = {
  id: string;
  flow_id: string;
  state: "pending" | "connected" | "cancelled" | "failed";
  profile: string;
  authenticated: boolean;
  authorization_url: string | null;
  message: string;
  error_code: string | null;
};
export type RuntimeInstall = {
  driver: RuntimeDriver;
  executable: string;
  verified_sha256: string;
};
export type AuthStart = {
  flow_id: string;
  provider_id: string;
  authorization_url: string;
  expires_at: number;
};
export type AuthStatus = {
  state: string;
  provider_id: string;
  account_label?: string | null;
  error?: string | null;
};
export type ExecutionStatus = {
  capabilities: {
    automatic_reviewer: boolean;
    auto_falls_back_to: string;
    filesystem_confinement: boolean;
    network_confinement: boolean;
    shell_requires_full_access_on_windows: boolean;
    max_steps: number;
    max_children: number;
    max_child_depth: number;
    [key: string]: unknown;
  };
  runtime_descriptors: RuntimeDescriptor[];
  total_sessions: number;
  sessions: SessionSummary[];
  total_runs: number;
  runs: { id: string; session_id: string; status: string }[];
};
