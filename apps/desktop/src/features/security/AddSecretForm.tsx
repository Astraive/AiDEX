import { useRef, useState, type FormEvent } from "react";
import { Notice } from "@aidex/ui/components";
import { KeyIcon, PlusIcon } from "@aidex/ui/icons";
import {
  Button,
  CheckboxGroup,
  Grid,
  PasswordField,
  Select,
  Stack,
  TextArea,
  TextField,
  type CheckboxOption,
} from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import type { SecretKind, SecretMeta } from "@aidex/client/dto/vault";
import { useRpc } from "../../hooks/useRpc";
import {
  consumerList,
  MANAGED_MESSAGE,
  managedSecret,
  secretPolicyError,
  secretWriteError,
  validConsumer,
  validSecretName,
} from "./secretPolicy";

export const secretKindLabels: Record<SecretKind, string> = {
  api_key: "API key",
  token: "Token",
  password: "Password",
  ssh_key: "SSH key",
  private_key: "Private key",
  generic: "Other",
};

const kindOptions = (Object.keys(secretKindLabels) as SecretKind[]).map((value) => ({ value, label: secretKindLabels[value] }));

/** Kinds whose values span lines, so they get a text area instead of a masked field. */
const MULTILINE_KINDS: ReadonlySet<SecretKind> = new Set(["ssh_key", "private_key"]);

/** The web search tools' consumer (crates/aidexd/src/methods/web.rs). */
const WEB_CONSUMER = "process:web";

type Props = {
  /** The secret being replaced: its name is fixed and its consumers start selected. */
  editing?: SecretMeta;
  /** Names already stored, so adding one refuses to overwrite it silently. */
  existingNames: readonly string[];
  /** The daemon's `managed` names. */
  managed: readonly string[];
  onSaved: (meta: SecretMeta) => void;
  onCancel?: () => void;
  /** Told when a save starts and settles, so a dialog around the form can stay open while it saves. */
  onBusyChange?: (busy: boolean) => void;
};

type Errors = { name?: string; value?: string; consumers?: string; form?: string };

/** Consumers AiDEX knows about on this drive: providers, MCP servers, runtimes, and web search. */
function useConsumerOptions(): CheckboxOption[] {
  const providers = useRpc("model.providers");
  const servers = useRpc("mcp.list");
  const runtimes = useRpc("runtime.list");
  return [
    ...(providers.data?.providers ?? []).map((provider) => ({
      value: `provider:${provider.descriptor.id}`,
      title: `Provider · ${provider.display_name ?? provider.descriptor.id}`,
    })),
    ...(servers.data?.servers ?? []).map((server) => ({ value: `mcp:${server.id}`, title: `MCP server · ${server.name}` })),
    ...(runtimes.data?.runtimes ?? []).map((runtime) => ({
      value: `process:runtime-${runtime.id}`,
      title: `Runtime · ${runtime.id}`,
    })),
    { value: WEB_CONSUMER, title: "Web search" },
  ]
    .filter((option) => validConsumer(option.value))
    .map((option) => ({ ...option, description: option.value }));
}

/**
 * Stores a new secret or replaces one, with its kind and the exact consumers that may receive it.
 * The value goes straight to `vault.secret.set` and is cleared from state after every submit; it is
 * never read back, logged, or shown again.
 */
