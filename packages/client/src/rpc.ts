import { command, listenTo } from "./bridge";
import type {
  DaemonStatus,
  LatestCursor,
  ReplayPage,
  ReplayParams,
} from "./dto/daemon";
import type {
  VaultInitParams,
  VaultInitResult,
  VaultLockResult,
  VaultStatus,
  VaultUnlockParams,
  SecretMeta,
  SecretSetParams,
} from "./dto/vault";
import type {
  SelectedWorkspace,
  Workspace,
  WorkspaceList,
} from "./dto/workspace";
import type {
  ModelList,
  ModelRef,
  ProviderAccount,
  ProviderAddParams,
  ProviderPreset,
} from "./dto/models";
import type {
  Approval,
  Run,
  SessionConfig,
  SessionDetail,
  SessionSummary,
  ContentChunk,
  HistoryPage,
  HistoryScope,
  Checkpoint,
} from "./dto/sessions";
import type { AppPreferences } from "./dto/preferences";
import type {
  InferenceCatalog,
  InferenceOptions,
  InferenceRuntime,
  InferenceStatus,
  ManagedModel,
} from "./dto/inference";
import type {
  RuntimeConfig,
  RuntimeDriver,
  RuntimeDriverInfo,
  RuntimeHealth,
  RuntimeInstall,
  RuntimeModel,
  RuntimeAccountStatus,
  RuntimeLoginStatus,
  AuthStart,
  AuthStatus,
  ExecutionStatus,
} from "./dto/runtimes";
import type {
  SkillSummary,
  SkillDocument,
  McpServerConfig,
  McpServerSummary,
  PluginManifest,
  PluginSummary,
  ExtensionScope,
  CatalogSnapshot,
  MarketplaceCatalog,
} from "./dto/extensions";
import type { SearchProvider, WebStatus } from "./dto/web";
import type {
  UpdateStatus,
  UpdateSourceParams,
  CheckReport,
  ApplyReport,
  RollbackReport,
} from "./dto/updates";

/**
 * Daemon methods the apps use, with their params and results. Add an entry when a screen needs a
 * method (crates/aidexd/src/methods/); `callUntyped` reaches anything else. Streams such as
 * `events.subscribe` go through `subscribe`.
 */
