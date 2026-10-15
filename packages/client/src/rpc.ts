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
} from "./dto/vault";
import type {
  SelectedWorkspace,
  Workspace,
  WorkspaceList,
} from "./dto/workspace";
import type {
  DefaultModel,
  ModelList,
  ModelListParams,
  ProviderAccount,
  ProviderAddParams,
  ProviderHealth,
  ProviderPatchParams,
  ProviderPreset,
} from "./dto/models";
import type {
  RuntimeConfig,
  RuntimeDriver,
  RuntimeDriverInfo,
  RuntimeHealth,
  RuntimeInstallResult,
  RuntimeModel,
} from "./dto/runtime";
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
  ConversationForkParams,
  SendParams,
} from "./dto/sessions";
import type {
  SkillDeleted,
  SkillDetail,
  SkillDraft,
  SkillList,
  SkillListParams,
  SkillSetEnabledParams,
  SkillShowParams,
  SkillSummary,
} from "./dto/skills";
import type {
  McpImportResult,
  McpLogsResult,
  McpServer,
  McpServerList,
  McpServerSpec,
  McpToolsResult,
} from "./dto/mcp";
import type {
  WebProviderInfo,
  WebSettingsPatch,
  WebSettingsView,
  WebTestParams,
  WebTestResult,
} from "./dto/web";
import type {
  PluginDetail,
  PluginInstallOutcome,
  PluginInstallParams,
  PluginList,
  PluginSetEnabledParams,
  PluginSummary,
  PluginTrustParams,
  PluginUninstalled,
} from "./dto/plugins";
import type {
  ArtifactContent,
  ArtifactList,
  ArtifactListParams,
  ArtifactMeta,
  ArtifactPlanUpdateParams,
  ArtifactReadParams,
} from "./dto/artifacts";
import type {
  Attachment,
  AttachmentContent,
  AttachmentExcludeParams,
  AttachmentFromWorkspaceParams,
  AttachmentList,
  AttachmentListParams,
  AttachmentReadParams,
  AttachmentUploadBeginParams,
  AttachmentUploadChunkParams,
  AttachmentUploadTicket,
} from "./dto/attachments";
import type { ExecutionStatus } from "./dto/execution";
import type { AppPreferences, ModelPreferences } from "./dto/preferences";
import type { Appearance, AppearancePatch } from "./dto/preferences";
import type {
  InferenceCatalog,
  InferenceOptions,
  InferenceRuntime,
  InferenceStatus,
  ManagedModel,
} from "./dto/inference";

/**
 * Daemon methods the apps use, with their params and results. Add an entry when a screen needs a
 * method (crates/aidexd/src/methods/); `callUntyped` reaches anything else. Streams such as
 * `events.subscribe` go through `subscribe`.
 */
