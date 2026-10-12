// Full `SessionConfig` values for `conversation.create` and `agent.configuration.set`. The daemon
// replaces the whole configuration, so every write carries every field.
import type { AppPreferences } from "@aidex/client/dto/preferences";
import type {
  ConversationMode,
  PermissionMode,
  SessionConfig,
} from "@aidex/client/dto/sessions";
import type { ModelTarget } from "../model-manager/catalog/modelKeys";

/** The model fields a conversation is bound to today. */
export function sessionTargetOf(
  session: Pick<SessionConfig, "provider_id" | "model_id" | "runtime_id">,
): ModelTarget {
  return {
    provider_id: session.provider_id,
    model_id: session.model_id,
    runtime_id: session.runtime_id ?? null,
  };
}

export type SessionConfigInput = {
  /** The existing conversation; its own values win over the new-chat defaults. */
  session: SessionConfig | undefined;
  defaults: AppPreferences;
  /** Title for a new conversation. */
  title: string;
  mode: ConversationMode;
  workspaceId: string;
  isRegisteredWorkspace: (id: string) => boolean;
  target: ModelTarget;
  permission: PermissionMode;
};

/** Builds and checks a complete configuration; throws an `Error` with user-facing text when it cannot run. */
export function buildSessionConfig(input: SessionConfigInput): SessionConfig {
  const { session, defaults, mode, workspaceId, target } = input;
  if (mode !== "chat" && !workspaceId)
    throw new Error("Choose a project before starting Plan or Agent mode.");
  if (workspaceId && !input.isRegisteredWorkspace(workspaceId))
    throw new Error("Choose a registered project or No project context.");
  // A conversation keeps its own values, including an explicit null fallback; `??` would let the
  // global preference leak into it on every edit.
  return {
    title: session ? session.title : input.title,
    workspace_id: workspaceId || null,
    mode,
    provider_id: target.provider_id,
    model_id: target.model_id,
    runtime_id: target.runtime_id,
    permission_mode: input.permission,
    instructions: session ? session.instructions : defaults.instructions,
    fallback_model: session ? (session.fallback_model ?? null) : defaults.fallback_model,
    max_steps: session ? session.max_steps : defaults.max_steps,
    max_tokens: session ? session.max_tokens : defaults.max_tokens,
  };
}

/** Whether two configurations differ in what a run uses: model, runtime, mode, project, or permissions. */
export function configurationChanged(previous: SessionConfig, next: SessionConfig): boolean {
  return (
    previous.provider_id !== next.provider_id ||
    previous.model_id !== next.model_id ||
    (previous.runtime_id ?? null) !== (next.runtime_id ?? null) ||
    previous.mode !== next.mode ||
    previous.workspace_id !== next.workspace_id ||
    previous.permission_mode !== next.permission_mode
  );
}
