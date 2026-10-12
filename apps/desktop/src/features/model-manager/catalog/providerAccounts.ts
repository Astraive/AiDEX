// Display facts for configured provider accounts: title, brand glyph, preset, and sign-in style.
import type {
  ProviderAccount,
  ProviderAddParams,
  ProviderErrorKind,
  ProviderKind,
  ProviderPreset,
} from "@aidex/client/dto/models";

/** The llama.cpp provider AiDEX registers while a local model runs. */
export const LOCAL_PROVIDER_ID = "local-llama";
export const LOCAL_PROVIDER_NAME = "Local model";
export const LOCAL_PROVIDER_BRAND = "llamacpp";

const PRESET_BY_KIND: Partial<Record<ProviderKind, string>> = {
  "openai-responses": "openai",
  anthropic: "anthropic",
  gemini: "gemini",
  "azure-openai": "azure-openai",
  bedrock: "bedrock",
};

export const KIND_LABELS: Record<ProviderKind, string> = {
  "openai-compatible": "openai-compatible",
  "openai-responses": "openai-responses",
  anthropic: "anthropic",
  gemini: "gemini",
  "azure-openai": "azure-openai",
  bedrock: "bedrock",
  "llama-cpp": "llama.cpp",
  fake: "fake",
};

/** Accounts created by "Continue with ChatGPT" keep their tokens under `oauth.<id>.*` vault names. */
export function isOAuthAccount(account: ProviderAccount): boolean {
  const auth = account.descriptor.auth;
  return auth.type === "vault_secret" && auth.secret_name.startsWith("oauth.");
}

/** Providers AiDEX registers itself (the local llama.cpp model) rather than configured accounts. */
export function isRuntimeAccount(account: ProviderAccount): boolean {
  return account.origin === "runtime";
}

/** Failure kinds that mean the account needs a credential or a new sign-in. */
export function isAuthFailure(kind: ProviderErrorKind | undefined): boolean {
  return kind === "credential_unavailable" || kind === "unauthorized";
}

function normalizeUrl(url: string): string {
  return url.trim().replace(/\/+$/, "").toLowerCase();
}

/** The preset an account was created from; older accounts without `preset_id` are matched by kind and endpoint. */
export function presetForAccount(
  account: ProviderAccount,
  presets: readonly ProviderPreset[],
): ProviderPreset | undefined {
  if (account.preset_id) {
    const recorded = presets.find((preset) => preset.id === account.preset_id);
    if (recorded) return recorded;
  }
  if (isOAuthAccount(account)) return presets.find((preset) => preset.id === "openai");
  const byKind = PRESET_BY_KIND[account.descriptor.kind];
  if (byKind) return presets.find((preset) => preset.id === byKind);
  const baseUrl = account.descriptor.base_url;
  if (!baseUrl) return undefined;
  const url = normalizeUrl(baseUrl);
  return presets.find(
    (preset) =>
      preset.kind === account.descriptor.kind &&
      !!preset.base_url &&
      normalizeUrl(preset.base_url) === url,
  );
}

/** Brand glyph key for an account; unknown keys render a monogram. */
export function accountBrand(account: ProviderAccount, preset: ProviderPreset | undefined): string {
  if (account.descriptor.id === LOCAL_PROVIDER_ID || account.descriptor.kind === "llama-cpp")
    return LOCAL_PROVIDER_BRAND;
  return preset?.icon || "custom";
}

/** Name before disambiguation: the account's own name, then the preset's, then the id. */
function baseTitle(account: ProviderAccount, preset: ProviderPreset | undefined): string {
  if (account.display_name) return account.display_name;
  if (account.descriptor.id === LOCAL_PROVIDER_ID) return LOCAL_PROVIDER_NAME;
  return preset?.display_name ?? account.descriptor.id;
}

/**
 * Titles for every account, keyed by id. When two accounts would share a title (for example two
 * unnamed OpenAI accounts), the account id is appended so rows and rail entries stay distinguishable.
 */
export function accountTitles(
  accounts: readonly ProviderAccount[],
  presets: readonly ProviderPreset[],
): Map<string, string> {
  const base = accounts.map((account) => ({
    id: account.descriptor.id,
    title: baseTitle(account, presetForAccount(account, presets)),
  }));
  const counts = new Map<string, number>();
  for (const entry of base) counts.set(entry.title, (counts.get(entry.title) ?? 0) + 1);
  return new Map(
    base.map((entry) => [
      entry.id,
      (counts.get(entry.title) ?? 0) > 1 && entry.title !== entry.id
        ? `${entry.title} (${entry.id})`
        : entry.title,
    ]),
  );
}

/**
 * `provider.update` params that keep everything the account has today. `provider.update` replaces the
 * whole account, so every edit starts from this and overrides only what changes.
 */
export function accountUpdateParams(
  account: ProviderAccount,
  overrides: Partial<ProviderAddParams> = {},
): ProviderAddParams {
  const { descriptor } = account;
  const baseUrl = descriptor.base_url ?? "";
  return {
    id: descriptor.id,
    kind: descriptor.kind,
    base_url: baseUrl,
    secret_name: descriptor.auth.type === "vault_secret" ? descriptor.auth.secret_name : undefined,
    // Keep a remote classification (e.g. a loopback tunnel to a cloud service). The daemon accepts
    // `remote` for any host and decides loopback itself, so the client never re-derives it.
    locality: descriptor.locality === "remote" ? "remote" : undefined,
    discover_models: account.discover_models,
    models: account.declared_models,
    display_name: account.display_name,
    preset_id: account.preset_id,
    enabled: account.enabled,
    ...overrides,
  };
}
