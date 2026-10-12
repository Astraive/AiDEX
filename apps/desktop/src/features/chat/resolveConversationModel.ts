// Which model a conversation uses, and whether it can run now. Pure, so the precedence is testable.
import type { InferenceStatus } from "@aidex/client/dto/inference";
import type { ModelRef, ProviderModel } from "@aidex/client/dto/models";
import type { RuntimeConfig } from "@aidex/client/dto/runtime";
import type { SessionConfig } from "@aidex/client/dto/sessions";
import {
  nativeModelKey,
  parseModelKey,
  sessionModelKey,
} from "../model-manager/catalog/modelKeys";
import { LOCAL_PROVIDER_ID } from "../model-manager/catalog/providerAccounts";

export type ConversationModelState =
  /** Listed and runnable (or a configured, enabled external runtime). */
  | "ready"
  /** The local model is loading; it can run once it is ready. */
  | "loading-local"
  /** The chosen local model is not running. */
  | "local-stopped"
  /** Chosen but not listed now (provider removed, disabled, offline, or the model is gone). */
  | "unlisted"
  /** No explicit or default choice yet. */
  | "none";

export type ResolvedConversationModel = {
  /** The chosen model key; kept even when unlisted so the picker can show it as unavailable. */
  key: string | null;
  /** Listing entry for native models. */
  model?: ProviderModel;
  /** Configuration of an external runtime model. */
  runtime?: RuntimeConfig;
  state: ConversationModelState;
  /** Where the key came from. */
  origin: "session" | "draft" | "preference" | "registry-default" | "local" | "none";
};

export type ResolveConversationModelInput = {
  /** The existing conversation's configuration; its model is never silently replaced. */
  session?: Pick<SessionConfig, "provider_id" | "model_id" | "runtime_id">;
  /** A new chat's own choice from the picker. */
  draftKey?: string | null;
  /** `AppPreferences.model_id`: the default for new chats. */
  preferenceKey: string;
  registryDefault: ModelRef | null;
  /** Listed models AiDEX can stream chat with. */
  models: readonly ProviderModel[];
  inference: InferenceStatus | undefined;
  runtimes: readonly RuntimeConfig[];
};

function resolveKey(
  key: string,
  input: ResolveConversationModelInput,
  origin: ResolvedConversationModel["origin"],
): ResolvedConversationModel {
  const parsed = parseModelKey(key);
  if (!parsed) return { key, state: "unlisted", origin };
  if (parsed.kind === "runtime") {
    const runtime = input.runtimes.find((candidate) => candidate.id === parsed.runtimeId);
    return runtime?.enabled
      ? { key, runtime, state: "ready", origin }
      : { key, runtime, state: "unlisted", origin };
  }
  const model = input.models.find(
    (candidate) =>
      candidate.model.provider_id === parsed.providerId &&
      candidate.model.model_id === parsed.modelId,
  );
  if (parsed.providerId === LOCAL_PROVIDER_ID) {
    // Only one local model is registered at a time, and only while it runs.
    if (model && input.inference?.state === "ready") return { key, model, state: "ready", origin };
    // Loading only when it is this key's model that is starting; another local model does not help.
    if (
      input.inference?.state === "starting" &&
      (!input.inference.model_id || input.inference.model_id === parsed.modelId)
    )
      return { key, state: "loading-local", origin };
    return { key, state: "local-stopped", origin };
  }
  return model ? { key, model, state: "ready", origin } : { key, state: "unlisted", origin };
}

/**
 * Resolution order: the conversation's own model; for a new chat, its draft choice, then the saved
 * default, then the registry default, then a running local model. Never falls back to the first
 * listed remote model, and never replaces a chosen model that is unavailable.
 */
export function resolveConversationModel(
  input: ResolveConversationModelInput,
): ResolvedConversationModel {
  if (input.session) return resolveKey(sessionModelKey(input.session), input, "session");
  if (input.draftKey) return resolveKey(input.draftKey, input, "draft");
  if (input.preferenceKey) return resolveKey(input.preferenceKey, input, "preference");
  if (input.registryDefault)
    return resolveKey(nativeModelKey(input.registryDefault), input, "registry-default");
  const local =
    input.inference?.state === "ready"
      ? input.models.find((model) => model.model.provider_id === LOCAL_PROVIDER_ID)
      : undefined;
  if (local) return { key: nativeModelKey(local.model), model: local, state: "ready", origin: "local" };
  return { key: null, state: "none", origin: "none" };
}
