import { useEffect, useState, type FormEvent } from "react";
import { Notice } from "@aidex/ui/components";
import {
  Button,
  Card,
  PasswordField,
  Select,
  Stack,
  Text,
  TextArea,
  TextField,
} from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import type { SecretKind, SecretMeta } from "@aidex/client/dto/vault";
import { useRpc } from "../../hooks/useRpc";
import {
  consumerList,
  managedSecret,
  secretPolicyError,
  validConsumer,
} from "./secretPolicy";

export function AddSecretForm({
  editing,
  existingNames,
  disabled,
  onSaved,
  onCommitted,
  onCancel,
}: {
  editing?: SecretMeta;
  existingNames: string[];
  disabled?: boolean;
  onSaved: () => void;
  onCommitted: () => void;
  onCancel: () => void;
}) {
  const [name, setName] = useState(editing?.name ?? "");
  const [value, setValue] = useState("");
  const [kind, setKind] = useState<SecretKind>(editing?.kind ?? "api_key");
  const [scopes, setScopes] = useState(
    editing?.policy.allowed_consumers.join("\n") ?? "",
  );
  const [multiline, setMultiline] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const providers = useRpc("model.providers");
  const servers = useRpc("extensions.mcp.list");
  const runtimes = useRpc("runtime.list");
  useEffect(() => {
    if (disabled) setValue("");
  }, [disabled]);
  const suggestions = [
    ...(providers.data?.providers ?? []).map((provider) => ({
      value: `provider:${provider.descriptor.id}`,
      label: `Provider ${provider.descriptor.id}`,
    })),
    ...(servers.data ?? []).map((server) => ({
      value: `mcp:${server.id}`,
      label: `MCP ${server.name}`,
    })),
    ...(runtimes.data?.runtimes ?? []).map((runtime) => ({
      value: `process:runtime-${runtime.id}`,
      label: `Runtime ${runtime.id}`,
    })),
  ].filter((option) => validConsumer(option.value));
  const save = async (event: FormEvent) => {
    event.preventDefault();
    setError(undefined);
    const consumers = consumerList(scopes),
      trimmed = name.trim();
    const invalid = secretPolicyError(trimmed, consumers);
    if (invalid) {
      setError(invalid);
      return;
    }
    if (managedSecret({ name: trimmed })) {
      setError(
        "This record is managed by AiDEX. Use its account or runtime lifecycle instead.",
      );
      return;
    }
    if (!editing && existingNames.includes(trimmed)) {
      setError(
        "That secret exists. Use Replace value on its row to rotate it deliberately.",
      );
      return;
    }
    if (!value) {
      setError("Enter a new value. Stored values cannot be read back.");
      return;
    }
    setBusy(true);
    try {
      await call("vault.secret.set", { name: trimmed, value, kind, consumers });
      setValue("");
      onSaved();
    } catch (caught) {
      const failure = toAidexError(caught);
      setError(failure.message);
      if ((failure.data as { committed?: boolean } | undefined)?.committed)
        onCommitted();
    } finally {
      setValue("");
      setBusy(false);
    }
  };
  return (
    <Card padding="md">
      <form onSubmit={(event) => void save(event)}>
        <Stack gap={4}>
          <Text>
            {editing
              ? `Replace value and exact scopes · ${editing.name}`
              : "Add a scoped secret"}
          </Text>
          {error && <Notice tone="danger">{error}</Notice>}
          <TextField
            label="Secret reference name"
            required
            disabled={disabled || busy || !!editing}
            value={name}
            maxLength={128}
            autoComplete="off"
            spellCheck={false}
            onChange={(event) => setName(event.target.value)}
            hint="Use this name in runtime authentication or MCP secret references. Stored values cannot be read back."
          />
          <Select
            label="Secret kind"
            disabled={disabled || busy}
            value={kind}
            options={[
              { value: "api_key", label: "API key" },
              { value: "token", label: "Token" },
              { value: "password", label: "Password" },
              { value: "ssh_key", label: "SSH key" },
              { value: "private_key", label: "Private key" },
              { value: "generic", label: "Generic text" },
            ]}
            onChange={(event) => setKind(event.target.value as SecretKind)}
          />
          {multiline ? (
            <TextArea
              label="New multiline value (visible while entered)"
              rows={5}
              value={value}
              autoComplete="off"
              spellCheck={false}
              disabled={disabled || busy}
              onChange={(event) => setValue(event.target.value)}
            />
          ) : (
            <PasswordField
              label="New secret value"
              value={value}
              autoComplete="off"
              disabled={disabled || busy}
              onChange={(event) => setValue(event.target.value)}
            />
          )}
          <Button
            variant="ghost"
            disabled={disabled || busy}
            onClick={() => {
              setValue("");
              setMultiline(!multiline);
            }}
          >
            {multiline
              ? "Use masked single-line input"
              : "Enter a multiline value"}
          </Button>
          <Select
            label="Add a configured consumer"
            value=""
            disabled={disabled || busy}
            options={[
              { value: "", label: "Choose a provider, MCP server, or runtime" },
              ...suggestions,
            ]}
            onChange={(event) => {
              if (event.target.value)
                setScopes(
                  [...consumerList(scopes), event.target.value]
                    .filter((scope, index, all) => all.indexOf(scope) === index)
                    .join("\n"),
                );
            }}
          />
          <TextArea
            label="Exact allowed consumers"
            rows={4}
            monospace
            value={scopes}
            disabled={disabled || busy}
            onChange={(event) => setScopes(event.target.value)}
            hint="One per line: provider:<id>, mcp:<id>, process:runtime-<id>, or git:<host[:port]>. No wildcards. Empty grants deny every consumer."
          />
          <Text tone="muted">
            Secret scopes identify the component receiving a handle, rather than
            a workspace or model prompt. Replacing a value also updates its
            consumer policy.
          </Text>
          <Stack direction="row" gap={2}>
            <Button type="submit" disabled={disabled || busy}>
              {busy
                ? "Saving…"
                : editing
                  ? "Replace value and scopes"
                  : "Store secret"}
            </Button>
            <Button
              variant="ghost"
              disabled={busy}
              onClick={() => {
                setValue("");
                onCancel();
              }}
            >
              Cancel
            </Button>
          </Stack>
        </Stack>
      </form>
    </Card>
  );
}
