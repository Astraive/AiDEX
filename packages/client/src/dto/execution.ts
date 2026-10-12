// Mirrors the `execution.status` result (crates/aidexd/src/methods/native.rs), whose `capabilities`
// are `NativeEngine::capabilities` (crates/harness/src/engine.rs). Hand-written until schema
// generation exists.
import type { BackendCapabilities } from "./sandbox";
import type { ConversationMode, PermissionMode, RunStatus, SessionSummary } from "./sessions";

/** What AiDEX's own engine enforces on this computer. */
export type EngineCapabilities = {
  sandbox: BackendCapabilities;
  automatic_reviewer: boolean;
  /** The permission mode `auto` runs as while there is no automatic reviewer. */
  auto_falls_back_to: PermissionMode;
  /** True when the daemon runs on Windows, where shell commands need Full access. */
  shell_requires_full_access_on_windows: boolean;
  /** Commands can be confined to the project's files (the sandbox's Restricted baseline). */
  filesystem_confinement: boolean;
  /** Commands can be kept off the network. */
  network_confinement: boolean;
  max_children: number;
  max_child_depth: number;
  max_steps: number;
  configured_fallback: boolean;
};

/** A run of `execution.status`: `aidex_native_sessions::RunSummary`. */
export type RunSummary = { id: string; session_id: string; status: RunStatus };

/** An external runtime the engine can run: `aidex_native_sessions::RuntimeDescriptor`. */
export type RuntimeDescriptor = {
  id: string;
  name: string;
  modes: ConversationMode[];
  permission_modes: PermissionMode[];
  supports_resume: boolean;
  supports_approvals: boolean;
};

/**
 * Result of `execution.status` (no params). `sessions` holds at most 100 and `runs` at most 1000;
 * the `*_truncated` and `truncated` flags are present (true) only when the wire budget cut more.
 */
export type ExecutionStatus = {
  capabilities: EngineCapabilities;
  total_sessions: number;
  sessions: SessionSummary[];
  total_runs: number;
  runs: RunSummary[];
  runtime_descriptors: RuntimeDescriptor[];
  sessions_truncated?: true;
  runs_truncated?: true;
  truncated?: true;
};
