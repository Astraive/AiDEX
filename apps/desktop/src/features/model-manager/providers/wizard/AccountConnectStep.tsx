import { useId, useRef, useState, type FormEvent } from "react";
import { Notice, SettingRow } from "@aidex/ui/components";
import { ArrowLeftIcon, ChevronDownIcon, ChevronRightIcon, ExternalLinkIcon } from "@aidex/ui/icons";
import {
  Button,
  Grid,
  PasswordField,
  Stack,
  Switch,
  Text,
  TextField,
} from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import type { ProviderAddParams, ProviderPreset } from "@aidex/client/dto/models";
import { accountAuthAdapter, authFlowText, useAuthFlow } from "../useAuthFlow";
import {
  ACCOUNT_ID,
  declaresModels,
  endpointHint,
  endpointPlaceholder,
  keyMode,
  requiresEndpoint,
  slugify,
  supportsChatGpt,
  uniqueId,
} from "./wizardLogic";

/** An account this wizard saved, and the vault secret it stored for it, if any. */
export type SavedAccount = { id: string; secretName: string | null };

type Props = {
  preset: ProviderPreset;
  /** Ids already used by accounts. */
  takenIds: ReadonlySet<string>;
  /** Set when this wizard already saved the account and the user came back to fix it. */
  existing: SavedAccount | null;
  onSaved: (saved: SavedAccount) => void;
};

/** Step 2 for a provider preset: ChatGPT sign-in or an API key, plus advanced endpoint settings. */
export function AccountConnectStep({ preset, takenIds, existing, onSaved }: Props) {
  const chatGpt = supportsChatGpt(preset);
  // Coming back to fix a saved account reopens the way it was connected.
  const [method, setMethod] = useState<"chatgpt" | "key">(
    chatGpt && !(existing && existing.secretName) ? "chatgpt" : "key",
  );
  return (
    <Stack gap={4}>
      {preset.description && <Text tone="muted">{preset.description}</Text>}
      {method === "chatgpt" ? (
        <ChatGptConnect
          takenIds={takenIds}
          existing={existing && !existing.secretName ? existing : null}
          onSaved={onSaved}
          onUseKey={() => setMethod("key")}
        />
      ) : (
        <>
          {chatGpt && (
            <Stack direction="row">
              <Button variant="ghost" icon={<ArrowLeftIcon />} onClick={() => setMethod("chatgpt")}>
                Use ChatGPT sign-in instead
              </Button>
            </Stack>
          )}
          <KeyForm preset={preset} takenIds={takenIds} existing={existing} onSaved={onSaved} />
        </>
      )}
    </Stack>
  );
}

function freshChatGptId(): string {
  return `openai-chatgpt-${crypto.randomUUID().replace(/-/g, "").slice(0, 8)}`;
}

function ChatGptConnect({
  takenIds,
  existing,
  onSaved,
  onUseKey,
}: Pick<Props, "takenIds" | "existing" | "onSaved"> & { onUseKey: () => void }) {
  const flow = useAuthFlow(accountAuthAdapter, (providerId) => onSaved({ id: providerId, secretName: null }));
  const pending = flow.state.phase === "waiting" || flow.state.phase === "starting";
  const begin = (providerId: string) =>
    void flow.start(() => call("auth.start", { provider_id: providerId, method: "chatgpt" }));
  const text = authFlowText(flow.state);
  return (
    <Stack gap={3}>
      <Text>Sign in with your ChatGPT account to use an eligible plan in AiDEX. Tokens stay in the drive vault.</Text>
      {text && (
        <Notice tone={flow.state.phase === "failed" ? "danger" : "info"} role="status">
          {text}
        </Notice>
      )}
      <Stack direction="row" gap={2} wrap>
        {pending ? (
          <Button variant="outline" onClick={() => void flow.cancel()}>
            Cancel sign-in
          </Button>
        ) : (
          <>
            <Button
              variant="primary"
              onClick={() =>
                begin(
                  // Coming back to fix the account this wizard signed in reconnects it in place.
                  existing ? existing.id : takenIds.has("openai") ? freshChatGptId() : "openai",
                )
              }
            >
              {existing ? "Sign in again" : "Continue with ChatGPT"}
            </Button>
            <Button variant="outline" onClick={onUseKey}>
              Use an API key
            </Button>
            <Button variant="ghost" onClick={() => begin(freshChatGptId())}>
              Connect another account or workspace
            </Button>
          </>
        )}
      </Stack>
      <Text tone="muted">
        If ChatGPT reports a workspace access error, use a new connection and choose a workspace you belong to.
        Existing connections are preserved.
      </Text>
    </Stack>
  );
}

