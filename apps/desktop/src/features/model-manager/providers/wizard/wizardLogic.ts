// Pure helpers for the Add provider wizard: catalog filtering, badges, ids, and preset rules.
import type { PresetCategory, ProviderPreset } from "@aidex/client/dto/models";
import type { RuntimeDriverInfo } from "@aidex/client/dto/runtime";

export type WizardCategory = "all" | PresetCategory;

export const CATEGORY_OPTIONS: readonly { value: WizardCategory; label: string }[] = [
  { value: "all", label: "All" },
  { value: "cloud", label: "Cloud" },
  { value: "gateway", label: "Gateways" },
  { value: "local", label: "Local" },
  { value: "enterprise", label: "Enterprise" },
  { value: "custom", label: "Custom" },
];

/** Account ids the daemon accepts (`ProviderId`, kept to lowercase here). */
export const ACCOUNT_ID = /^[a-z0-9][a-z0-9_-]{0,63}$/;
/**
 * Runtime ids for new runtimes. Kept lowercase so the vault consumer `process:runtime-<id>` that
 * releases a runtime's key is always valid (the daemon itself also accepts capitals).
 */
export const RUNTIME_ID = /^[a-z0-9][a-z0-9_-]{0,63}$/;

/** Lowercase slug of `text` that fits an account or runtime id. */
export function slugify(text: string, fallback = "provider"): string {
  const slug = text
    .toLowerCase()
    .replace(/[^a-z0-9_-]+/g, "-")
    .replace(/^[-_]+|[-_]+$/g, "")
    .slice(0, 56);
  return slug || fallback;
}

/** `base`, or `base-2`, `base-3`, … when taken. */
export function uniqueId(base: string, taken: ReadonlySet<string>): string {
  if (!taken.has(base)) return base;
  for (let index = 2; ; index += 1) {
    const candidate = `${base}-${index}`;
    if (!taken.has(candidate)) return candidate;
  }
}

/** Whether an account needs, accepts, or has no API key. */
export type KeyMode = "required" | "optional" | "none";

export function keyMode(preset: ProviderPreset): KeyMode {
  const methods = preset.auth_methods;
  const keyed = methods.includes("api_key") || methods.includes("aws_sigv4");
  if (methods.includes("none")) return keyed ? "optional" : "none";
  // Presets from an older daemon have no auth methods; they always took a vault key.
  return "required";
}

/** Endpoints that must be typed in: Azure resources, Bedrock regions, and custom servers have no default. */
export function requiresEndpoint(preset: ProviderPreset): boolean {
  return !preset.base_url;
}

/** Example endpoint for presets the user must type one for. */
export function endpointPlaceholder(preset: ProviderPreset): string | undefined {
  if (preset.kind === "azure-openai") return "https://<resource>.openai.azure.com/openai";
  if (preset.kind === "bedrock") return "https://bedrock-runtime.us-east-1.amazonaws.com";
  return undefined;
}

/** What the endpoint field expects, matching the daemon's URL contract for each kind. */
export function endpointHint(preset: ProviderPreset): string {
  if (preset.kind === "azure-openai")
    return "Your resource's API root, for example https://<resource>.openai.azure.com/openai. AiDEX adds the deployment path and API version.";
  if (preset.kind === "bedrock")
    return "The Bedrock runtime endpoint for your region, for example https://bedrock-runtime.us-east-1.amazonaws.com.";
  return "HTTPS, or http:// for a server on this computer.";
}

/** Kinds that cannot list their models, so the wizard asks for one deployment or model id. */
export function declaresModels(preset: ProviderPreset): boolean {
  return preset.kind === "azure-openai" || preset.kind === "bedrock";
}

export function supportsChatGpt(preset: ProviderPreset): boolean {
  return preset.oauth_supported || preset.auth_methods.includes("oauth_chatgpt");
}

/** Badges for a preset tile. */
export function presetBadges(preset: ProviderPreset): string[] {
  const badges: string[] = [];
  if (supportsChatGpt(preset)) badges.push("ChatGPT sign-in");
  const mode = keyMode(preset);
  if (preset.auth_methods.includes("aws_sigv4")) badges.push("AWS credentials");
  else if (mode === "required") badges.push("API key");
  else if (mode === "optional") badges.push("Key optional");
  else badges.push("No key");
  if (preset.category === "local") badges.push("Local");
  return badges;
}

function matchesQuery(query: string, ...fields: (string | null | undefined)[]): boolean {
  const tokens = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  if (!tokens.length) return true;
  const text = fields.filter(Boolean).join(" ").toLowerCase();
  return tokens.every((token) => text.includes(token));
}

export function presetMatches(preset: ProviderPreset, category: WizardCategory, query: string): boolean {
  if (category !== "all" && preset.category !== category) return false;
  return matchesQuery(query, preset.display_name, preset.id, preset.description, preset.category);
}

export function runtimeMatches(driver: RuntimeDriverInfo, query: string): boolean {
  return matchesQuery(query, driver.name, driver.driver, driver.protocol, "runtime agent");
}