export type RpcMethods = {
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
  "model.preferences.get": { params: undefined; result: ModelPreferences };
  "model.preferences.set": {
    params: Partial<ModelPreferences>;
    result: ModelPreferences;
  };
  "settings.set": {
    params: { patch: Partial<AppPreferences> };
    result: AppPreferences;
  };
  "appearance.get": { params: undefined; result: Appearance };
  "appearance.set": { params: AppearancePatch; result: Appearance };
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
    params: undefined;
    result: { sessions: SessionSummary[] };
  };
  "conversation.create": { params: SessionConfig; result: SessionDetail };
  "conversation.fork": { params: ConversationForkParams; result: SessionDetail };
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
      field?: "content" | "tool_arguments" | "provider_metadata";
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
    result: { checkpoints: Checkpoint[]; total_checkpoints: number };
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
  "message.send": { params: SendParams; result: Run };
  "agent.start": { params: SendParams; result: Run };
  "run.get": { params: { id: string }; result: Run };
  "run.cancel": { params: { id: string }; result: Run };
  "run.resume": { params: { id: string }; result: Run };
  "approval.list": {
    params: { run_id?: string };
    result: { approvals: Approval[] };
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
  /** Engine capabilities on this computer, with bounded session and run summaries. */
  "execution.status": { params: undefined; result: ExecutionStatus };
  "model.list": { params: ModelListParams | undefined; result: ModelList };
  "model.providers": {
    params: undefined;
    result: { providers: ProviderAccount[] };
  };
  "model.use": {
    params: { provider_id: string; model_id: string };
    result: DefaultModel;
  };
  "model.default": { params: undefined; result: DefaultModel };
  "provider.presets": {
    params: undefined;
    result: { presets: ProviderPreset[] };
  };
  "provider.add": { params: ProviderAddParams; result: ProviderAccount };
  "provider.update": { params: ProviderAddParams; result: ProviderAccount };
  "provider.patch": { params: ProviderPatchParams; result: ProviderAccount };
  "provider.remove": { params: { id: string }; result: { removed: string } };
  "provider.health": { params: { id: string }; result: ProviderHealth };
  /** Opens a preset's own API-key or docs page in the system browser; never takes a URL. */
  "provider.open_link": {
    params: { preset_id: string; link: "api_key" | "docs" };
    result: { opened: true };
  };
  "runtime.catalog": { params: undefined; result: RuntimeDriverInfo[] };
  "runtime.list": { params: undefined; result: { runtimes: RuntimeConfig[] } };
  "runtime.add": { params: RuntimeConfig; result: RuntimeConfig };
  "runtime.update": { params: RuntimeConfig; result: RuntimeConfig };
  "runtime.remove": { params: { id: string }; result: { removed: boolean } };
  "runtime.config.get": { params: { id: string }; result: RuntimeConfig };
  "runtime.health": { params: { id: string }; result: RuntimeHealth };
  "runtime.models": { params: { id: string }; result: { models: RuntimeModel[] } };
  "runtime.install": {
    params: { driver: RuntimeDriver; source?: string };
    result: RuntimeInstallResult;
  };
  "runtime.auth.start": {
    params: { id: string };
    result: {
      flow_id: string;
      provider_id: string;
      authorization_url: string;
      expires_at: number;
    };
  };
  "runtime.auth.status": {
    params: { id: string; flow_id?: string };
    result: {
      state: string;
      provider_id: string;
      account_label?: string | null;
      error?: string | null;
    };
  };
  "runtime.auth.cancel": {
    params: { id: string; flow_id: string };
    result: unknown;
  };
  "vault.secret.set": {
    params: { name: string; value: string; consumers: string[]; kind?: string };
    result: unknown;
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
  "skill.list": { params: SkillListParams | undefined; result: SkillList };
  "skill.show": { params: SkillShowParams; result: SkillDetail };
  "skill.set_enabled": { params: SkillSetEnabledParams; result: SkillSummary };
  "skill.save": { params: SkillDraft; result: SkillSummary };
  "skill.delete": { params: { name: string }; result: SkillDeleted };
  "mcp.list": { params: { workspace_id?: string } | undefined; result: McpServerList };
  "mcp.add": { params: McpServerSpec; result: McpServer };
  /** Stops a running server so its next start uses the new configuration. */
  "mcp.update": { params: McpServerSpec; result: McpServer };
  "mcp.remove": { params: { id: string }; result: { removed: boolean } };
  "mcp.set_enabled": { params: { id: string; enabled: boolean }; result: McpServer };
  "mcp.start": { params: { id: string }; result: McpServer };
  "mcp.stop": { params: { id: string }; result: McpServer };
  "mcp.restart": { params: { id: string }; result: McpServer };
  "mcp.tools": { params: { id: string }; result: McpToolsResult };
  "mcp.logs": { params: { id: string; limit?: number }; result: McpLogsResult };
  /** Saves each valid `.mcp.json` entry of the workspace as a DISABLED server (`dry_run` previews). */
  "mcp.import": {
    params: { workspace_id?: string; dry_run?: boolean } | undefined;
    result: McpImportResult;
  };
  "plugin.list": { params: undefined; result: PluginList };
  "plugin.show": { params: { name: string }; result: PluginDetail };
  "plugin.install": { params: PluginInstallParams; result: PluginInstallOutcome };
  "plugin.trust": { params: PluginTrustParams; result: PluginSummary };
  "plugin.set_enabled": { params: PluginSetEnabledParams; result: PluginSummary };
  "plugin.uninstall": { params: { name: string }; result: PluginUninstalled };
  "web.settings.get": { params: undefined; result: WebSettingsView };
  "web.settings.set": { params: WebSettingsPatch; result: WebSettingsView };
  "web.providers": { params: undefined; result: { providers: WebProviderInfo[] } };
  "web.test": { params: WebTestParams; result: WebTestResult };
  /** Opens an https URL from a result in the system browser after strict validation. */
  "web.open": { params: { url: string }; result: { opened: true } };
  "artifact.list": { params: ArtifactListParams; result: ArtifactList };
  "artifact.get": { params: { id: string }; result: ArtifactMeta };
  "artifact.read": { params: ArtifactReadParams; result: ArtifactContent };
  "artifact.delete": { params: { id: string }; result: { deleted: boolean } };
  "artifact.plan.update": { params: ArtifactPlanUpdateParams; result: ArtifactMeta };
  "attachment.upload.begin": {
    params: AttachmentUploadBeginParams;
    result: AttachmentUploadTicket;
  };
  "attachment.upload.chunk": {
    params: AttachmentUploadChunkParams;
    result: { received: number };
  };
  "attachment.upload.commit": { params: { upload_id: string }; result: Attachment };
  /** Works while the vault is locked. */
  "attachment.upload.abort": {
    params: { upload_id: string };
    result: { aborted: boolean };
  };
  "attachment.from_workspace": {
    params: AttachmentFromWorkspaceParams;
    result: Attachment;
  };
  "attachment.list": { params: AttachmentListParams; result: AttachmentList };
  "attachment.get": { params: { id: string }; result: Attachment };
  "attachment.read": { params: AttachmentReadParams; result: AttachmentContent };
  /** Staged attachments only (`attachment_sent` otherwise). */
  "attachment.delete": { params: { id: string }; result: { deleted: boolean } };
  "attachment.exclude": { params: AttachmentExcludeParams; result: Attachment };
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
