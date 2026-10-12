// Rows of the providers screen: configured accounts, the local model, and external runtimes, each
// with t3code-style status copy (headline, detail, tone).
import type { ProviderRowTone } from "@aidex/ui/components";
import type {
  ModelList,
  ProviderAccount,
  ProviderFailure,
  ProviderModel,
  ProviderPreset,
} from "@aidex/client/dto/models";
import type { RuntimeConfig, RuntimeDriverInfo } from "@aidex/client/dto/runtime";
import { providerFailure } from "../catalog/buildCatalog";
import { isChatModel } from "../catalog/modelFacts";
import {
  KIND_LABELS,
  accountBrand,
  accountTitles,
  isAuthFailure,
  isRuntimeAccount,
  presetForAccount,
} from "../catalog/providerAccounts";
import {
  runtimeBrand,
  runtimeName,
  type RuntimeHealthState,
} from "../catalog/runtimes";
import { providerSelectionId } from "./selection";

export type ProviderSummary = { headline: string; detail?: string; tone: ProviderRowTone };

export type AccountItem = {
  kind: "provider";
  selectionId: string;
  id: string;
  title: string;
  brand: string;
  /** Monospace tag after the title: the provider kind. */
  version: string;
  summary: ProviderSummary;
  account: ProviderAccount;
  preset: ProviderPreset | undefined;
  /** Every model the latest listing reported for this account. */
  models: ProviderModel[];
  failure: ProviderFailure | undefined;
};

export type RuntimeItem = {
  kind: "runtime";
  selectionId: string;
  id: string;
  title: string;
  brand: string;
  /** Monospace tag after the title: the driver protocol. */
  version: string;
  summary: ProviderSummary;
  runtime: RuntimeConfig;
  driver: RuntimeDriverInfo | undefined;
  health: RuntimeHealthState | undefined;
};

export type ProviderItem = AccountItem | RuntimeItem;

/** Status line for an account row (t3code `getProviderSummary`, adapted). */
export function accountSummary(
  account: ProviderAccount,
  failure: ProviderFailure | undefined,
  models: readonly ProviderModel[],
  listed: boolean,
): ProviderSummary {
  if (!account.enabled)
    return {
      headline: "Disabled",
      detail: "This provider is disabled for new conversations.",
      tone: "neutral",
    };
  if (failure) {
    if (isAuthFailure(failure.kind))
      return { headline: "Not authenticated", detail: failure.message, tone: "warning" };
    return { headline: "Unavailable", detail: failure.message, tone: "danger" };
  }
  if (!listed && !account.catalog)
    return {
      headline: "Checking provider status",
      detail: "Models have not been listed in this session yet.",
      tone: "neutral",
    };
  const chat = models.filter(isChatModel).length;
  if (chat === 0)
    return { headline: "Needs attention", detail: "No chat models are available.", tone: "warning" };
  return {
    headline: "Available",
    detail: `${models.length} ${models.length === 1 ? "model" : "models"}`,
    tone: "success",
  };
}

/** Status line for a runtime row. */
export function runtimeSummary(
  runtime: RuntimeConfig,
  health: RuntimeHealthState | undefined,
  name: string,
): ProviderSummary {
  if (!runtime.enabled)
    return {
      headline: "Disabled",
      detail: "This runtime is disabled for new conversations.",
      tone: "neutral",
    };
  if (!health || health.state === "loading")
    return { headline: "Checking runtime status", tone: "neutral" };
  if (health.state === "error") return { headline: "Unavailable", detail: health.message, tone: "danger" };
  switch (health.health.status) {
    case "ready":
      return { headline: "Available", tone: "success" };
    case "binary_missing":
      return {
        headline: "Not found",
        detail: `${name} is not installed on this drive.`,
        tone: "danger",
      };
    case "auth_required":
      return { headline: "Not authenticated", detail: "Sign in to use this runtime.", tone: "warning" };
    case "installed_auth_unverified":
    case "protocol_ready_auth_unverified":
      return {
        headline: "Needs attention",
        detail: "Installed; sign-in could not be verified.",
        tone: "warning",
      };
    default:
      return { headline: "Needs attention", detail: health.health.status, tone: "warning" };
  }
}

export function buildProviderItems(input: {
  accounts: readonly ProviderAccount[];
  presets: readonly ProviderPreset[];
  modelList: ModelList | undefined;
  runtimes: readonly RuntimeConfig[];
  drivers: readonly RuntimeDriverInfo[];
  runtimeHealth: Readonly<Record<string, RuntimeHealthState>>;
}): { accounts: AccountItem[]; runtimes: RuntimeItem[] } {
  const titles = accountTitles(input.accounts, input.presets);
  const accounts = input.accounts.map((account): AccountItem => {
    const id = account.descriptor.id;
    const preset = presetForAccount(account, input.presets);
    const models = (input.modelList?.models ?? []).filter((model) => model.model.provider_id === id);
    const failure = providerFailure(account, id, input.modelList);
    return {
      kind: "provider",
      selectionId: providerSelectionId("provider", id),
      id,
      title: titles.get(id) ?? id,
      brand: accountBrand(account, preset),
      version: KIND_LABELS[account.descriptor.kind] ?? account.descriptor.kind,
      summary: accountSummary(account, failure, models, !!input.modelList),
      account,
      preset,
      models,
      failure,
    };
  });
  // Configured accounts first; the local model AiDEX registers itself comes last.
  accounts.sort(
    (left, right) => Number(isRuntimeAccount(left.account)) - Number(isRuntimeAccount(right.account)),
  );
  const runtimes = input.runtimes.map((runtime): RuntimeItem => {
    const title = runtimeName(runtime, input.drivers);
    const driver = input.drivers.find((info) => info.driver === runtime.driver);
    const health = input.runtimeHealth[runtime.id];
    return {
      kind: "runtime",
      selectionId: providerSelectionId("runtime", runtime.id),
      id: runtime.id,
      title,
      brand: runtimeBrand(runtime.driver),
      version: driver?.protocol ?? runtime.driver,
      summary: runtimeSummary(runtime, health, title),
      runtime,
      driver,
      health,
    };
  });
  return { accounts, runtimes };
}

/** Newest catalog listing time across accounts, for "Checked …". */
export function lastCheckedAt(accounts: readonly ProviderAccount[]): string | null {
  let newest: string | null = null;
  for (const account of accounts) {
    const at = account.catalog?.fetched_at;
    if (at && (!newest || Date.parse(at) > Date.parse(newest))) newest = at;
  }
  return newest;
}

/** "just now", "5 min ago", "3 h ago", "2 days ago"; null for an unreadable timestamp. */
export function relativeTime(iso: string, now: number): string | null {
  const at = Date.parse(iso);
  if (!Number.isFinite(at)) return null;
  const seconds = Math.max(0, Math.round((now - at) / 1000));
  if (seconds < 45) return "just now";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours} h ago`;
  const days = Math.round(hours / 24);
  return `${days} ${days === 1 ? "day" : "days"} ago`;
}
