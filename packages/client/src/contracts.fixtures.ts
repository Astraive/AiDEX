// Contract fixtures: params sent to and results recorded from aidexd over JSON-RPC on a scratch
// drive (ids and times as recorded; drive paths shortened), checked by `bun run check`. `satisfies`
// rejects a missing required field and any field the TypeScript type lacks, so a Rust serde change
// that is not mirrored in `src/dto/` fails the check. Re-record a fixture whenever its Rust type
// changes. Nothing here is imported at run time.
import type { EventStreamNotification, EventsSubscribed } from "./dto/daemon";
import type { RpcParams, RpcResult } from "./rpc";

const secret = {
  created_at: "2026-10-12T18:03:06.707Z",
  kind: "api_key",
  name: "contract-fixture",
  policy: { allowed_consumers: ["provider:fixture", "process:web"] },
  secret_ref: {
    secret_id: "sec_01M4RMGNJKYRMEAE6HC9HPR3M7",
    vault_id: "vault_01M4RMGCSD6D55D655M71PDV96",
  },
  updated_at: "2026-10-12T18:03:06.707Z",
} satisfies RpcResult<"vault.secret.set">;

export const vaultSecretSet = secret;
/** `managed` names the records AiDEX owns (the managed-record guard); a fresh drive has none. */
export const vaultSecretList = {
  secrets: [secret],
  managed: [],
} satisfies RpcResult<"vault.secret.list">;
export const vaultSecretDelete = { deleted: secret } satisfies RpcResult<"vault.secret.delete">;
export const vaultPassphraseChange = {
  old_passphrase: "fixture passphrase only",
  new_passphrase: "another fixture passphrase",
} satisfies RpcParams<"vault.passphrase.change">;

/** `update.status` on a drive with no installed bundle, right after startup recovery. */
export const updateStatus = {
  current: null,
  daemon_version: "0.1.0",
  integrity: {
    dir: null,
    error: "no AiDEX bundle is installed on this drive (`E:\\app\\current.json` does not exist)",
    installed: false,
    manifest_sha256: null,
    pending_update: null,
    previous: null,
    reason: "not_installed",
    sbom: { error: null, present: false, summary: null, valid: false },
    scope: "critical",
    signature: null,
    signer_key_id: null,
    target: "windows-x64",
    verified: false,
    verified_files: null,
    version: null,
  },
  journal: null,
  pending: false,
  previous: null,
  recovery: {
    active: null,
    interrupted_state: null,
    operation: null,
    outcome: "clean",
    removed: [],
    restored_pointer: false,
    update_id: null,
    warnings: [],
  },
  recovery_error: null,
  restart_required: false,
  running_dir: null,
  target: "windows-x64",
  versions: [],
} satisfies RpcResult<"update.status">;
export const updateCheck = {
  source: "E:\\releases\\aidex-0.2.0-windows-x64.zip",
  allow_downgrade: false,
} satisfies RpcParams<"update.check">;

export const networkStatus = {
  network: "online",
  offline: false,
} satisfies RpcResult<"network.status">;
export const networkOffline = {
  network: "offline",
  offline: true,
} satisfies RpcResult<"network.offline">;

export const authStatus = {
  account_label: null,
  error: null,
  provider_id: "chatgpt",
  state: "disconnected",
} satisfies RpcResult<"auth.status">;

export const conversationListParams = {
  offset: 0,
  limit: 1,
} satisfies RpcParams<"conversation.list">;
export const conversationList = {
  next_offset: null,
  offset: 0,
  sessions: [
    {
      created_at: 1791828197365,
      id: "session_01M4RMGZZNSX03GPQ31B507DQN",
      locked: false,
      mode: "chat",
      model_id: "fake-model",
      pinned: false,
      provider_id: "fake",
      status: "idle",
      title: "Contract fixture",
    },
  ],
  total_sessions: 1,
  truncated: false,
} satisfies RpcResult<"conversation.list">;
export const checkpointList = {
  checkpoints: [],
  next_offset: null,
  offset: 0,
  total_checkpoints: 0,
  truncated: false,
} satisfies RpcResult<"checkpoint.list">;
export const approvalListParams = {
  status: "pending",
  offset: 0,
  limit: 5,
} satisfies RpcParams<"approval.list">;
export const approvalList = {
  approvals: [],
  next_offset: null,
  offset: 0,
  total_approvals: 0,
  truncated: false,
} satisfies RpcResult<"approval.list">;

/** A filtered `events.subscribe` (`{after: 0, filter: {kinds: ["session"]}}`) on that drive. */
export const eventsSubscribed = { after: 0, latest_cursor: 4 } satisfies EventsSubscribed;
export const eventsWatermark = {
  method: "events.watermark",
  data: { cursor: 4 },
} satisfies EventStreamNotification;
