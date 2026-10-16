// A conversation's instructions, fallback model, and step and token limits, with the bounds the
// daemon enforces (`SessionConfig::validate` in crates/native-sessions/src/model.rs, and the same
// bounds for the defaults in `AppPreferences`). Pure, so the conversation settings dialog and the
// Agents defaults form check them the same way.
import type { ProviderModel } from "@aidex/client/dto/models";
import { parseModelKey } from "../model-manager/catalog/modelKeys";

/** The fields of a `SessionConfig` (and of `AppPreferences`) that shape every run. */
export type SessionSettings = {
  instructions: string;
  /** A native `provider/model` key tried once after a temporary failure, or null. */
  fallback_model: string | null;
  max_steps: number;
  max_tokens: number;
};

export const MAX_STEPS = 128;
export const MAX_TOKENS = 32_768;
/** Instructions are limited in UTF-8 bytes, not characters. */
export const MAX_INSTRUCTION_BYTES = 32 * 1024;
/** Longest model key the preferences accept. */
export const MAX_MODEL_KEY_BYTES = 512;

/** Field text as the forms hold it. */
export type SessionSettingsDraft = {
  instructions: string;
  /** "" for no fallback. */
  fallback_model: string;
  max_steps: string;
  max_tokens: string;
};

export type SessionSettingsErrors = Partial<Record<keyof SessionSettingsDraft, string>>;

const utf8 = new TextEncoder();

export function byteLength(text: string): number {
  return utf8.encode(text).length;
}

/** A whole number written with digits only; NaN for anything else (signs, decimals, exponents, blanks). */
export function parseWholeNumber(text: string): number {
  const trimmed = text.trim();
  return /^\d{1,9}$/.test(trimmed) ? Number(trimmed) : Number.NaN;
}

export function draftOf(settings: SessionSettings): SessionSettingsDraft {
  return {
    instructions: settings.instructions,
    fallback_model: settings.fallback_model ?? "",
    max_steps: String(settings.max_steps),
    max_tokens: String(settings.max_tokens),
  };
}

function limitError(text: string, max: number, noun: string): string | undefined {
  const value = parseWholeNumber(text);
  return Number.isInteger(value) && value >= 1 && value <= max
    ? undefined
    : `Enter a whole number of ${noun} from 1 to ${max.toLocaleString("en-US")}.`;
}

/** Whether `key` is a model AiDEX's own harness can fall back to: a native `provider/model` key. */
export function isNativeFallbackKey(key: string): boolean {
  return parseModelKey(key)?.kind === "native" && byteLength(key) <= MAX_MODEL_KEY_BYTES;
}

/** Field errors for a draft, keyed like the draft; empty when it can be saved. */
export function sessionSettingsErrors(draft: SessionSettingsDraft): SessionSettingsErrors {
  const errors: SessionSettingsErrors = {};
  const bytes = byteLength(draft.instructions);
  if (bytes > MAX_INSTRUCTION_BYTES)
    errors.instructions = `Shorten the instructions to 32 KiB (they are ${Math.ceil(bytes / 1024)} KiB).`;
  if (draft.fallback_model && !isNativeFallbackKey(draft.fallback_model))
    errors.fallback_model = "Choose a fallback model from a connected provider.";
  const steps = limitError(draft.max_steps, MAX_STEPS, "steps");
  if (steps) errors.max_steps = steps;
  const tokens = limitError(draft.max_tokens, MAX_TOKENS, "tokens");
  if (tokens) errors.max_tokens = tokens;
  return errors;
}

export function hasErrors(errors: Partial<Record<string, string>>): boolean {
  return Object.values(errors).some(Boolean);
}

/** The values a valid draft describes; throws an `Error` with the first problem otherwise. */
export function settingsOf(draft: SessionSettingsDraft): SessionSettings {
  const problem = Object.values(sessionSettingsErrors(draft)).find(Boolean);
  if (problem) throw new Error(problem);
  return {
    instructions: draft.instructions,
    fallback_model: draft.fallback_model || null,
    max_steps: parseWholeNumber(draft.max_steps),
    max_tokens: parseWholeNumber(draft.max_tokens),
  };
}

/** A picker row: its key, name, and whether it can be chosen. */
type ListedModel = { key: string; name: string; unavailable?: boolean; disabledReason?: string | null };

/**
 * Fallback choices: "No fallback", then native chat models in picker order. Agent conversations only
 * get models with native tool calling (the harness refuses any other fallback there). The model the
 * fallback stands in for is left out; a saved fallback that is no longer listed stays, marked.
 */
export function fallbackModelOptions(input: {
  models: readonly ListedModel[];
  findModel: (key: string) => ProviderModel | undefined;
  providerName: (key: string) => string | undefined;
  /** Agent conversations need a fallback with native tool calling; omit for new-chat defaults. */
  agent?: boolean;
  /** The conversation's own model. */
  primaryKey?: string | null;
  /** The saved fallback. */
  current?: string | null;
}): { value: string; label: string }[] {
  const options = [{ value: "", label: "No fallback" }];
  for (const model of input.models) {
    if (model.unavailable || model.disabledReason || model.key === input.primaryKey) continue;
    if (parseModelKey(model.key)?.kind !== "native") continue;
    const descriptor = input.findModel(model.key);
    if (!descriptor?.capabilities.chat || !descriptor.capabilities.streaming) continue;
    if (input.agent && descriptor.capabilities.tool_calls !== "native") continue;
    const provider = input.providerName(model.key);
    options.push({ value: model.key, label: provider ? `${model.name} · ${provider}` : model.name });
  }
  const current = input.current;
  if (current && !options.some((option) => option.value === current)) {
    const descriptor = input.findModel(current);
    const name = descriptor?.display_name ?? parseModelKey(current)?.modelId ?? current;
    const why =
      current === input.primaryKey
        ? "this conversation's own model"
        : descriptor && input.agent && descriptor.capabilities.tool_calls !== "native"
          ? "can't use agent tools"
          : "not available now";
    options.push({ value: current, label: `${name} (${why})` });
  }
  return options;
}
