// Vault secrets for MCP servers (`mcp:<id>`) and web search providers (`process:web`). Values go
// only to `vault.secret.set`; this module never keeps, logs, or returns one.
import { call, callUntyped } from "@aidex/client/rpc";

type SecretListing = {
  secrets?: { name?: string; kind?: string; policy?: { allowed_consumers?: string[] } }[];
};

export type StoredSecret = { name: string; kind?: string; consumers: readonly string[] };

/** Names, kinds, and allowed consumers of every vault secret (no values). Needs an unlocked vault. */
export async function listSecrets(): Promise<Map<string, StoredSecret>> {
  const listing = (await callUntyped("vault.secret.list")) as SecretListing;
  const secrets = new Map<string, StoredSecret>();
  for (const secret of listing.secrets ?? []) {
    if (!secret.name) continue;
    secrets.set(secret.name, {
      name: secret.name,
      kind: secret.kind,
      consumers: secret.policy?.allowed_consumers ?? [],
    });
  }
  return secrets;
}

/**
 * Stores `value` under `name` and releases it to `consumer`. Consumers and the kind the secret
 * already has are kept, so a secret shared with a provider or another server keeps working there.
 */
export async function storeSecret(name: string, value: string, consumer: string, kind = "api_key"): Promise<void> {
  const existing = (await listSecrets()).get(name);
  const consumers = existing?.consumers ?? [];
  await call("vault.secret.set", {
    name,
    value,
    consumers: consumers.includes(consumer) ? [...consumers] : [...consumers, consumer],
    kind: existing?.kind ?? kind,
  });
}

/** Vault secret names: letters, digits, `.`, `_`, `-`, starting with a letter or digit, at most 128. */
export const SECRET_NAME_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;
