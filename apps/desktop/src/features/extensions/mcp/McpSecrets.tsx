import { useState, type FormEvent } from "react";
import { InlineCode, KeyValueList, Notice, SectionHeader } from "@aidex/ui/components";
import { KeyIcon } from "@aidex/ui/icons";
import { Button, PasswordField, Select, Stack, StatusIndicator, Text } from "@aidex/ui/primitives";
import { ErrorKind } from "@aidex/client/errors";
import type { McpServer } from "@aidex/client/dto/mcp";
import { useToast } from "../../../app/providers/toast";
import { describeError, useAction, useVaultSecrets } from "../data";
import { storeSecret, type StoredSecret } from "../vaultSecrets";
import { mcpConsumer } from "./mcpLabels";

type Mapping = { use: string; secret: string };

function mappingsOf(server: McpServer): Mapping[] {
  const mapped: Mapping[] = [
    ...(server.config.env_from_secrets ?? []).map((entry) => ({ use: `${entry.env} (environment)`, secret: entry.secret_name })),
    ...(server.config.headers_from_secrets ?? []).map((entry) => ({ use: `${entry.header} (header)`, secret: entry.secret_name })),
  ];
  const named = new Set(mapped.map((entry) => entry.secret));
  return [...mapped, ...server.secret_names.filter((name) => !named.has(name)).map((name) => ({ use: name, secret: name }))];
}

function secretStatus(stored: StoredSecret | undefined, consumer: string) {
  if (!stored) return { label: "Not in the vault", tone: "warning" as const };
  if (!stored.consumers.includes(consumer)) return { label: "Not released to this server", tone: "warning" as const };
  return { label: "Stored", tone: "success" as const };
}

/**
 * The vault secrets a server's configuration names, whether each is stored and released to the
 * server (`mcp:<id>`), and a field to store a value. The value leaves React state as it is sent.
 */
export function McpSecrets({ server }: { server: McpServer }) {
  const secrets = useVaultSecrets();
  const action = useAction();
  const notify = useToast();
  const consumer = mcpConsumer(server.id);
  const mappings = mappingsOf(server);
  const names = [...new Set(mappings.map((entry) => entry.secret))];
  const [target, setTarget] = useState(names[0] ?? "");
  const [value, setValue] = useState("");
  if (mappings.length === 0) return null;
  const chosen = names.includes(target) ? target : (names[0] ?? "");
  const locked = secrets.error?.kind === ErrorKind.vaultLocked;

  const save = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!value || !chosen) return;
    const secretValue = value;
    setValue("");
    const stored = await action.run("secret", async () => {
      await storeSecret(chosen, secretValue, consumer, "token");
      return true;
    });
    if (stored)
      notify(
        server.status === "running" ? `${chosen} stored. Restart the server to use it.` : `${chosen} stored in the vault.`,
        "success",
      );
  };

  return (
    <Stack gap={3}>
      <SectionHeader level={3} title="Secrets" meta={consumer} />
      <KeyValueList
        items={mappings.map((entry) => {
          const status = secrets.data ? secretStatus(secrets.data.get(entry.secret), consumer) : null;
          return {
            label: entry.use,
            value: (
              <Stack direction="row" gap={3} align="center" wrap>
                <InlineCode>{entry.secret}</InlineCode>
                {status ? <StatusIndicator tone={status.tone} label={status.label} /> : null}
              </Stack>
            ),
          };
        })}
      />
      {locked ? (
        <Notice tone="warning">Unlock the drive vault to check and store this server&apos;s secrets.</Notice>
      ) : secrets.error ? (
        <Notice tone="warning">{describeError(secrets.error)}</Notice>
      ) : (
        <form onSubmit={(event) => void save(event)} aria-label={`Store a secret for ${server.name}`}>
          <Stack gap={3}>
            {names.length > 1 ? (
              <Select
                label="Secret"
                options={names.map((name) => ({ value: name, label: name }))}
                value={chosen}
                onChange={(event) => setTarget(event.target.value)}
              />
            ) : null}
            <PasswordField
              label={`Value for ${chosen}`}
              revealLabel="Show value"
              size="md"
              autoComplete="off"
              value={value}
              hint="Encrypted in the drive vault and released only to this server. Storing replaces the secret's current value."
              onChange={(event) => setValue(event.target.value)}
            />
            <Stack direction="row" gap={2}>
              <Button type="submit" variant="outline" icon={<KeyIcon />} disabled={!value || action.busy !== null}>
                {action.busy ? "Storing…" : "Store in vault"}
              </Button>
            </Stack>
          </Stack>
        </form>
      )}
      {action.error ? (
        <Notice tone="danger" role="alert">
          {action.error}
        </Notice>
      ) : null}
      <Text tone="muted">A secret shared with other servers or providers keeps working for them.</Text>
    </Stack>
  );
}
