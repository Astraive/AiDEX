// Display facts for `model.list` descriptors: picker rows, capability words, and capability sources.
import {
  formatContextWindow,
  type ModelPickerCapability,
  type ModelPickerModel,
} from "@aidex/ui/components";
import type { CapabilitySource, ProviderModel } from "@aidex/client/dto/models";
import { nativeModelKey } from "./modelKeys";

/** Chat models AiDEX can run: the harness needs streamed chat. */
export function isChatModel(model: ProviderModel): boolean {
  return model.capabilities.chat && model.capabilities.streaming;
}

export function modelCapabilities(model: ProviderModel): ModelPickerCapability[] {
  const capabilities: ModelPickerCapability[] = [];
  if (model.capabilities.tool_calls === "native") capabilities.push("tools");
  if (model.capabilities.vision) capabilities.push("vision");
  if (model.capabilities.json_schema_output) capabilities.push("json");
  return capabilities;
}

/** Badge words for the providers screen and model details, in display order. */
export function capabilityBadges(model: ProviderModel): string[] {
  return [
    ...(model.capabilities.tool_calls === "native" ? ["Tools"] : []),
    ...(model.capabilities.vision ? ["Vision"] : []),
    ...(model.capabilities.json_schema_output ? ["JSON"] : []),
    ...(model.capabilities.embeddings ? ["Embeddings"] : []),
  ];
}

/** "200K context", or null when the provider did not report a limit. */
export function contextLabel(tokens: number | null | undefined): string | null {
  const compact = formatContextWindow(tokens);
  return compact ? `${compact} context` : null;
}

export const SOURCE_LABELS: Record<CapabilitySource, { label: string; title: string }> = {
  discovered: { label: "Discovered", title: "Reported by the provider" },
  catalog: { label: "Catalog", title: "Known model family; limits are conservative" },
  configured: { label: "Configured", title: "Declared for this account" },
  assumed: { label: "Assumed", title: "Limits unknown; choose explicitly" },
};

export const TOOL_LABELS = {
  native: "Native tools",
  structured: "Structured tools",
  none: "No tools",
} as const;

/** One picker row for a native model. `custom` marks models declared for a configured account. */
export function toPickerModel(
  model: ProviderModel,
  options: { defaultKey: string | null; custom: boolean },
): ModelPickerModel {
  const key = nativeModelKey(model.model);
  return {
    key,
    providerId: model.model.provider_id,
    name: model.display_name || model.model.model_id,
    description: model.model.model_id,
    contextWindow: model.context_window || null,
    capabilities: modelCapabilities(model),
    badge: key === options.defaultKey ? "default" : options.custom ? "custom" : undefined,
  };
}