export type RpcMethods = {
  "update.status": { params: undefined; result: UpdateStatus };
  "update.check": { params: UpdateSourceParams; result: CheckReport };
  "update.apply": { params: UpdateSourceParams; result: ApplyReport };
  "update.rollback": {
    params: { allow_unsigned?: boolean } | undefined;
    result: RollbackReport;
  };
  "network.status": {
    params: undefined;
    result: { offline: boolean; network: "online" | "offline" };
  };
  "network.offline": {
    params: { enabled: boolean };
    result: { offline: boolean; network: "online" | "offline" };
  };
  "runtime.account.status": {
    params: { id: string };
    result: RuntimeAccountStatus;
  };
  "runtime.profile.import": {
    params: { id: string; source: string };
    result: RuntimeAccountStatus;
  };
  "runtime.login.start": { params: { id: string }; result: RuntimeLoginStatus };
  "runtime.login.status": {
    params: { id: string; flow_id: string };
    result: RuntimeLoginStatus;
  };
  "runtime.login.cancel": {
    params: { id: string; flow_id: string };
    result: RuntimeLoginStatus;
  };
  "runtime.login.logout": {
    params: { id: string };
    result: RuntimeAccountStatus;
  };
  "extensions.marketplace.list": {
    params: undefined;
    result: MarketplaceCatalog;
  };
  "extensions.marketplace.install": {
    params: {
      id: string;
      version: string;
      package_digest: string;
      trusted: boolean;
      workspace_ids: string[];
      profiles: string[];
    };
    result: PluginSummary;
  };
  "web.status": { params: undefined; result: WebStatus };
  "web.configure": {
    params: { provider: SearchProvider; enabled: boolean; api_key?: string };
    result: WebStatus;
  };
  "web.disconnect": { params: undefined; result: WebStatus };
  "extensions.skills.list": { params: undefined; result: SkillSummary[] };
  "extensions.skills.read": { params: { id: string }; result: SkillDocument };
  "extensions.skills.save": {
    params: { id: string; content: string; expected_digest?: string };
    result: SkillSummary;
  };
  "extensions.skills.import": {
    params: {
      content: string;
      trusted: boolean;
      workspace_ids: string[];
      profiles: string[];
    };
    result: SkillSummary;
  };
  "extensions.skills.enable": {
    params: { id: string; enabled: boolean };
    result: SkillSummary;
  };
  "extensions.skills.remove": {
    params: { id: string };
    result: { removed: boolean };
  };
  "extensions.plugins.list": { params: undefined; result: PluginSummary[] };
  "extensions.plugins.install": {
    params: { manifest: PluginManifest; files: Record<string, string> };
    result: PluginSummary;
  };
  "extensions.plugins.enable": {
    params: { id: string; enabled: boolean };
    result: PluginSummary;
  };
  "extensions.plugins.remove": {
    params: { id: string };
    result: { removed: boolean };
  };
  "extensions.mcp.list": { params: undefined; result: McpServerSummary[] };
  "extensions.mcp.upsert": {
    params: { config: McpServerConfig };
    result: McpServerSummary;
  };
  "extensions.mcp.remove": {
    params: { id: string };
    result: { removed: boolean };
  };
  "extensions.mcp.connect": {
    params: ExtensionScope & { id: string };
    result: McpServerSummary;
  };
  "extensions.mcp.discover": {
    params: ExtensionScope & { id: string };
    result: McpServerSummary;
  };
  "extensions.mcp.disconnect": {
    params: { id: string };
    result: McpServerSummary;
  };
  "extensions.mcp.cancel": { params: { id: string }; result: McpServerSummary };
  "extensions.catalog": { params: ExtensionScope; result: CatalogSnapshot };
  "runtime.catalog": { params: undefined; result: RuntimeDriverInfo[] };
  "runtime.list": { params: undefined; result: { runtimes: RuntimeConfig[] } };
  "runtime.config.get": { params: { id: string }; result: RuntimeConfig };
  "runtime.config.save": { params: RuntimeConfig; result: RuntimeConfig };
  "runtime.add": { params: RuntimeConfig; result: RuntimeConfig };
  "runtime.update": { params: RuntimeConfig; result: RuntimeConfig };
  "runtime.remove": { params: { id: string }; result: { removed: boolean } };
  "runtime.health": { params: { id: string }; result: RuntimeHealth };
  "runtime.models": {
    params: { id: string };
    result: { models: RuntimeModel[] };
  };
  "runtime.install": {
    params: { driver: RuntimeDriver; source?: string };
    result: RuntimeInstall;
  };
  "runtime.auth.start": { params: { id: string }; result: AuthStart };
  "runtime.auth.status": {
    params: { id: string; flow_id?: string };
    result: AuthStatus;
  };
  "runtime.auth.cancel": {
    params: { id: string; flow_id: string };
    result: AuthStatus;
  };
  "execution.status": { params: undefined; result: ExecutionStatus };
  "inference.catalog": { params: undefined; result: InferenceCatalog };
  "inference.status": { params: undefined; result: InferenceStatus };
  "inference.import": {
    params: { source: string; name?: string };
    result: ManagedModel;
  };
  "inference.runtime.install": {
    params: { source?: string };
    result: InferenceRuntime;
  };
  "inference.start": { params: InferenceOptions; result: InferenceStatus };
  "inference.stop": { params: undefined; result: InferenceStatus };
  "inference.cancel": { params: undefined; result: { cancelled: boolean } };
  "inference.model.remove": {
    params: { id: string };
    result: { removed: boolean };
  };
  "auth.start": {
    params: { provider_id: string; method: "chatgpt" };
    result: {
      flow_id: string;
      provider_id: string;
      authorization_url: string;
      expires_at: number;
    };
  };
  "auth.open": { params: { authorization_url: string }; result: unknown };
  "auth.status": {
    params: { flow_id?: string; provider_id?: string };
    result: {
      state: string;
      provider_id: string;
      account_label?: string | null;
      error?: string | null;
    };
  };
  "auth.cancel": { params: { flow_id: string }; result: unknown };
  "auth.disconnect": { params: { provider_id: string }; result: unknown };
  "settings.get": { params: undefined; result: AppPreferences };
  "settings.set": {
    params: { patch: Partial<AppPreferences> };
    result: AppPreferences;
  };
  "workspace.add": {
    params: { path: string; name?: string };
    result: Workspace;
  };
  "workspace.create_drive": { params: { name: string }; result: Workspace };
  "workspace.select": { params: { id_or_name: string }; result: Workspace };
  "workspace.get": { params: { id: string }; result: Workspace };
  "workspace.remove": {
    params: { id: string; delete_data: boolean };
    result: { workspace: Workspace; deleted_data: boolean };
  };
  "conversation.list": {
    params: { offset?: number; limit?: number } | undefined;
    result: {
      sessions: SessionSummary[];
      total_sessions: number;
      offset: number;
      next_offset: number | null;
      truncated: boolean;
    };
  };
  "conversation.create": { params: SessionConfig; result: SessionDetail };
  "conversation.get": {
    params: { id: string; message_offset?: number; message_limit?: number };
    result: SessionDetail;
  };
  "history.list": {
    params: HistoryScope & { offset?: number; limit?: number };
    result: HistoryPage;
  };
  "history.read": {
    params: HistoryScope & {
      index: number;
      field?: string;
      call_index?: number;
      offset?: number;
      limit?: number;
    };
    result: ContentChunk;
  };
  "checkpoint.content": {
    params: { session_id: string; id: string; offset?: number; limit?: number };
    result: ContentChunk;
  };
  "checkpoint.list": {
    params: { session_id: string; offset?: number; limit?: number };
    result: {
      checkpoints: Checkpoint[];
      total_checkpoints: number;
      offset: number;
      next_offset: number | null;
      truncated: boolean;
    };
  };
  "checkpoint.compare": {
    params: { session_id: string; id: string };
    result: {
      checkpoint: Checkpoint;
      current_digest: string;
      conflict: boolean;
    };
  };
  "checkpoint.restore": {
    params: { session_id: string; id: string; expected_digest: string };
    result: { restored: string };
  };
  "conversation.delete": {
    params: { id: string };
    result: { deleted: boolean };
  };
  "conversation.pin": {
    params: { id: string; pinned: boolean };
    result: SessionSummary;
  };
  "message.send": {
    params: { session_id: string; content: string; command_id: string };
    result: Run;
  };
  "agent.start": {
    params: { session_id: string; content: string; command_id: string };
    result: Run;
  };
  "run.get": { params: { id: string }; result: Run };
  "run.cancel": { params: { id: string }; result: Run };
  "run.resume": { params: { id: string }; result: Run };
  "approval.list": {
    params: {
      run_id?: string;
      status?: "pending" | "approved" | "denied" | "expired";
      offset?: number;
      limit?: number;
    };
    result: {
      approvals: Approval[];
      total_approvals: number;
      offset: number;
      next_offset: number | null;
      truncated: boolean;
    };
  };
  "approval.respond": {
    params: {
      id: string;
      fingerprint: string;
      approved: boolean;
      command_id: string;
    };
    result: Run;
  };
  "agent.configuration.set": {
    params: { session_id: string; configuration: SessionConfig };
    result: SessionConfig;
  };
  "model.list": { params: undefined; result: ModelList };
  "model.providers": {
    params: undefined;
    result: { providers: ProviderAccount[] };
  };
  "model.use": {
    params: { provider_id: string; model_id: string };
    result: { default_model: ModelRef | null };
  };
  "provider.presets": {
    params: undefined;
    result: { presets: ProviderPreset[] };
  };
  "provider.add": { params: ProviderAddParams; result: ProviderAccount };
  "provider.update": { params: ProviderAddParams; result: ProviderAccount };
  "provider.remove": { params: { id: string }; result: { removed: boolean } };
  "provider.health": {
    params: { id: string };
    result: { provider_id: string; status: string; message?: string };
  };
  "vault.secret.set": {
    params: SecretSetParams;
    result: SecretMeta;
  };
  "vault.secret.list": { params: undefined; result: { secrets: SecretMeta[] } };
  "vault.secret.delete": {
    params: { name: string };
    result: { deleted: SecretMeta };
  };
  "vault.passphrase.change": {
    params: { old_passphrase?: string; new_passphrase: string };
    result: { changed: boolean };
  };
  "daemon.status": { params: undefined; result: DaemonStatus };
  "daemon.ping": { params: undefined; result: { pong: true } };
  "events.replay": { params: ReplayParams; result: ReplayPage };
  "events.latest": { params: undefined; result: LatestCursor };
  "vault.status": { params: undefined; result: VaultStatus };
  "vault.init": { params: VaultInitParams; result: VaultInitResult };
  "vault.unlock": { params: VaultUnlockParams; result: VaultStatus };
  "vault.lock": { params: undefined; result: VaultLockResult };
  "workspace.list": { params: undefined; result: WorkspaceList };
  "workspace.selected": { params: undefined; result: SelectedWorkspace };
};

