import { useEffect, useState } from "react";
import { InlineCode, KeyValueList, List, ListRow, Notice, SettingsSection } from "@aidex/ui/components";
import { KeyIcon, LayersIcon, RefreshIcon } from "@aidex/ui/icons";
import { Button, PasswordField, Stack, Text } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { useProviderCatalog } from "../catalog/ProviderCatalog";
import { supportsModelDiscovery } from "../catalog/runtimes";
import { EditorHeader } from "./EditorHeader";
import type { RuntimeItem } from "./providerItems";
import { RuntimeConnectionSection } from "./RuntimeConnectionSection";
import { runtimeConsumerValid } from "./runtimeSecret";
import { authFlowText, runtimeAuthAdapter, useAuthFlow } from "./useAuthFlow";
import { useProviderActions } from "./useProviderActions";

/** Editor for an external agent runtime (Codex, Claude Code, ...). */
export function RuntimeEditor({ item }: { item: RuntimeItem }) {
  const actions = useProviderActions();
  const badges = ["Agent runtime", ...(item.runtime.enabled ? [] : ["Disabled"])];
  return (
    <Stack gap={4}>
      <EditorHeader
        brand={item.brand}
        title={item.title}
        badges={badges}
        onRemove={() => actions.removeRuntime(item.runtime)}
        busy={actions.busy !== null}
        error={actions.error}
      />
      <RuntimeSetupSection item={item} />
      <RuntimeConnectionSection item={item} />
      <RuntimeModelsSection item={item} />
    </Stack>
  );
}

function RuntimeSetupSection({ item }: { item: RuntimeItem }) {
  const catalog = useProviderCatalog();
  const { runtime } = item;
  const flow = useAuthFlow(runtimeAuthAdapter(runtime.id), () => void catalog.checkRuntime(runtime.id));
  const auth = runtime.auth;
  const flowText = authFlowText(flow.state);
  return (
    <SettingsSection title="Setup" icon={<KeyIcon />}>
      {auth.kind === "codex_oauth" ? (
        <>
          <Text>
            Codex shares the ChatGPT sign-in of the <InlineCode>{auth.provider_id}</InlineCode> account.
          </Text>
          {flowText && (
            <Notice tone={flow.state.phase === "failed" ? "danger" : "info"} role="status">
              {flowText}
            </Notice>
          )}
          <Stack direction="row" gap={2}>
            {flow.state.phase === "waiting" || flow.state.phase === "starting" ? (
              <Button variant="outline" onClick={() => void flow.cancel()}>
                Cancel sign-in
              </Button>
            ) : (
              <Button
                variant="primary"
                onClick={() => void flow.start(() => call("runtime.auth.start", { id: runtime.id }))}
              >
                Sign in with ChatGPT
              </Button>
            )}
          </Stack>
        </>
      ) : auth.kind === "vault" ? (
        <RuntimeKeySetup item={item} secretName={auth.secret_name} envName={auth.env_name} />
      ) : (
        <Text>This runtime uses its own sign-in on this computer; AiDEX passes it no credentials.</Text>
      )}
    </SettingsSection>
  );
}

function RuntimeKeySetup({
  item,
  secretName,
  envName,
}: {
  item: RuntimeItem;
  secretName: string;
  envName: string;
}) {
  const actions = useProviderActions();
  const [key, setKey] = useState("");
  const { runtime } = item;
  const usable = runtimeConsumerValid(runtime.id);
  const save = async () => {
    const value = key;
    // The key leaves React state as soon as it is sent.
    setKey("");
    await actions.replaceRuntimeKey(runtime, value);
  };
  return (
    <>
      <Text>
        AiDEX passes the key to the runtime as an environment variable. It is encrypted in the drive vault and
        released only to this runtime.
      </Text>
      <KeyValueList
        items={[
          { label: "Vault secret", value: <InlineCode>{secretName}</InlineCode> },
          { label: "Environment variable", value: <InlineCode>{envName}</InlineCode> },
        ]}
      />
      {usable ? (
        <form
          onSubmit={(event) => {
            event.preventDefault();
            if (key) void save();
          }}
        >
          <Stack gap={3}>
            <PasswordField
              label="Replace API key"
              revealLabel="Show key"
              size="md"
              autoComplete="off"
              hint="Saving checks the runtime again."
              value={key}
              onChange={(event) => setKey(event.target.value)}
            />
            <Stack direction="row" gap={2}>
              <Button type="submit" variant="primary" disabled={!key || actions.busy !== null}>
                {actions.busy === "key" ? "Saving…" : "Save key"}
              </Button>
            </Stack>
          </Stack>
        </form>
      ) : (
        <Notice tone="warning">
          The vault cannot release a key to a runtime whose ID has capital letters or other unsupported
          characters. Remove this runtime and add it again with a lowercase ID.
        </Notice>
      )}
      {actions.error && (
        <Notice tone="danger" role="alert">
          {actions.error}
        </Notice>
      )}
    </>
  );
}

function RuntimeModelsSection({ item }: { item: RuntimeItem }) {
  const catalog = useProviderCatalog();
  const { runtime } = item;
  const discovers = supportsModelDiscovery(runtime.driver);
  const state = catalog.runtimeModels[runtime.id];
  const load = catalog.loadRuntimeModels;
  useEffect(() => {
    if (discovers && runtime.enabled) load(runtime.id);
  }, [discovers, runtime.enabled, runtime.id, load]);
  return (
    <SettingsSection
      title="Models"
      icon={<LayersIcon />}
      actions={
        discovers ? (
          <Button
            variant="outline"
            icon={<RefreshIcon />}
            disabled={state?.state === "loading" || !runtime.enabled}
            onClick={() => load(runtime.id, true)}
          >
            {state?.state === "loading" ? "Refreshing…" : "Refresh models"}
          </Button>
        ) : undefined
      }
    >
      {!discovers ? (
        <Text tone="muted">
          Model discovery is unavailable for this runtime. Conversations use the runtime's default model unless you
          pick a specific one in the model picker.
        </Text>
      ) : !runtime.enabled ? (
        <Text tone="muted">Enable this runtime to list its models.</Text>
      ) : state?.state === "error" ? (
        <Notice tone="danger">{state.message}</Notice>
      ) : state?.state === "ready" ? (
        state.models.length ? (
          <List>
            {state.models.map((model) => (
              <ListRow key={model.id} title={model.name || model.id} subtitle={<InlineCode>{model.id}</InlineCode>} />
            ))}
          </List>
        ) : (
          <Text tone="muted">The runtime reported no models. Conversations use its default model.</Text>
        )
      ) : (
        <Text tone="muted">Listing models…</Text>
      )}
    </SettingsSection>
  );
}
