// Full `SessionConfig` values for `conversation.create` and `agent.configuration.set`. The daemon
// replaces the whole configuration, so every write carries every field.
import type { AppPreferences } from "@aidex/client/dto/preferences";
import type {
  ConversationMode,
  PermissionMode,
  SessionConfig,
} from "@aidex/client/dto/sessions";
import type { ModelTarget } from "../model-manager/catalog/modelKeys";
import { draftOf, settingsOf, type SessionSettings } from "./sessionSettings";

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
  /**
   * New instructions, fallback, or limits (the conversation settings dialog). A field left out keeps
   * the conversation's value; `fallback_model: null` removes its fallback.
   */
  overrides?: Partial<SessionSettings>;
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
  const kept: SessionSettings = session
    ? {
        instructions: session.instructions ?? defaults.instructions,
        fallback_model: session.fallback_model ?? null,
        max_steps: session.max_steps ?? defaults.max_steps,
        max_tokens: session.max_tokens ?? defaults.max_tokens,
      }
    : {
        instructions: defaults.instructions,
        fallback_model: defaults.fallback_model,
        max_steps: defaults.max_steps,
        max_tokens: defaults.max_tokens,
      };
  const { overrides } = input;
  // Checked like the daemon's `SessionConfig::validate`, so a bad value is refused with field wording.
  const settings = overrides
    ? settingsOf(
        draftOf({
          instructions: overrides.instructions ?? kept.instructions,
          fallback_model:
            overrides.fallback_model !== undefined ? overrides.fallback_model : kept.fallback_model,
          max_steps: overrides.max_steps ?? kept.max_steps,
          max_tokens: overrides.max_tokens ?? kept.max_tokens,
        }),
      )
    : kept;
  return {
    title: session ? session.title : input.title,
    workspace_id: workspaceId || null,
    mode,
    provider_id: target.provider_id,
    model_id: target.model_id,
    runtime_id: target.runtime_id,
    permission_mode: input.permission,
    ...settings,
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
