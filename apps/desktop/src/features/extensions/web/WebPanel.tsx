import { useId, useState, type FormEvent } from "react";
import { InlineCode, Notice, ProviderTile, SettingRow, SettingsSection } from "@aidex/ui/components";
import { BrandIcon, ExternalLinkIcon, GlobeIcon, KeyIcon } from "@aidex/ui/icons";
import { Badge, Button, Grid, PasswordField, Select, Stack, StatusIndicator, Switch, Text, TextField } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { ErrorKind } from "@aidex/client/errors";
import type { SearchProvider, WebProviderInfo, WebSettingsPatch, WebSettingsView } from "@aidex/client/dto/web";
import { useToast } from "../../../app/providers/toast";
import { describeError, useAction, useExtensionsQuery, useOpenLink, useVaultSecrets } from "../data";
import { listSecrets, storeSecret } from "../vaultSecrets";
import { WebTestSearch } from "./WebTestSearch";

const MAX_RESULTS = 10;
const maxResultOptions = Array.from({ length: MAX_RESULTS }, (_, index) => ({
  value: String(index + 1),
  label: String(index + 1),
}));

/** The vault secret AiDEX names a provider's key when the settings name none yet. */
const defaultSecretName = (provider: SearchProvider) => `web-${provider}`;

/** What is still missing, in words that point at the field to fill. */
function setupProblem(view: WebSettingsView, provider: WebProviderInfo | undefined): string | undefined {
  if (view.configured) return undefined;
  if (provider?.requires_endpoint && !view.endpoint) return `Enter your ${provider.display_name} endpoint below.`;
  if (provider?.requires_key && !view.secret_name) return `Add a ${provider.display_name} API key below.`;
  return view.problem;
}

function providerBadges(provider: WebProviderInfo): string[] {
  if (provider.requires_endpoint) return ["Self-hosted"];
  if (provider.requires_key) return ["API key"];
  return ["No key"];
}

/** Web tab: the master switch, provider choice and setup, fetch and result limits, and a test search. */
export function WebPanel() {
  const settings = useExtensionsQuery("web.settings.get");
  const catalog = useExtensionsQuery("web.providers");
  const action = useAction();
  const enabledId = useId();
  const fetchId = useId();
  const maxId = useId();
  const view = settings.data;
  const providers = catalog.data?.providers ?? [];
  const provider = providers.find((entry) => entry.id === view?.provider);

  const patch = (label: string, changes: WebSettingsPatch) =>
    action.run(label, () => call("web.settings.set", changes));

  /** Switching provider reuses a key AiDEX already stored for it, so switching back needs no re-entry. */
  const chooseProvider = async (next: WebProviderInfo) => {
    if (!view || next.id === view.provider) return;
    let secretName: string | undefined;
    if (next.accepts_key) {
      const name = defaultSecretName(next.id);
      const stored = await listSecrets().catch(() => undefined);
      if (stored?.get(name)?.consumers.includes(view.credential_consumer)) secretName = name;
    }
    await patch("provider", { provider: next.id, ...(secretName ? { secret_name: secretName } : {}) });
  };

  if (!view) {
    const loadError = settings.error ?? catalog.error;
    return loadError ? (
      <Notice tone="danger" role="alert">
        {describeError(loadError)}
      </Notice>
    ) : (
      <Text tone="muted">Loading web search settings…</Text>
    );
  }

  const problem = setupProblem(view, provider);

  return (
    <Stack gap={6}>
      <SettingsSection
        titleLevel={2}
        title="Web search"
        icon={<GlobeIcon />}
        description="Lets agents search the web and read pages. Results reach the model as untrusted data with their URLs, and it is asked to cite them."
      >
        <SettingRow
          title="Enable web search"
          description={
            view.configured
              ? "Agents get web search in Agent and Plan mode. In Chat, turn it on per conversation with the globe in the composer."
              : "Finish setting up a provider below before agents can search."
          }
          controlId={enabledId}
          control={
            <Switch
              id={enabledId}
              aria-describedby={`${enabledId}-description`}
              checked={view.enabled}
              disabled={action.busy !== null}
              onCheckedChange={(enabled) => void patch("enabled", { enabled })}
            />
          }
        />
        <Stack direction="row" gap={3} align="center" wrap role="status">
          <StatusIndicator
            tone={view.configured ? (view.enabled ? "success" : "neutral") : "warning"}
            label={view.configured ? (view.enabled ? "On" : "Ready, turned off") : "Needs setup"}
          />
          {problem ? <Text as="span" tone="muted">{problem}</Text> : null}
        </Stack>
        <SettingRow
          title="Read web pages"
          description="Agents can fetch a page's text. In Supervised mode each fetch asks first; long pages are saved as artifacts."
          controlId={fetchId}
          control={
            <Switch
              id={fetchId}
              aria-describedby={`${fetchId}-description`}
              checked={view.fetch_enabled}
              disabled={action.busy !== null}
              onCheckedChange={(fetchEnabled) => void patch("fetch", { fetch_enabled: fetchEnabled })}
            />
          }
        />
        <SettingRow
          title="Results per search"
          description="How many results one search returns to the agent."
          controlId={maxId}
          control={
            <Select
              id={maxId}
              label="Results per search"
              labelVisibility="hidden"
              aria-describedby={`${maxId}-description`}
              options={maxResultOptions}
              value={String(view.max_results)}
              disabled={action.busy !== null}
              onChange={(event) => void patch("max", { max_results: Number(event.target.value) })}
            />
          }
        />
        {action.error ? (
          <Notice tone="danger" role="alert">
            {action.error}
          </Notice>
        ) : null}
      </SettingsSection>
      <SettingsSection titleLevel={2} title="Search provider" description="Searches go only to the provider you choose.">
        {catalog.error ? (
          <Notice tone="danger">{describeError(catalog.error)}</Notice>
        ) : (
          <Grid columns={3} gap={3}>
            {providers.map((entry) => (
              <ProviderTile
                key={entry.id}
                brand={entry.icon}
                name={entry.display_name}
                description={entry.description}
                selected={entry.id === view.provider}
                disabled={action.busy !== null}
                badges={
                  <>
                    {providerBadges(entry).map((label) => (
                      <Badge key={label} tone="neutral" size="sm">
                        {label}
                      </Badge>
                    ))}
                  </>
                }
                onSelect={() => void chooseProvider(entry)}
              />
            ))}
          </Grid>
        )}
      </SettingsSection>
      {provider ? <ProviderSetup key={provider.id} provider={provider} view={view} /> : null}
      <WebTestSearch providerName={provider?.display_name ?? view.provider} disabled={!view.configured} />
    </Stack>
  );
}

