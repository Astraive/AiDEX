import type { SecretMeta } from "@aidex/client/dto/vault";
const identifier = /^[a-z0-9][a-z0-9._-]{0,127}$/;
export function validConsumer(text: string): boolean {
  const separator = text.indexOf(":");
  if (separator < 1) return false;
  const kind = text.slice(0, separator),
    target = text.slice(separator + 1);
  if (["provider", "mcp", "process"].includes(kind))
    return identifier.test(target);
  if (kind !== "git" || !target || target.length > 128) return false;
  const [host, port, extra] = target.split(":");
  if (
    extra !== undefined ||
    (port !== undefined &&
      (!/^\d{1,5}$/.test(port) || Number(port) < 1 || Number(port) > 65535))
  )
    return false;
  return host
    .split(".")
    .every((label) => /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(label));
}
export const consumerList = (text: string): string[] => [
  ...new Set(
    text
      .split(/[\n,]/)
      .map((value) => value.trim())
      .filter(Boolean),
  ),
];
export function secretPolicyError(
  name: string,
  consumers: string[],
): string | undefined {
  if (!/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/.test(name))
    return "Use a 1–128 character name beginning with a letter or digit, with letters, digits, dot, underscore, or dash.";
  if (consumers.length > 32)
    return "A secret may have at most 32 exact consumers.";
  if (consumers.some((consumer) => !validConsumer(consumer)))
    return "Use exact provider:<id>, mcp:<id>, process:<id>, or git:<lowercase-host[:port]> scopes. Wildcards, agents, prompts, logs, and shell environments are not valid consumers.";
}
/** Internal records are controlled by their domain lifecycle, never rotated as user keys. */
export function managedSecret(meta: Pick<SecretMeta, "name">): boolean {
  return (
    [
      "aidex-preferences",
      "native-record-encryption-key",
      "external-runtime-encryption-key",
    ].includes(meta.name) || meta.name.startsWith("oauth.")
  );
}
