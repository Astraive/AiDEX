import { DataTable, InlineCode, List, ListRow, MediaItem, SectionHeader, type DataTableColumn } from "@aidex/ui/components";
import { CloudIcon, GitBranchIcon, LockIcon, LockOpenIcon, RefreshIcon, TrashIcon } from "@aidex/ui/icons";
import { Button, Card, IconBadge, IconButton, Stack, StatusIndicator, Text } from "@aidex/ui/primitives";
import { useToast } from "../../app/providers/toast";
import { AddSecretForm } from "./AddSecretForm";
import { sampleAccounts, sampleSecrets, type VaultSecret } from "./sample";

const accountIcons = { openai: CloudIcon, github: GitBranchIcon } as const;

/** Vault and secrets: vault status, stored secrets (masked), the add form, and connected accounts. */
export function VaultPanel() {
  const notify = useToast();
  const preview = (action: string) => notify(`Preview: ${action} will be available when AiDEX Core is connected.`);

  const secretColumns: readonly DataTableColumn<VaultSecret>[] = [
    { key: "name", header: "Name", rowHeader: true },
    { key: "type", header: "Type" },
    { key: "scope", header: "Scope" },
    { key: "masked", header: "Value (masked)", render: (row) => <InlineCode>{row.masked}</InlineCode> },
    { key: "lastUsed", header: "Last used" },
    {
      key: "actions",
      header: "Actions",
      align: "end",
      render: (row) => (
        <Stack direction="row" gap={1} justify="end">
          <IconButton size="sm" label={`Rotate ${row.name}`} icon={<RefreshIcon />} onClick={() => preview(`rotating ${row.name}`)} />
          <IconButton size="sm" label={`Delete ${row.name}`} icon={<TrashIcon />} onClick={() => preview(`deleting ${row.name}`)} />
        </Stack>
      ),
    },
  ];

  return (
    <Stack gap={10}>
      <Card padding="md">
        <MediaItem
          media={<IconBadge tone="accent" icon={<LockOpenIcon />} />}
          title="Encrypted vault · XChaCha20-Poly1305"
          description="Unlocked for this session. Your passphrase derives the key with Argon2id; records stay encrypted on the drive."
          trailing={
            <Stack direction="row" gap={3} align="center">
              <StatusIndicator tone="success" label="Unlocked" />
              <Button variant="outline" size="md" icon={<LockIcon />} onClick={() => preview("locking the vault")}>
                Lock vault
              </Button>
            </Stack>
          }
        />
      </Card>

      <section>
        <Stack gap={4}>
          <SectionHeader title="Secrets" meta={`${sampleSecrets.length} stored`} />
          <DataTable caption="Vault secrets" hideCaption columns={secretColumns} rows={sampleSecrets} rowKey={(row) => row.id} />
          <Text tone="secondary">
            Agents receive secret handles, not values. The broker passes the credential to the tool or network call, so it stays out of
            prompts, logs, and model context.
          </Text>
        </Stack>
      </section>

      <section>
        <Stack gap={4}>
          <SectionHeader title="Add a secret" />
          <AddSecretForm />
        </Stack>
      </section>

      <section>
        <Stack gap={4}>
          <SectionHeader title="Connected accounts" meta={`${sampleAccounts.length} accounts`} />
          <List>
            {sampleAccounts.map((account) => {
              const Icon = accountIcons[account.id as keyof typeof accountIcons] ?? CloudIcon;
              return (
                <ListRow
                  key={account.id}
                  media={<IconBadge size="md" icon={<Icon />} />}
                  title={account.name}
                  subtitle={account.detail}
                  trailing={
                    <Stack direction="row" gap={4} align="center">
                      <StatusIndicator tone={account.status.tone} label={account.status.label} />
                      <Button
                        variant="ghost"
                        size="md"
                        aria-label={`Manage ${account.name}`}
                        onClick={() => preview(`managing ${account.name}`)}
                      >
                        Manage
                      </Button>
                    </Stack>
                  }
                />
              );
            })}
          </List>
        </Stack>
      </section>
    </Stack>
  );
}