export function AddSecretForm({ editing, existingNames, managed, onSaved, onCancel, onBusyChange }: Props) {
  const [name, setName] = useState(editing?.name ?? "");
  const [kind, setKind] = useState<SecretKind>(editing?.kind ?? "api_key");
  const [value, setValue] = useState("");
  const [selected, setSelected] = useState<string[]>(editing?.policy.allowed_consumers ?? []);
  const [other, setOther] = useState("");
  const [errors, setErrors] = useState<Errors>({});
  const [busy, setBusy] = useState(false);
  const nameRef = useRef<HTMLInputElement>(null);
  const known = useConsumerOptions();

  // Consumers the secret already has stay listed (and checked) even when nothing configured matches them.
  const options: CheckboxOption[] = [
    ...known,
    ...(editing?.policy.allowed_consumers ?? [])
      .filter((consumer) => !known.some((option) => option.value === consumer))
      .map((consumer) => ({ value: consumer, title: consumer, description: "Not configured on this drive" })),
  ];
  const multiline = MULTILINE_KINDS.has(kind);

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const trimmed = name.trim();
    const consumers = [...new Set([...selected, ...consumerList(other)])];
    const next: Errors = {};
    if (!trimmed) next.name = "Enter a name, e.g. OPENAI_API_KEY.";
    else if (!validSecretName(trimmed)) next.name = secretPolicyError(trimmed, []);
    else if (managedSecret({ name: trimmed }, managed)) next.name = MANAGED_MESSAGE;
    else if (!editing && existingNames.includes(trimmed)) next.name = "A secret with this name exists. Use Replace on its row.";
    else next.consumers = secretPolicyError(trimmed, consumers);
    if (!value) next.value = "Enter the value. Stored values cannot be read back.";
    setErrors(next);
    if (next.name) {
      nameRef.current?.focus();
      return;
    }
    if (next.value || next.consumers) return;

    setBusy(true);
    onBusyChange?.(true);
    try {
      const meta = await call("vault.secret.set", { name: trimmed, value, consumers, kind });
      if (!editing) {
        setName("");
        setSelected([]);
        setOther("");
      }
      onSaved(meta);
    } catch (caught) {
      setErrors({ form: secretWriteError(toAidexError(caught)) });
    } finally {
      setValue("");
      setBusy(false);
      onBusyChange?.(false);
    }
  };

  return (
    <form onSubmit={(event) => void submit(event)} noValidate aria-label={editing ? `Replace ${editing.name}` : "Add secret"}>
      <Stack gap={5}>
        {errors.form ? (
          <Notice tone="danger" role="alert">
            {errors.form}
          </Notice>
        ) : null}
        <Grid columns={2} gap={4}>
          <TextField
            ref={nameRef}
            label="Name"
            placeholder="OPENAI_API_KEY"
            leadingIcon={<KeyIcon />}
            autoComplete="off"
            spellCheck={false}
            maxLength={128}
            value={name}
            disabled={busy || editing !== undefined}
            error={errors.name}
            onChange={(event) => {
              setName(event.target.value);
              setErrors((current) => ({ ...current, name: undefined, form: undefined }));
            }}
          />
          <Select
            label="Kind"
            options={kindOptions}
            value={kind}
            disabled={busy}
            onChange={(event) => {
              setKind(event.target.value as SecretKind);
              setValue("");
            }}
          />
        </Grid>
        {multiline ? (
          <TextArea
            label={editing ? "New value" : "Value"}
            rows={5}
            monospace
            autoComplete="off"
            spellCheck={false}
            value={value}
            disabled={busy}
            error={errors.value}
            hint="Shown while you type; cleared as soon as it is sent."
            onChange={(event) => {
              setValue(event.target.value);
              setErrors((current) => ({ ...current, value: undefined }));
            }}
          />
        ) : (
          <PasswordField
            label={editing ? "New value" : "Value"}
            placeholder="Paste the secret"
            revealLabel="Show secret value"
            autoComplete="off"
            value={value}
            disabled={busy}
            error={errors.value}
            onChange={(event) => {
              setValue(event.target.value);
              setErrors((current) => ({ ...current, value: undefined }));
            }}
          />
        )}
        {options.length ? (
          <CheckboxGroup
            label="Who can use it"
            options={options.map((option) => ({ ...option, disabled: busy }))}
            value={selected}
            onValueChange={(next) => {
              setSelected(next);
              setErrors((current) => ({ ...current, consumers: undefined }));
            }}
          />
        ) : null}
        <TextArea
          label="Other consumers"
          rows={2}
          monospace
          spellCheck={false}
          placeholder="git:github.com"
          value={other}
          disabled={busy}
          error={errors.consumers}
          hint="One per line: provider:<id>, mcp:<id>, process:<id>, or git:<host[:port]>. Exact names only. With no consumers, nothing can use the secret."
          onChange={(event) => {
            setOther(event.target.value);
            setErrors((current) => ({ ...current, consumers: undefined }));
          }}
        />
        <Stack direction="row" gap={2} justify="end">
          {onCancel ? (
            <Button
              variant="ghost"
              size="md"
              disabled={busy}
              onClick={() => {
                setValue("");
                onCancel();
              }}
            >
              Cancel
            </Button>
          ) : null}
          <Button type="submit" variant="primary" size="md" icon={editing ? undefined : <PlusIcon />} disabled={busy}>
            {busy ? "Saving…" : editing ? "Replace value" : "Add secret"}
          </Button>
        </Stack>
      </Stack>
    </form>
  );
}