type Errors = { id?: string; endpoint?: string; key?: string; model?: string; context?: string };

function KeyForm({ preset, takenIds, existing, onSaved }: Props) {
  const existingId = existing?.id ?? null;
  const discoverId = useId();
  const mode = keyMode(preset);
  const declared = declaresModels(preset);
  const bedrock = preset.kind === "bedrock";
  const [name, setName] = useState(preset.display_name);
  const [key, setKey] = useState("");
  const [advanced, setAdvanced] = useState(requiresEndpoint(preset) || declared);
  const [accountId, setAccountId] = useState(() => existingId ?? uniqueId(slugify(preset.id), takenIds));
  const [endpoint, setEndpoint] = useState(preset.base_url ?? "");
  const [discover, setDiscover] = useState(preset.discover_models && !declared);
  const [modelId, setModelId] = useState("");
  const [context, setContext] = useState("128000");
  const [errors, setErrors] = useState<Errors>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // The vault secret this wizard already stored, so a retry need not ask for the key again.
  const storedSecret = useRef<string | null>(existing?.secretName ?? null);

  const id = existingId ?? accountId.trim();
  const secretName = `provider-${id}`;
  const haveSecret = storedSecret.current === secretName;
  // An account without discovery must declare a model; the daemon refuses one that lists nothing.
  const needsModel = declared || !discover;

  const validate = (): Errors => ({
    id:
      !ACCOUNT_ID.test(id)
        ? "Use 1-64 lowercase letters, digits, dashes, or underscores, starting with a letter or digit."
        : !existingId && takenIds.has(id)
          ? "An account with this ID already exists."
          : undefined,
    endpoint:
      requiresEndpoint(preset) && !endpoint.trim()
        ? "Enter the endpoint URL."
        : endpoint.trim() && !/^https?:\/\//i.test(endpoint.trim())
          ? "Use an https:// URL, or http:// for a server on this computer."
          : undefined,
    key:
      mode === "required" && !key && !haveSecret
        ? bedrock
          ? "Paste the AWS credentials JSON."
          : "Paste the API key."
        : undefined,
    model: needsModel && !modelId.trim() ? "Enter a deployment or model ID." : undefined,
    context:
      needsModel && (!Number.isSafeInteger(Number(context)) || Number(context) < 1)
        ? "Enter the model's context limit in tokens."
        : undefined,
  });

  const save = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const found = validate();
    setErrors(found);
    if (Object.values(found).some(Boolean)) {
      if (found.id || found.endpoint || found.model || found.context) setAdvanced(true);
      return;
    }
    setBusy(true);
    setError(null);
    const value = key;
    // The key leaves React state as soon as it is sent.
    setKey("");
    try {
      if (value) {
        await call("vault.secret.set", {
          name: secretName,
          value,
          consumers: [`provider:${id}`],
          kind: "api_key",
        });
        storedSecret.current = secretName;
      }
      const trimmedName = name.trim();
      const params: ProviderAddParams = {
        id,
        kind: preset.kind,
        base_url: endpoint.trim() || preset.base_url || "",
        secret_name: value || storedSecret.current === secretName ? secretName : undefined,
        discover_models: declared ? false : discover,
        models: needsModel
          ? [
              {
                id: modelId.trim(),
                context_window: Number(context),
                tool_calls: preset.tool_calls ? "native" : "none",
                json_schema_output: preset.json_schema_output,
              },
            ]
          : [],
        display_name: trimmedName && trimmedName !== preset.display_name ? trimmedName : null,
        preset_id: preset.id,
        enabled: true,
      };
      await call(existingId ? "provider.update" : "provider.add", params);
      onSaved({ id, secretName: storedSecret.current === secretName ? secretName : null });
    } catch (caught) {
      setError(toAidexError(caught).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <form onSubmit={(event) => void save(event)} noValidate aria-label={`Connect ${preset.display_name}`}>
      <Stack gap={4}>
        <TextField
          label="Account name"
          size="md"
          value={name}
          maxLength={64}
          hint="Shown in the model picker and the providers list."
          onChange={(event) => setName(event.target.value)}
        />
        {mode === "none" ? (
          preset.category === "local" ? (
            <Notice>Make sure {preset.display_name} is running on this computer.</Notice>
          ) : null
        ) : (
          <Stack gap={2}>
            <PasswordField
              label={
                bedrock
                  ? "AWS credentials JSON"
                  : mode === "optional"
                    ? "API key (optional)"
                    : haveSecret
                      ? "API key (leave empty to keep the saved key)"
                      : "API key"
              }
              revealLabel="Show key"
              size="md"
              autoComplete="off"
              placeholder={preset.key_placeholder ?? undefined}
              hint={
                bedrock
                  ? "access_key_id, secret_access_key, region, optional session_token"
                  : "Encrypted in the drive vault and never shown again."
              }
              error={errors.key}
              value={key}
              onChange={(event) => setKey(event.target.value)}
            />
            {mode === "optional" && preset.category === "local" && (
              <Text tone="muted">Make sure {preset.display_name} is running on this computer.</Text>
            )}
            {preset.api_key_url && (
              <Stack direction="row">
                <Button
                  variant="ghost"
                  icon={<ExternalLinkIcon />}
                  onClick={() =>
                    void call("provider.open_link", { preset_id: preset.id, link: "api_key" }).catch((caught) =>
                      setError(toAidexError(caught).message),
                    )
                  }
                >
                  Get an API key
                </Button>
              </Stack>
            )}
          </Stack>
        )}
        <Stack direction="row">
          <Button
            variant="ghost"
            aria-expanded={advanced}
            icon={advanced ? <ChevronDownIcon /> : <ChevronRightIcon />}
            onClick={() => setAdvanced((open) => !open)}
          >
            Advanced
          </Button>
        </Stack>
        {advanced && (
          <Stack gap={3}>
            <TextField
              label="Account ID"
              size="md"
              spellCheck={false}
              autoCapitalize="off"
              disabled={!!existingId}
              value={existingId ?? accountId}
              error={errors.id}
              hint="Used in model keys and logs; it cannot change later."
              onChange={(event) => setAccountId(event.target.value)}
            />
            <TextField
              label={requiresEndpoint(preset) ? "Endpoint" : "Endpoint (optional)"}
              size="md"
              spellCheck={false}
              inputMode="url"
              value={endpoint}
              error={errors.endpoint}
              placeholder={endpointPlaceholder(preset)}
              hint={endpointHint(preset)}
              onChange={(event) => setEndpoint(event.target.value)}
            />
            {!declared && (
              <SettingRow
                title="Discover models automatically"
                description="List the models this provider serves. When off, only the model you declare here is offered."
                controlId={discoverId}
                control={
                  <Switch
                    id={discoverId}
                    aria-describedby={`${discoverId}-description`}
                    checked={discover}
                    onCheckedChange={setDiscover}
                  />
                }
              />
            )}
            {needsModel && (
              <Grid columns={2} gap={3}>
                <TextField
                  label={preset.kind === "azure-openai" ? "Deployment name" : "Model ID"}
                  size="md"
                  spellCheck={false}
                  value={modelId}
                  error={errors.model}
                  onChange={(event) => setModelId(event.target.value)}
                />
                <TextField
                  label="Context window"
                  size="md"
                  inputMode="numeric"
                  value={context}
                  error={errors.context}
                  onChange={(event) => setContext(event.target.value)}
                />
              </Grid>
            )}
          </Stack>
        )}
        {error && (
          <Notice tone="danger" role="alert">
            {error}
          </Notice>
        )}
        <Stack direction="row" justify="end">
          <Button type="submit" variant="primary" disabled={busy}>
            {busy ? "Connecting…" : existingId ? "Save and check again" : "Connect"}
          </Button>
        </Stack>
      </Stack>
    </form>
  );
}