/** The chosen provider's key (stored in the vault for `process:web`) and SearXNG endpoint. */
function ProviderSetup({ provider, view }: { provider: WebProviderInfo; view: WebSettingsView }) {
  const action = useAction();
  const notify = useToast();
  const openLink = useOpenLink();
  const secrets = useVaultSecrets();
  const [key, setKey] = useState("");
  const [endpoint, setEndpoint] = useState(view.endpoint ?? "");
  const secretName = view.secret_name ?? defaultSecretName(provider.id);
  const stored = view.secret_name ? secrets.data?.get(view.secret_name) : undefined;
  const released = stored?.consumers.includes(view.credential_consumer) ?? false;
  const locked = secrets.error?.kind === ErrorKind.vaultLocked;

  const saveKey = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!key) return;
    // The key leaves React state as soon as it is sent.
    const value = key;
    setKey("");
    const saved = await action.run("key", async () => {
      await storeSecret(secretName, value, view.credential_consumer, "api_key");
      if (view.secret_name !== secretName) await call("web.settings.set", { secret_name: secretName });
      return true;
    });
    if (saved) notify(`${provider.display_name} key saved in the vault.`, "success");
  };

  const saveEndpoint = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const saved = await action.run("endpoint", () =>
      call("web.settings.set", { endpoint: endpoint.trim() ? endpoint.trim() : null }),
    );
    if (saved) notify("Endpoint saved.", "success");
  };

  return (
    <SettingsSection
      titleLevel={2}
      title={provider.display_name}
      icon={<BrandIcon brand={provider.icon} monogramLabel={provider.display_name} size={20} aria-hidden />}
      description={provider.description}
      actions={
        <Button variant="ghost" icon={<ExternalLinkIcon />} onClick={() => openLink(provider.docs_url)}>
          Documentation
        </Button>
      }
    >
      {provider.requires_endpoint ? (
        <form onSubmit={(event) => void saveEndpoint(event)} aria-label={`${provider.display_name} endpoint`}>
          <Stack gap={3}>
            <TextField
              label="Endpoint"
              size="md"
              type="url"
              placeholder="https://search.example.com"
              autoComplete="off"
              spellCheck={false}
              value={endpoint}
              hint="Your SearXNG base URL with the JSON output format enabled. Plain http works only for this computer."
              onChange={(event) => setEndpoint(event.target.value)}
            />
            <Stack direction="row" gap={2}>
              <Button
                type="submit"
                variant="outline"
                disabled={action.busy !== null || endpoint.trim() === (view.endpoint ?? "")}
              >
                {action.busy === "endpoint" ? "Saving…" : "Save endpoint"}
              </Button>
            </Stack>
          </Stack>
        </form>
      ) : null}
      {provider.accepts_key ? (
        <form onSubmit={(event) => void saveKey(event)} aria-label={`${provider.display_name} API key`}>
          <Stack gap={3}>
            {view.secret_name ? (
              <Stack direction="row" gap={3} align="center" wrap>
                <Text as="span">
                  Vault secret <InlineCode>{view.secret_name}</InlineCode>
                </Text>
                {secrets.data ? (
                  <StatusIndicator
                    tone={released ? "success" : "warning"}
                    label={!stored ? "Missing from the vault" : released ? "Stored" : "Not released to web search"}
                  />
                ) : locked ? (
                  <StatusIndicator tone="neutral" label="Vault locked" />
                ) : null}
              </Stack>
            ) : null}
            <PasswordField
              label={view.secret_name ? "Replace API key" : provider.requires_key ? "API key" : "API key (optional)"}
              revealLabel="Show key"
              size="md"
              autoComplete="off"
              value={key}
              hint={`Encrypted in the drive vault as ${secretName} and released only to web search (${view.credential_consumer}).`}
              onChange={(event) => setKey(event.target.value)}
            />
            <Stack direction="row" gap={2} wrap>
              <Button type="submit" variant="primary" icon={<KeyIcon />} disabled={!key || action.busy !== null}>
                {action.busy === "key" ? "Saving…" : "Save key"}
              </Button>
              {provider.key_url ? (
                <Button variant="ghost" icon={<ExternalLinkIcon />} onClick={() => openLink(provider.key_url ?? "")}>
                  Get an API key
                </Button>
              ) : null}
            </Stack>
          </Stack>
        </form>
      ) : null}
      {!provider.accepts_key && !provider.requires_endpoint ? (
        <Text tone="muted">No key or setup needed.</Text>
      ) : null}
      {action.error ? (
        <Notice tone="danger" role="alert">
          {action.error}
        </Notice>
      ) : null}
    </SettingsSection>
  );
}
