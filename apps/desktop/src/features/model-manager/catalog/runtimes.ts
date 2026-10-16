// External agent runtimes (Codex, Claude Code, ...) as picker providers and models.
import type { ModelPickerModel, ModelPickerProviderStatus } from "@aidex/ui/components";
import {
  RUNTIME_DEFAULT_MODEL,
  RUNTIME_DISCOVERY_DRIVERS,
  type RuntimeConfig,
  type RuntimeDriver,
  type RuntimeDriverInfo,
  type RuntimeHealth,
  type RuntimeModel,
} from "@aidex/client/dto/runtime";
import { runtimeModelKey, runtimeOwnerId } from "./modelKeys";

const BRANDS: Record<RuntimeDriver, string> = {
  codex: "codex",
  claude_code: "claudecode",
  cursor: "cursor",
  grok: "grok",
  opencode: "opencode",
  antigravity: "antigravity",
  pi: "pi",
  omp: "pi",
  // No glyph ships for these; BrandIcon shows a monogram of the runtime name.
  muse: "muse",
  acp: "acp",
};

const NAMES: Record<RuntimeDriver, string> = {
  codex: "Codex",
  claude_code: "Claude Code",
  cursor: "Cursor",
  grok: "Grok",
  opencode: "OpenCode",
  antigravity: "Antigravity",
  pi: "Pi",
  omp: "Oh My Pi",
  muse: "Muse",
  acp: "ACP",
};

/** Environment variable a vault key must use for each driver; other drivers accept any name. */
export const RUNTIME_VAULT_ENV: Partial<Record<RuntimeDriver, string>> = {
  codex: "OPENAI_API_KEY",
  claude_code: "ANTHROPIC_API_KEY",
  cursor: "CURSOR_API_KEY",
  grok: "XAI_API_KEY",
  antigravity: "GEMINI_API_KEY",
  muse: "META_API_KEY",
};

const CLAUDE_CODE_ALIASES: readonly RuntimeModel[] = [
  { id: "sonnet", name: "Claude Sonnet (latest)" },
  { id: "opus", name: "Claude Opus (latest)" },
  { id: "haiku", name: "Claude Haiku (latest)" },
];

/** What a runtime's health check returned, or why it could not run. */
export type RuntimeHealthState =
  | { state: "loading" }
  | { state: "ready"; health: RuntimeHealth }
  | { state: "error"; message: string };

/** Models a runtime reported through `runtime.models`, fetched on demand and kept for the session. */
export type RuntimeModelsState =
  | { state: "loading" }
  | { state: "ready"; models: RuntimeModel[] }
  | { state: "error"; message: string };

export function runtimeBrand(driver: RuntimeDriver): string {
  return BRANDS[driver] ?? driver;
}

export function runtimeDriverName(
  driver: RuntimeDriver,
  drivers: readonly RuntimeDriverInfo[] = [],
): string {
  return NAMES[driver] ?? drivers.find((info) => info.driver === driver)?.name ?? driver;
}

/** Runtime title: the driver name, plus the runtime id when it is not just the driver. */
export function runtimeName(
  runtime: Pick<RuntimeConfig, "id" | "driver">,
  drivers: readonly RuntimeDriverInfo[] = [],
): string {
  const name = runtimeDriverName(runtime.driver, drivers);
  return runtime.id === runtime.driver ? name : `${name} (${runtime.id})`;
}

/** Splits "one argument per line" text into argv, dropping blank lines. */
export function parseArgv(text: string): string[] {
  return text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
}

export function supportsModelDiscovery(driver: RuntimeDriver): boolean {
  return RUNTIME_DISCOVERY_DRIVERS.includes(driver);
}

/**
 * What a `runtime.health` status means to the screens. Besides the documented statuses, a runtime
 * with an account AiDEX manages (Codex) can report its account's status (`runtime.account.status`):
 * `authenticated_cached` (signed in, but the runtime could not be checked, e.g. offline),
 * `auth_pending` (a sign-in is running), and `signed_out`. Unknown codes stay `unknown` and are shown
 * as a generic warning, never as raw text.
 */
export type RuntimeHealthKind =
  | "ready"
  | "binary_missing"
  | "auth_required"
  | "auth_pending"
  | "signed_in_unverified"
  | "auth_unverified"
  | "unknown";

export function runtimeHealthKind(status: string): RuntimeHealthKind {
  switch (status) {
    case "ready":
      return "ready";
    case "binary_missing":
      return "binary_missing";
    case "auth_required":
    case "signed_out":
      return "auth_required";
    case "auth_pending":
      return "auth_pending";
    case "authenticated_cached":
      return "signed_in_unverified";
    case "installed_auth_unverified":
    case "protocol_ready_auth_unverified":
      return "auth_unverified";
    default:
      return "unknown";
  }
}

/** Picker readiness of a runtime from its health check. */
export function runtimePickerStatus(
  health: RuntimeHealthState | undefined,
  name: string,
): { status: ModelPickerProviderStatus; statusText?: string } {
  if (!health || health.state === "loading")
    return { status: "warning", statusText: "Checking runtime status." };
  if (health.state === "error") return { status: "error", statusText: health.message };
  switch (runtimeHealthKind(health.health.status)) {
    case "ready":
      return { status: "ready" };
    case "binary_missing":
      return { status: "setup", statusText: `Install ${name} in Providers to use it.` };
    case "auth_required":
      return { status: "setup" };
    case "auth_pending":
      return { status: "warning", statusText: `A ${name} sign-in is in progress.` };
    case "signed_in_unverified":
      return { status: "warning", statusText: "Signed in; the runtime could not be checked right now." };
    case "auth_unverified":
      return { status: "warning", statusText: "Installed; sign-in could not be verified." };
    case "unknown":
      return { status: "warning", statusText: "The runtime reported a status this app does not know." };
  }
}

/**
 * Picker rows for one runtime: "Default model" (the runtime's own configured default), Claude Code's
 * family aliases, then any models the runtime reported.
 */
export function runtimePickerModels(
  runtime: RuntimeConfig,
  discovered: RuntimeModelsState | undefined,
): ModelPickerModel[] {
  const providerId = runtimeOwnerId(runtime.id);
  const rows: ModelPickerModel[] = [
    {
      key: runtimeModelKey(runtime.id, RUNTIME_DEFAULT_MODEL),
      providerId,
      name: "Default model",
      description: "Uses the model configured in the runtime itself.",
      capabilities: ["tools"],
      badge: "default",
    },
  ];
  const seen = new Set<string>([RUNTIME_DEFAULT_MODEL]);
  const extra = [
    ...(runtime.driver === "claude_code" ? CLAUDE_CODE_ALIASES : []),
    ...(discovered?.state === "ready" ? discovered.models : []),
  ];
  for (const model of extra) {
    if (seen.has(model.id)) continue;
    seen.add(model.id);
    rows.push({
      key: runtimeModelKey(runtime.id, model.id),
      providerId,
      name: model.name || model.id,
      description: model.id,
      capabilities: ["tools"],
    });
  }
  return rows;
}
