import { useEffect, useState } from "react";
import {
  DataTable,
  Dialog,
  formatDateTime,
  InlineCode,
  KeyValueList,
  MediaItem,
  Notice,
  SectionHeader,
  type DataTableColumn,
} from "@aidex/ui/components";
import { BotIcon, CloudIcon, LockIcon, LockOpenIcon, PencilIcon, PlugIcon, TrashIcon } from "@aidex/ui/icons";
import { Button, Card, IconBadge, IconButton, Stack, StatusIndicator, Text } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import type { SecretMeta, VaultState } from "@aidex/client/dto/vault";
import { useNavigation } from "../../app/providers/navigation";
import { useToast } from "../../app/providers/toast";
import { useRpc } from "../../hooks/useRpc";
import { CoreStatusNotice } from "../system/CoreStatusNotice";
import { AddSecretForm, secretKindLabels } from "./AddSecretForm";
import { PassphraseForm } from "./PassphraseForm";
import { managedSecret, secretWriteError } from "./secretPolicy";
import type { SecurityStatus } from "./useSecurityStatus";

type Props = { status: SecurityStatus };

const stateIndicator: Record<VaultState, { label: string; tone: "success" | "warning" | "danger" | "neutral" }> = {
  unlocked: { label: "Unlocked", tone: "success" },
  locked: { label: "Locked", tone: "neutral" },
  missing: { label: "Not set up", tone: "warning" },
  unavailable: { label: "Unavailable", tone: "danger" },
};

const idleLockLabel = (seconds: number | undefined): string =>
  !seconds ? "Never locks by itself" : seconds < 120 ? `After ${seconds} idle seconds` : `After ${Math.round(seconds / 60)} idle minutes`;

const formatTime = (time: string | undefined): string => {
  const date = time ? new Date(time) : null;
  return date && !Number.isNaN(date.getTime()) ? formatDateTime(date) : "Unknown";
};

/** The launch screens own creating and unlocking the vault; reloading the window returns to them. */
const openUnlockScreen = () => window.location.reload();

/**
 * Vault and secrets: live vault status with Lock, value-free secret metadata (`vault.secret.list`),
 * adding and replacing secrets with exact consumers, deleting after confirmation, and changing the
 * passphrase. Records AiDEX manages are read-only here. Creating and unlocking the vault stay on the
 * launch screens.
 */