export type RpcMethod = keyof RpcMethods;
export type RpcParams<M extends RpcMethod> = RpcMethods[M]["params"];
export type RpcResult<M extends RpcMethod> = RpcMethods[M]["result"];

/** Call a typed daemon method through the desktop bridge (`rpc_call`). */
export function call<M extends RpcMethod>(
  method: M,
  params?: RpcParams<M>,
): Promise<RpcResult<M>> {
  return command<RpcResult<M>>("rpc_call", { method, params: params ?? null });
}

/** Call any daemon method by name; errors keep the daemon's `kind` (e.g. `method_not_found`). */
export function callUntyped(
  method: string,
  params?: unknown,
): Promise<unknown> {
  return command<unknown>("rpc_call", { method, params: params ?? null });
}

/** Payload of the `aidex://notification` event. */
export type NotificationEvent = {
  subscription: string;
  method: string;
  data: unknown;
};
/** Payload of the `aidex://subscription-ended` event. */
export type SubscriptionEnded = {
  subscription: string;
  reason: string;
  error?: unknown;
};

export type SubscribeHandlers = {
  /** Each notification's `data`, in order. */
  onData: (data: unknown, method: string) => void;
  /** The stream ended on the daemon side or its connection closed (not called after `unsubscribe`). */
  onEnd?: (ended: SubscriptionEnded) => void;
};

