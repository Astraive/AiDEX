import { useEffect, useState } from "react";
import {
  DataTable,
  Notice,
  SectionHeader,
  type DataTableColumn,
} from "@aidex/ui/components";
import {
  Button,
  Card,
  PasswordField,
  Select,
  Stack,
  StatusIndicator,
  Text,
} from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import {
  MIN_PASSPHRASE_CHARS,
  type SecretMeta,
  type VaultState,
} from "@aidex/client/dto/vault";
import { useRpc } from "../../hooks/useRpc";
import { useNavigation } from "../../app/providers/navigation";
import { CoreStatusNotice } from "../system/CoreStatusNotice";
import { AddSecretForm } from "./AddSecretForm";
import { managedSecret } from "./secretPolicy";

export function VaultPanel() {
  const { navigate } = useNavigation();
  const vault = useRpc("vault.status");
  const [committedState, setCommittedState] = useState<VaultState>();
  const state = committedState ?? vault.data?.state;
  const secrets = useRpc("vault.secret.list", undefined, {
    enabled: state === "unlocked",
  });
  const [editing, setEditing] = useState<SecretMeta>();
  const [adding, setAdding] = useState(false);
  const [passphrase, setPassphrase] = useState("");
  const [method, setMethod] = useState<"passphrase" | "recovery_key">(
    "passphrase",
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [message, setMessage] = useState<string>();
  useEffect(() => {
    if (vault.data) setCommittedState(undefined);
  }, [vault.data]);
  useEffect(() => {
    const timer = setInterval(vault.refetch, 3000);
    return () => clearInterval(timer);
  }, [vault.refetch]);
  useEffect(() => {
    if (state !== "unlocked") {
      setEditing(undefined);
      setAdding(false);
    }
  }, [state]);
  const refresh = () => {
    vault.refetch();
    secrets.refetch();
    window.dispatchEvent(new Event("aidex:security-changed"));
  };
  const act = async (operation: () => Promise<unknown>) => {
    setBusy(true);
    setError(undefined);
    setMessage(undefined);
    try {
      await operation();
      refresh();
    } catch (caught) {
      setError(toAidexError(caught).message);
    } finally {
      refresh();
      setPassphrase("");
      setBusy(false);
    }
  };
  const columns: DataTableColumn<SecretMeta>[] = [
    { key: "name", header: "Name", rowHeader: true },
    { key: "kind", header: "Kind" },
    {
      key: "consumers",
      header: "Exact consumers",
      render: (secret) =>
        secret.policy.allowed_consumers.join(", ") || "None · no access",
    },
    {
      key: "updated_at",
      header: "Updated",
      render: (secret) => new Date(secret.updated_at).toLocaleString(),
    },
    {
      key: "actions",
      header: "Actions",
      render: (secret) =>
        managedSecret(secret) ? (
          <Text tone="muted">System-managed · use its owning feature</Text>
        ) : (
          <Stack direction="row" gap={2}>
            <Button
              variant="ghost"
              disabled={busy}
              onClick={() => {
                setEditing(secret);
                setAdding(false);
              }}
            >
              Replace value
            </Button>
            <Button
              variant="ghost"
              disabled={busy}
              onClick={() => {
                if (
                  window.confirm(
                    `Delete secret ${secret.name} and revoke its handles? Configurations referencing it will require a new credential.`,
                  )
                )
                  void act(async () => {
                    await call("vault.secret.delete", { name: secret.name });
                    setMessage("Secret deleted and handles revoked.");
                  });
              }}
            >
              Delete
            </Button>
          </Stack>
        ),
    },
  ];
  return (
    <Stack gap={5}>
      <CoreStatusNotice requires="daemon" />
      {(error || vault.error || secrets.error) && (
        <Notice tone="danger">
          {error ?? vault.error?.message ?? secrets.error?.message}
        </Notice>
      )}
      {message && <Notice tone="success">{message}</Notice>}
      <Card padding="md">
        <Stack gap={3}>
          <StatusIndicator
            tone={state === "unlocked" ? "success" : "neutral"}
            label={state ? `Vault ${state}` : "Vault state unavailable"}
          />
          {vault.data?.vault_id && (
            <Text tone="muted">Vault {vault.data.vault_id}</Text>
          )}
          <Text>
            Secret records are encrypted on the drive with XChaCha20-Poly1305;
            the passphrase key uses Argon2id.
          </Text>
          {vault.data?.idle_lock_secs != null && (
            <Text tone="muted">
              Idle lock after {vault.data.idle_lock_secs} seconds.
            </Text>
          )}
          {state === "unlocked" && (
            <Button
              variant="outline"
              disabled={busy}
              onClick={() =>
                void act(async () => {
                  const locked = await call("vault.lock");
                  setCommittedState(locked.state);
                  setMessage(
                    `Vault locked. ${locked.revoked_handles} handles revoked; managed process cancellation was requested.`,
                  );
                })
              }
            >
              Lock vault and stop managed processes
            </Button>
          )}
          {(state === "locked" || state === "missing") && (
            <Stack gap={3}>
              {state === "locked" && (
                <Select
                  label="Unlock method"
                  value={method}
                  options={[
                    { value: "passphrase", label: "Passphrase" },
                    {
                      value: "recovery_key",
                      label: vault.data?.has_recovery_key
                        ? "Recovery key"
                        : "Recovery key · not configured",
                      disabled: vault.data?.has_recovery_key !== true,
                    },
                  ]}
                  onChange={(event) => {
                    setMethod(event.target.value as typeof method);
                    setPassphrase("");
                  }}
                />
              )}
              <PasswordField
                label={
                  state === "missing"
                    ? "New vault passphrase"
                    : method === "recovery_key"
                      ? "Recovery key"
                      : "Vault passphrase"
                }
                value={passphrase}
                autoComplete="off"
                onChange={(event) => setPassphrase(event.target.value)}
                disabled={busy}
              />
              <Button
                disabled={
                  busy ||
                  !passphrase ||
                  (state === "missing" &&
                    [...passphrase].length < MIN_PASSPHRASE_CHARS)
                }
                onClick={() =>
                  void act(async () => {
                    const result =
                      state === "missing"
                        ? await call("vault.init", {
                            passphrase,
                            recovery: false,
                          })
                        : await call(
                            "vault.unlock",
                            method === "passphrase"
                              ? { passphrase }
                              : { recovery_key: passphrase },
                          );
                    setCommittedState(result.state);
                    setMessage("Vault unlocked.");
                  })
                }
              >
                {state === "missing"
                  ? "Create encrypted vault"
                  : "Unlock vault"}
              </Button>
            </Stack>
          )}
          {state === "unavailable" && (
            <Notice tone="danger">
              {vault.data?.message ??
                "Vault data is unavailable. Resolve the reported drive or vault error before changing credentials."}
            </Notice>
          )}
          <Button variant="ghost" disabled={busy} onClick={vault.refetch}>
            Refresh vault status
          </Button>
        </Stack>
      </Card>
      {state === "unlocked" && (
        <>
          <SectionHeader
            title="Stored secret metadata"
            meta={`${secrets.data?.secrets.length ?? vault.data?.secrets ?? "…"} records`}
            actions={
              <Button
                disabled={busy}
                onClick={() => {
                  setAdding(true);
                  setEditing(undefined);
                }}
              >
                Add scoped secret
              </Button>
            }
          />
          <Text tone="muted">
            Only names, kinds, timestamps, and exact consumer policies are
            returned. Secret values and resolving handles have no frontend read
            API.
          </Text>
          <DataTable
            caption="Vault secret metadata"
            columns={columns}
            rows={secrets.data?.secrets ?? []}
            rowKey={(secret) => secret.secret_ref.secret_id}
            emptyText={
              secrets.loading ? "Loading metadata…" : "No stored secrets."
            }
          />
          {(adding || editing) && (
            <AddSecretForm
              key={editing?.name ?? "new"}
              editing={editing}
              existingNames={
                secrets.data?.secrets.map((secret) => secret.name) ?? []
              }
              disabled={busy || state !== "unlocked"}
              onSaved={() => {
                setEditing(undefined);
                setAdding(false);
                setMessage("Secret stored with its exact consumer policy.");
                refresh();
              }}
              onCommitted={refresh}
              onCancel={() => {
                setEditing(undefined);
                setAdding(false);
              }}
            />
          )}
        </>
      )}
      <Stack direction="row" gap={2}>
        <Button
          variant="ghost"
          onClick={() => navigate({ page: "models", tab: "providers" })}
        >
          Manage provider accounts
        </Button>
        <Button variant="ghost" onClick={() => navigate({ page: "agents" })}>
          Manage runtime accounts
        </Button>
        <Button
          variant="ghost"
          onClick={() => navigate({ page: "extensions", tab: "mcp" })}
        >
          Configure MCP secret references
        </Button>
      </Stack>
    </Stack>
  );
}