export function VaultPanel({ status }: Props) {
  const { navigate } = useNavigation();
  const notify = useToast();
  const vault = status.vault.data;
  const state = vault?.state;
  const unlocked = state === "unlocked";
  // Read when the vault unlocks and after every change made here, not on each Security poll: every
  // read goes through the vault and would count as use, holding off its idle auto-lock.
  const secrets = useRpc("vault.secret.list", undefined, { enabled: unlocked });
  const [replacing, setReplacing] = useState<SecretMeta>();
  const [deleting, setDeleting] = useState<SecretMeta>();
  const [busy, setBusy] = useState<"lock" | "delete" | "replace">();
  // Lock failures show on the page; delete failures only inside the confirmation they belong to.
  const [lockError, setLockError] = useState<string>();
  const [deleteError, setDeleteError] = useState<string>();

  // A form or confirmation must not outlive the unlocked vault it belongs to.
  useEffect(() => {
    if (unlocked) return;
    setReplacing(undefined);
    setDeleting(undefined);
    setDeleteError(undefined);
  }, [unlocked]);

  const list = unlocked ? secrets.data : undefined;
  const managed = list?.managed ?? [];
  const rows = list?.secrets ?? [];
  const changed = () => {
    secrets.refetch();
    status.refresh();
  };

  const lock = async () => {
    setBusy("lock");
    setLockError(undefined);
    try {
      await call("vault.lock");
      // Reloading drops decrypted preferences and cached data, like Lock drive in the account menu.
      window.location.reload();
    } catch (caught) {
      setLockError(`The vault could not be locked: ${toAidexError(caught).message}`);
      setBusy(undefined);
    }
  };

  const remove = async (secret: SecretMeta) => {
    setBusy("delete");
    setDeleteError(undefined);
    try {
      await call("vault.secret.delete", { name: secret.name });
      setDeleting(undefined);
      notify(`Deleted ${secret.name}. Handles issued for it are revoked.`);
      changed();
    } catch (caught) {
      setDeleteError(secretWriteError(toAidexError(caught)));
    } finally {
      setBusy(undefined);
    }
  };

  const closeDelete = () => {
    if (busy === "delete") return;
    setDeleting(undefined);
    setDeleteError(undefined);
  };

  // Like a delete, a replace keeps its dialog open while it saves, so a failure is shown in it.
  const closeReplace = () => {
    if (busy === "replace") return;
    setReplacing(undefined);
  };

  const columns: readonly DataTableColumn<SecretMeta>[] = [
    { key: "name", header: "Name", rowHeader: true, render: (row) => <InlineCode>{row.name}</InlineCode> },
    { key: "kind", header: "Kind", render: (row) => secretKindLabels[row.kind] ?? row.kind },
    {
      key: "consumers",
      header: "Who can use it",
      render: (row) => (row.policy.allowed_consumers.length ? row.policy.allowed_consumers.join(", ") : "No one"),
    },
    { key: "updated", header: "Updated", render: (row) => formatTime(row.updated_at) },
    {
      key: "actions",
      header: "Actions",
      align: "end",
      render: (row) =>
        managedSecret(row, managed) ? (
          <Text tone="muted">Managed by AiDEX</Text>
        ) : (
          <Stack direction="row" gap={1} justify="end">
            <IconButton size="sm" label={`Replace ${row.name}`} icon={<PencilIcon />} onClick={() => setReplacing(row)} />
            <IconButton size="sm" label={`Delete ${row.name}`} icon={<TrashIcon />} onClick={() => setDeleting(row)} />
          </Stack>
        ),
    },
  ];

  return (
    <Stack gap={10}>
      <CoreStatusNotice requires="daemon" />
      {status.vault.error ? <Notice tone="danger">Cannot read the vault status: {status.vault.error.message}</Notice> : null}
      {lockError ? (
        <Notice tone="danger" role="alert">
          {lockError}
        </Notice>
      ) : null}

      <Card padding="md">
        <Stack gap={4}>
          <MediaItem
            media={<IconBadge tone="accent" icon={unlocked ? <LockOpenIcon /> : <LockIcon />} />}
            title="Encrypted vault · XChaCha20-Poly1305"
            description="Your passphrase derives the key with Argon2id; records stay encrypted on the drive."
            trailing={
              <Stack direction="row" gap={3} align="center">
                {state ? <StatusIndicator tone={stateIndicator[state].tone} label={stateIndicator[state].label} /> : null}
                {unlocked ? (
                  <Button variant="outline" size="md" icon={<LockIcon />} disabled={busy !== undefined} onClick={() => void lock()}>
                    {busy === "lock" ? "Locking…" : "Lock vault"}
                  </Button>
                ) : null}
              </Stack>
            }
          />
          {unlocked && vault ? (
            <KeyValueList
              items={[
                { label: "Unlocked with", value: vault.unlocked_by === "recovery_key" ? "Recovery key" : "Passphrase" },
                { label: "Unlocked at", value: formatTime(vault.unlocked_at) },
                { label: "Locks", value: idleLockLabel(vault.idle_lock_secs) },
                { label: "Recovery key", value: vault.has_recovery_key ? "Set up" : "Not set up" },
              ]}
            />
          ) : null}
          {unlocked ? (
            <Text tone="secondary">
              Locking cancels active agent runs and runtime sign-ins, stops agent runtimes and MCP servers, revokes
              every handle issued from the vault, and returns to the unlock screen.
            </Text>
          ) : null}
          {state === "locked" || state === "missing" ? (
            <Notice tone="warning">
              <Stack gap={3} align="start">
                <Text as="span">
                  {state === "locked"
                    ? "The vault is locked, so secrets and settings cannot be read. Unlock it from the start screen."
                    : "This drive has no vault yet. Create one from the start screen."}
                </Text>
                <Button variant="outline" size="md" icon={<LockOpenIcon />} onClick={openUnlockScreen}>
                  {state === "locked" ? "Go to unlock" : "Set up the vault"}
                </Button>
              </Stack>
            </Notice>
          ) : null}
          {state === "unavailable" ? (
            <Notice tone="danger">{vault?.message ?? "The vault cannot be read. Check the drive, then try again."}</Notice>
          ) : null}
        </Stack>
      </Card>

      {unlocked ? (
        <>
          <section>
            <Stack gap={4}>
              <SectionHeader title="Secrets" meta={list ? `${rows.length} stored` : undefined} />
              {secrets.error ? <Notice tone="danger">Cannot list secrets: {secrets.error.message}</Notice> : null}
              <DataTable
                caption="Vault secrets"
                hideCaption
                columns={columns}
                rows={rows}
                rowKey={(row) => row.secret_ref.secret_id}
                emptyText={secrets.loading ? "Reading the vault…" : "No secrets stored yet."}
              />
              <Text tone="secondary">
                Only names, kinds, times, and consumers are shown; values never leave the vault. Agents receive handles, not
                values, and only the exact consumers listed can resolve them.
              </Text>
            </Stack>
          </section>

          <section>
            <Stack gap={4}>
              <SectionHeader title="Add a secret" />
              <Card padding="md">
                <AddSecretForm
                  existingNames={rows.map((row) => row.name)}
                  managed={managed}
                  onSaved={(meta) => {
                    notify(`Stored ${meta.name}.`);
                    changed();
                  }}
                />
              </Card>
            </Stack>
          </section>

          <section>
            <Stack gap={4}>
              <SectionHeader title="Change passphrase" />
              <Card padding="md">
                <PassphraseForm unlockedByRecoveryKey={vault?.unlocked_by === "recovery_key"} onChanged={status.refresh} />
              </Card>
            </Stack>
          </section>
        </>
      ) : null}

      <section>
        <Stack gap={4}>
          <SectionHeader title="Where secrets are used" />
          <Stack direction="row" gap={2} wrap>
            <Button variant="ghost" size="md" icon={<CloudIcon />} onClick={() => navigate({ page: "models", tab: "providers" })}>
              Model providers
            </Button>
            <Button variant="ghost" size="md" icon={<BotIcon />} onClick={() => navigate({ page: "agents" })}>
              Agent runtimes
            </Button>
            <Button variant="ghost" size="md" icon={<PlugIcon />} onClick={() => navigate({ page: "extensions", tab: "mcp" })}>
              MCP servers
            </Button>
          </Stack>
        </Stack>
      </section>

      <Dialog
        open={replacing !== undefined}
        size="lg"
        title={replacing ? `Replace ${replacing.name}` : "Replace secret"}
        description="Enter the new value and choose who can use it. The old value is overwritten and cannot be recovered."
        onClose={closeReplace}
      >
        {replacing && unlocked ? (
          <AddSecretForm
            key={replacing.secret_ref.secret_id}
            editing={replacing}
            existingNames={rows.map((row) => row.name)}
            managed={managed}
            onBusyChange={(saving) => setBusy(saving ? "replace" : undefined)}
            onCancel={closeReplace}
            onSaved={(meta) => {
              setReplacing(undefined);
              notify(`Replaced ${meta.name}.`);
              changed();
            }}
          />
        ) : null}
      </Dialog>

      <Dialog
        open={deleting !== undefined}
        title={deleting ? `Delete ${deleting.name}?` : "Delete secret?"}
        description="The value is erased and every handle issued for it is revoked. Anything configured to use it will need a new secret."
        onClose={closeDelete}
      >
        <Stack gap={4}>
          {deleteError ? (
            <Notice tone="danger" role="alert">
              {deleteError}
            </Notice>
          ) : null}
          <Stack direction="row" gap={2} justify="end">
            <Button variant="ghost" disabled={busy === "delete"} onClick={closeDelete}>
              Cancel
            </Button>
            <Button
              variant="primary"
              icon={<TrashIcon />}
              disabled={busy !== undefined || !deleting}
              onClick={() => deleting && void remove(deleting)}
            >
              {busy === "delete" ? "Deleting…" : "Delete secret"}
            </Button>
          </Stack>
        </Stack>
      </Dialog>
    </Stack>
  );
}