export type Subscription = {
  id: string;
  /** What the daemon's subscribe handler returned (for example a cursor). */
  initial: unknown;
  unsubscribe: () => Promise<void>;
};

/** Notifications that can arrive before `rpc_subscribe` returns the id are buffered up to this many. */
const EARLY_BUFFER_LIMIT = 1024;

/**
 * Subscribe to a daemon stream (`rpc_subscribe`). Listeners attach before the subscribe call, so
 * notifications sent before it returns are replayed in order rather than lost.
 */
export async function subscribe(
  method: string,
  params: unknown,
  handlers: SubscribeHandlers,
): Promise<Subscription> {
  let id: string | null = null;
  let closed = false;
  const early: NotificationEvent[] = [];
  const earlyEnds: SubscriptionEnded[] = [];

  const stopNotifications = await listenTo<NotificationEvent>(
    "aidex://notification",
    (event) => {
      if (closed) return;
      if (id === null) {
        if (early.length < EARLY_BUFFER_LIMIT) early.push(event);
      } else if (event.subscription === id) {
        handlers.onData(event.data, event.method);
      }
    },
  );
  const stopEnds = await listenTo<SubscriptionEnded>(
    "aidex://subscription-ended",
    (event) => {
      if (closed) return;
      if (id === null) earlyEnds.push(event);
      else if (event.subscription === id) finish(event);
    },
  );

  const detach = () => {
    closed = true;
    stopNotifications();
    stopEnds();
  };
  const finish = (event: SubscriptionEnded) => {
    detach();
    handlers.onEnd?.(event);
  };

  let subscribed: { subscription: string; initial: unknown };
  try {
    subscribed = await command<{ subscription: string; initial: unknown }>(
      "rpc_subscribe",
      {
        method,
        params: params ?? null,
      },
    );
  } catch (error) {
    detach();
    throw error;
  }
  id = subscribed.subscription;
  for (const event of early.filter(
    (candidate) => candidate.subscription === id,
  ))
    handlers.onData(event.data, event.method);
  const ended = earlyEnds.find((candidate) => candidate.subscription === id);
  if (ended) finish(ended);

  return {
    id,
    initial: subscribed.initial,
    unsubscribe: async () => {
      if (closed) return;
      detach();
      await command<boolean>("rpc_unsubscribe", {
        subscription: subscribed.subscription,
      });
    },
  };
}
