// Model keys used by the picker, favorites, hidden models, and `AppPreferences.model_id`.
// Native models: `<provider_id>/<model_id>`. External runtimes: `runtime:<runtime_id>/<model_id>`.
// Provider and runtime ids never contain "/", so a key splits at its first "/" (model ids may contain more).
import type { ModelRef } from "@aidex/client/dto/models";
import { RUNTIME_PROVIDER_ID } from "@aidex/client/dto/runtime";
import type { SessionConfig } from "@aidex/client/dto/sessions";

export const RUNTIME_KEY_PREFIX = "runtime:";
/** `runtime_id` value that means AiDEX's own harness rather than an external runtime. */
const NATIVE_RUNTIME_ID = "native";

export type ParsedModelKey =
  | { kind: "native"; providerId: string; modelId: string }
  | { kind: "runtime"; runtimeId: string; modelId: string };

/** The provider, model, and runtime fields a conversation configuration needs for one model. */
export type ModelTarget = {
  provider_id: string;
  model_id: string;
  runtime_id: string | null;
};

export function nativeModelKey(ref: ModelRef): string {
  return `${ref.provider_id}/${ref.model_id}`;
}

export function runtimeModelKey(runtimeId: string, modelId: string): string {
  return `${RUNTIME_KEY_PREFIX}${runtimeId}/${modelId}`;
}

/** Splits a model key; null for an empty or malformed key. */
export function parseModelKey(key: string | null | undefined): ParsedModelKey | null {
  if (!key) return null;
  const runtime = key.startsWith(RUNTIME_KEY_PREFIX);
  const body = runtime ? key.slice(RUNTIME_KEY_PREFIX.length) : key;
  const slash = body.indexOf("/");
  if (slash <= 0 || slash === body.length - 1) return null;
  const owner = body.slice(0, slash);
  const modelId = body.slice(slash + 1);
  return runtime
    ? { kind: "runtime", runtimeId: owner, modelId }
    : { kind: "native", providerId: owner, modelId };
}

/** True when a session configuration runs on an external runtime rather than AiDEX's harness. */
export function isExternalRuntime(runtimeId: string | null | undefined): runtimeId is string {
  return !!runtimeId && runtimeId !== NATIVE_RUNTIME_ID;
}

/** The model key a conversation is bound to. */
export function sessionModelKey(
  config: Pick<SessionConfig, "provider_id" | "model_id" | "runtime_id">,
): string {
  return isExternalRuntime(config.runtime_id)
    ? runtimeModelKey(config.runtime_id, config.model_id)
    : nativeModelKey(config);
}

/** Configuration fields for a key. Runtime sessions record the placeholder provider id. */
export function modelTargetOf(parsed: ParsedModelKey): ModelTarget {
  return parsed.kind === "runtime"
    ? { provider_id: RUNTIME_PROVIDER_ID, model_id: parsed.modelId, runtime_id: parsed.runtimeId }
    : { provider_id: parsed.providerId, model_id: parsed.modelId, runtime_id: null };
}

/** Rail id of the provider or runtime that owns a key; runtimes use their key prefix so ids never collide. */
export function ownerIdOf(parsed: ParsedModelKey): string {
  return parsed.kind === "runtime" ? runtimeOwnerId(parsed.runtimeId) : parsed.providerId;
}

/** Picker rail id of an external runtime. */
export function runtimeOwnerId(runtimeId: string): string {
  return `${RUNTIME_KEY_PREFIX}${runtimeId}`;
}
