// AiDEX Native's defaults for new conversations, saved encrypted in `AppPreferences` (`settings.set`):
// the default model, the permission for Plan and Agent, instructions, the fallback model, and the step
// and token limits. Each new conversation copies them into its own configuration
// (`chat/sessionConfig.ts`). Pure, so the bounds are testable; they match the daemon's
// (`AppPreferences::validate` in crates/preferences and `SessionConfig::validate` in
// crates/native-sessions).
import type { AppPreferences } from "@aidex/client/dto/preferences";
import type { PermissionMode } from "@aidex/client/dto/sessions";
import {
  MAX_MODEL_KEY_BYTES,
  byteLength,
  draftOf,
  sessionSettingsErrors,
  settingsOf,
  type SessionSettingsDraft,
  type SessionSettingsErrors,
} from "../chat/sessionSettings";
import { parseModelKey } from "../model-manager/catalog/modelKeys";

/** The route id of AiDEX Native's own configuration page (`agent-config`). */
export const NATIVE_AGENT_ID = "native";

/** The form's text: every field as the user edits it. */
export type NativeDefaultsDraft = SessionSettingsDraft & {
  /** A native or runtime model key, or "" for the registry default. */
  model_id: string;
  permission_mode: PermissionMode;
};

export type NativeDefaultsErrors = SessionSettingsErrors & { model_id?: string };

/** The fields this form saves. */
export type NativeDefaults = Pick<
  AppPreferences,
  "model_id" | "permission_mode" | "instructions" | "fallback_model" | "max_steps" | "max_tokens"
>;

/** The saved defaults as form text; a missing permission is Supervised, as new chats apply it. */
export function nativeDefaultsDraft(settings: AppPreferences): NativeDefaultsDraft {
  return {
    ...draftOf(settings),
    model_id: settings.model_id,
    permission_mode: settings.permission_mode ?? "supervised",
  };
}

/** Field errors, keyed like the draft; empty when the draft can be saved. */
export function validateNativeDefaults(draft: NativeDefaultsDraft): NativeDefaultsErrors {
  const errors: NativeDefaultsErrors = sessionSettingsErrors(draft);
  if (draft.model_id && (!parseModelKey(draft.model_id) || byteLength(draft.model_id) > MAX_MODEL_KEY_BYTES))
    errors.model_id = "Choose a default model from the list.";
  return errors;
}

/** The `settings.set` patch of a valid draft; throws an `Error` with the first problem otherwise. */
export function nativeDefaultsPatch(draft: NativeDefaultsDraft): NativeDefaults {
  const problem = Object.values(validateNativeDefaults(draft)).find(Boolean);
  if (problem) throw new Error(problem);
  return { model_id: draft.model_id, permission_mode: draft.permission_mode, ...settingsOf(draft) };
}

/** Whether the draft differs from the saved defaults. */
export function nativeDefaultsChanged(draft: NativeDefaultsDraft, settings: AppPreferences): boolean {
  const saved = nativeDefaultsDraft(settings);
  return (Object.keys(saved) as (keyof NativeDefaultsDraft)[]).some((key) => draft[key] !== saved[key]);
}
