// Client-side checks for vault secrets that mirror the daemon's rules, so the form can explain a refusal
// before sending anything: names (`aidex_vault::SecretName`), exact consumer scopes
// (crates/vault/src/scope.rs), and the records AiDEX itself owns. The daemon enforces all of them again.
import type { SecretMeta } from "@aidex/client/dto/vault";

/** `aidex_vault::SecretPolicy`: at most this many consumers per secret. */
export const MAX_CONSUMERS = 32;

const NAME = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;
const IDENTIFIER = /^[a-z0-9][a-z0-9._-]{0,127}$/;
const HOST_LABEL = /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/;

/** Records AiDEX manages through their own feature: encryption keys and encrypted settings. */
const MANAGED_NAMES = new Set([
  "aidex-preferences",
  "aidex-model-preferences",
  "native-record-encryption-key",
  "external-runtime-encryption-key",
]);

/** Prefix of provider sign-in records (`oauth.<provider>…`), owned by the sign-in flow. */
const MANAGED_PREFIX = "oauth.";

/** Whether `name` is a valid vault secret name: 1-128 of `A-Z a-z 0-9 . _ -`, starting with a letter or digit. */
export const validSecretName = (name: string): boolean => NAME.test(name);

/**
 * Whether `text` is an exact consumer scope the vault accepts: `provider:`, `mcp:`, or `process:` with
 * a lowercase identifier, or `git:` with a lowercase host and an optional port. There are no wildcards,
 * and models, prompts, agents, sessions, logs, and shells can never receive a secret.
 */
export function validConsumer(text: string): boolean {
  const separator = text.indexOf(":");
  if (separator < 1) return false;
  const kind = text.slice(0, separator);
  const target = text.slice(separator + 1);
  if (kind === "provider" || kind === "mcp" || kind === "process") return IDENTIFIER.test(target);
  if (kind !== "git" || !target || target.length > 128) return false;
  const [host, port, extra] = target.split(":");
  if (extra !== undefined) return false;
  if (port !== undefined && (!/^\d{1,5}$/.test(port) || Number(port) < 1 || Number(port) > 65535)) return false;
  return host.split(".").every((label) => HOST_LABEL.test(label));
}

/** Consumers typed one per line (or comma separated), trimmed and without duplicates. */
export const consumerList = (text: string): string[] => [
  ...new Set(
    text
      .split(/[\n,]/)
      .map((value) => value.trim())
      .filter(Boolean),
  ),
];

/** Why `name` with `consumers` would be refused, or `undefined` when the vault accepts it. */
export function secretPolicyError(name: string, consumers: readonly string[]): string | undefined {
  if (!validSecretName(name))
    return "Use 1–128 letters, digits, dots, underscores, or hyphens, starting with a letter or digit.";
  if (consumers.length > MAX_CONSUMERS) return `A secret can have at most ${MAX_CONSUMERS} consumers.`;
  const invalid = consumers.find((consumer) => !validConsumer(consumer));
  if (invalid)
    return `"${invalid}" is not an exact consumer. Use provider:<id>, mcp:<id>, process:<id>, or git:<host[:port]> in lowercase; wildcards, models, prompts, agents, and shells cannot receive secrets.`;
  return undefined;
}

/**
 * Whether AiDEX manages this record itself (encryption keys, encrypted settings, provider sign-ins), so
 * the vault screen must not replace or delete it. The daemon's `managed` list is authoritative; this
 * is the same rule for when that list is missing.
 */
export function managedSecret(meta: Pick<SecretMeta, "name">, managed: readonly string[] = []): boolean {
  return managed.includes(meta.name) || MANAGED_NAMES.has(meta.name) || meta.name.startsWith(MANAGED_PREFIX);
}

/** Why a managed record cannot be changed here. */
export const MANAGED_MESSAGE = "AiDEX manages this record itself. Change it from the feature that owns it.";

/**
 * The message to show for a failed `vault.secret.set` or `vault.secret.delete`: the managed-record
 * explanation when the daemon refused with `data.reason = "managed_secret"`, otherwise its own message.
 */
export function secretWriteError(error: { message: string; data?: unknown }): string {
  const data = error.data;
  const reason = typeof data === "object" && data !== null ? (data as { reason?: unknown }).reason : undefined;
  return reason === "managed_secret" ? MANAGED_MESSAGE : error.message;
}
