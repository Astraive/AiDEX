import { useState } from "react";
import { Dialog, Notice, SettingsSection } from "@aidex/ui/components";
import { ExternalLinkIcon, KeyIcon } from "@aidex/ui/icons";
import { Button, PasswordField, Stack, Text } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { useNavigation } from "../../../app/providers/navigation";
import { useRpc } from "../../../hooks/useRpc";
import { useProviderCatalog } from "../catalog/ProviderCatalog";
import { isOAuthAccount, isRuntimeAccount } from "../catalog/providerAccounts";
import type { AccountItem } from "./providerItems";
import { accountAuthAdapter, authFlowText, useAuthFlow } from "./useAuthFlow";
import { useProviderActions } from "./useProviderActions";

/** How the account signs in: ChatGPT, an API key in the vault, or nothing for local servers. */
export function SetupSection({ item }: { item: AccountItem }) {
  const { navigate } = useNavigation();
  if (isRuntimeAccount(item.account))
    return (
      <SettingsSection title="Setup" icon={<KeyIcon />}>
        <Text>AiDEX runs this model from the drive. Start, stop, or switch it in Local models.</Text>
        <Stack direction="row" gap={2}>
          <Button variant="outline" onClick={() => navigate({ page: "models", tab: "local" })}>
            Local models
          </Button>
        </Stack>
      </SettingsSection>
    );
  if (isOAuthAccount(item.account)) return <ChatGptSetup item={item} />;
  return <KeySetup item={item} />;
}

function ChatGptSetup({ item }: { item: AccountItem }) {
  const catalog = useProviderCatalog();
  const id = item.account.descriptor.id;
  const status = useRpc("auth.status", { provider_id: id });
  const actions = useProviderActions();
  const [confirming, setConfirming] = useState(false);
  const flow = useAuthFlow(accountAuthAdapter, () => {
    status.refetch();
    void catalog.refresh(id).catch(() => undefined);
  });
  const connected = status.data?.state === "connected";
  const label = status.data?.account_label;
  const flowText = authFlowText(flow.state);
  return (
    <SettingsSection
      title="Setup"
      icon={<KeyIcon />}
      description="Signed in through your ChatGPT plan. AiDEX keeps the tokens in the drive vault."
    >
      <Text>
        {status.loading && !status.data
          ? "Checking sign-in…"
          : connected
            ? label
              ? `Signed in as ${label}.`
              : "Signed in."
            : "Not signed in. Reconnect to use this account."}
      </Text>
      {status.error && <Notice tone="warning">{status.error.message}</Notice>}
      {flowText && (
        <Notice tone={flow.state.phase === "failed" ? "danger" : "info"} role="status">
          {flowText}
        </Notice>
      )}
      {actions.error && (
        <Notice tone="danger" role="alert">
          {actions.error}
        </Notice>
      )}
      <Stack direction="row" gap={2} wrap>
        {flow.state.phase === "waiting" || flow.state.phase === "starting" ? (
          <Button variant="outline" onClick={() => void flow.cancel()}>
            Cancel sign-in
          </Button>
        ) : (
          <Button
            variant="primary"
            onClick={() =>
              void flow.start(() => call("auth.start", { provider_id: id, method: "chatgpt" }))
            }
          >
            Reconnect
          </Button>
        )}
        <Button variant="ghost" disabled={!connected || actions.busy !== null} onClick={() => setConfirming(true)}>
          Sign out
        </Button>
      </Stack>
      <Dialog open={confirming} title={`Sign out of ${item.title}?`} onClose={() => setConfirming(false)}>
        <Text>
          AiDEX deletes this account's tokens from the vault. Conversations that use it stop working until you
          sign in again.
        </Text>
        {actions.error && (
          <Notice tone="danger" role="alert">
            {actions.error}
          </Notice>
        )}
        <Stack direction="row" gap={2} justify="end">
          <Button variant="ghost" onClick={() => setConfirming(false)}>
            Cancel
          </Button>
          <Button
            variant="primary"
            disabled={actions.busy !== null}
            onClick={() =>
              void actions
                .run("sign-out", async () => {
                  await call("auth.disconnect", { provider_id: id });
                  return true;
                })
                .then((signedOut) => {
                  if (!signedOut) return;
                  setConfirming(false);
                  status.refetch();
                  catalog.reload();
                })
            }
          >
            Sign out
          </Button>
        </Stack>
      </Dialog>
    </SettingsSection>
  );
}

function KeySetup({ item }: { item: AccountItem }) {
  const actions = useProviderActions();
  const [key, setKey] = useState("");
  const { account, preset } = item;
  const hasKey = account.descriptor.auth.type === "vault_secret";
  const keyOptional = !hasKey || !!preset?.auth_methods.includes("none");
  const keyless = !hasKey && !!preset && !preset.auth_methods.includes("api_key");
  const bedrock = account.descriptor.kind === "bedrock";
  // Only a server on this computer can be started by the user; remote endpoints get no such hint.
  const runsHere = account.descriptor.locality === "local";
  const save = async () => {
    const value = key;
    // The key leaves React state as soon as it is sent.
    setKey("");
    await actions.replaceKey(account, value);
  };
  return (
    <SettingsSection
      title="Setup"
      icon={<KeyIcon />}
      description={
        hasKey
          ? "The key is encrypted in the drive vault and never shown again."
          : keyOptional && runsHere
            ? `Make sure ${item.title} is running on this computer.`
            : undefined
      }
    >
      {keyless ? (
        <Text>
          This provider needs no key. If it stops answering, start it again and refresh its models.
        </Text>
      ) : (
        <form
          onSubmit={(event) => {
            event.preventDefault();
            if (key) void save();
          }}
        >
          <Stack gap={3}>
            <PasswordField
              label={
                bedrock
                  ? "Replace AWS credentials JSON"
                  : hasKey
                    ? "Replace API key"
                    : "API key (optional)"
              }
              revealLabel="Show key"
              size="md"
              autoComplete="off"
              placeholder={preset?.key_placeholder ?? undefined}
              hint={
                bedrock
                  ? "access_key_id, secret_access_key, region, optional session_token"
                  : "Saving checks the key by listing this provider's models."
              }
              value={key}
              onChange={(event) => setKey(event.target.value)}
            />
            <Stack direction="row" gap={2} wrap>
              <Button type="submit" variant="primary" disabled={!key || actions.busy !== null}>
                {actions.busy === "key" ? "Saving…" : "Save key"}
              </Button>
              {preset?.api_key_url && (
                <Button
                  variant="ghost"
                  icon={<ExternalLinkIcon />}
                  onClick={() => void actions.openPresetLink(preset.id, "api_key")}
                >
                  Get an API key
                </Button>
              )}
            </Stack>
          </Stack>
        </form>
      )}
      {actions.error && (
        <Notice tone="danger" role="alert">
          {actions.error}
        </Notice>
      )}
    </SettingsSection>
  );
}
