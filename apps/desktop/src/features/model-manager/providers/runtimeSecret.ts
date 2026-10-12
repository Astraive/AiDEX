// Vault keys for external runtimes. A runtime resolves its `vault` key only through the consumer
// `process:runtime-<id>`, so the key must be stored for exactly that consumer.
import { call, callUntyped } from "@aidex/client/rpc";

/** The vault consumer a runtime's key is released to. */
export function runtimeConsumer(runtimeId: string): string {
  return `process:runtime-${runtimeId}`;
}

/** Whether the vault accepts this runtime's consumer (lowercase letters, digits, `.`, `_`, `-`). */
export function runtimeConsumerValid(runtimeId: string): boolean {
  return /^[a-z0-9._-]{1,64}$/.test(runtimeId);
}

type SecretListing = {
  secrets?: { name?: string; policy?: { allowed_consumers?: string[] } }[];
};

/**
 * Stores `value` as the runtime's key under `secretName`. Consumers the secret already has are kept,
 * so a secret shared with another runtime or provider keeps working for them.
 */
export async function storeRuntimeKey(
  runtimeId: string,
  secretName: string,
  value: string,
): Promise<void> {
  if (!runtimeConsumerValid(runtimeId))
    throw new Error("This runtime's ID has capital letters or other characters the vault refuses. Add it again with a lowercase ID.");
  const listing = (await callUntyped("vault.secret.list")) as SecretListing;
  const existing =
    listing.secrets?.find((secret) => secret.name === secretName)?.policy?.allowed_consumers ?? [];
  const consumer = runtimeConsumer(runtimeId);
  await call("vault.secret.set", {
    name: secretName,
    value,
    consumers: existing.includes(consumer) ? existing : [...existing, consumer],
    kind: "api_key",
  });
}
