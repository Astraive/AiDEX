import { useState, type FormEvent } from "react";
import { Notice } from "@aidex/ui/components";
import {
  Button,
  Card,
  Grid,
  PasswordField,
  Select,
  Stack,
  Switch,
  Text,
  TextArea,
  TextField,
} from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import type { McpServerConfig } from "@aidex/client/dto/extensions";
import { ScopeFields, splitGrants } from "./ScopeFields";
import { useExtensionAction } from "./useExtensionAction";

export function McpConfigForm({
  initial,
  onCancel,
  onSaved,
}: {
  initial?: McpServerConfig;
  onCancel: () => void;
  onSaved: (id: string) => void;
}) {
  const [config, setConfig] = useState<McpServerConfig>(() =>
    initial
      ? {
          id: initial.id,
          name: initial.name,
          enabled: initial.enabled,
          trusted: initial.trusted,
          workspace_ids: initial.workspace_ids,
          profiles: initial.profiles,
          transport: initial.transport,
          secret_refs: initial.secret_refs,
          timeout_ms: initial.timeout_ms,
          max_output_bytes: initial.max_output_bytes,
        }
      : {
          id: "",
          name: "",
          enabled: true,
          trusted: false,
          workspace_ids: [],
          profiles: [],
          transport: { kind: "stdio", command: "", args: [], cwd: null },
          secret_refs: {},
          timeout_ms: 30000,
          max_output_bytes: 262144,
        },
  );
  const [args, setArgs] = useState(
    JSON.stringify(
      config.transport.kind === "stdio" ? config.transport.args : [],
      null,
      2,
    ),
  );
  const [workspaceIds, setWorkspaceIds] = useState(
    config.workspace_ids.join(", "),
  );
  const [profiles, setProfiles] = useState(config.profiles.join(", "));
  const [secretRefs, setSecretRefs] = useState(
    JSON.stringify(config.secret_refs, null, 2),
  );
  const [secretName, setSecretName] = useState("");
  const [credential, setCredential] = useState("");
  const action = useExtensionAction(() => {});
  const save = (event: FormEvent) => {
    event.preventDefault();
    void action.act(async () => {
      const refs: unknown = JSON.parse(secretRefs);
      if (
        !refs ||
        typeof refs !== "object" ||
        Array.isArray(refs) ||
        Object.values(refs).some((value) => typeof value !== "string")
      )
        throw new Error(
          "Secret references must be a JSON map of header/environment names to vault secret names.",
        );
      let transport = config.transport;
      if (transport.kind === "stdio") {
        const parsed: unknown = JSON.parse(args);
        if (
          !Array.isArray(parsed) ||
          parsed.some((value) => typeof value !== "string")
        )
          throw new Error("Arguments must be a JSON array of literal strings.");
        transport = { ...transport, args: parsed as string[] };
      }
      if (credential) {
        if (!secretName.trim())
          throw new Error(
            "Choose a vault secret name before storing a credential.",
          );
        await call("vault.secret.set", {
          name: secretName.trim(),
          value: credential,
          consumers: [`mcp:${config.id}`],
          kind: "api_key",
        });
        setCredential("");
      }
      const saved = await call("extensions.mcp.upsert", {
        config: {
          ...config,
          transport,
          secret_refs: refs as Record<string, string>,
          workspace_ids: splitGrants(workspaceIds),
          profiles: splitGrants(profiles),
        },
      });
      onSaved(saved.id);
    });
  };
  return (
    <Card padding="md">
      <form onSubmit={save}>
        <Stack gap={4}>
          <Text>{initial ? "Edit MCP server" : "Add MCP server"}</Text>
          {action.error && <Notice tone="danger">{action.error}</Notice>}
          <Grid columns={2} gap={3}>
            <TextField
              label="Server ID"
              required
              disabled={!!initial}
              value={config.id}
              onChange={(event) =>
                setConfig({ ...config, id: event.target.value })
              }
            />
            <TextField
              label="Name"
              required
              value={config.name}
              onChange={(event) =>
                setConfig({ ...config, name: event.target.value })
              }
            />
          </Grid>
          <Select
            label="Transport"
            value={config.transport.kind}
            options={[
              { value: "stdio", label: "Local stdio executable" },
              { value: "https", label: "HTTPS Streamable HTTP" },
            ]}
            onChange={(event) =>
              setConfig({
                ...config,
                transport:
                  event.target.value === "stdio"
                    ? { kind: "stdio", command: "", args: [], cwd: null }
                    : { kind: "https", url: "" },
              })
            }
          />
          {config.transport.kind === "stdio" ? (
            <>
              <TextField
                label="Literal executable path"
                required
                value={config.transport.command}
                onChange={(event) =>
                  setConfig({
                    ...config,
                    transport: {
                      ...(config.transport as Extract<
                        McpServerConfig["transport"],
                        { kind: "stdio" }
                      >),
                      command: event.target.value,
                    },
                  })
                }
              />
              <TextArea
                label="Literal arguments (JSON array)"
                monospace
                rows={3}
                value={args}
                onChange={(event) => setArgs(event.target.value)}
              />
              <TextField
                label="Working directory (optional)"
                value={config.transport.cwd ?? ""}
                onChange={(event) =>
                  setConfig({
                    ...config,
                    transport: {
                      ...(config.transport as Extract<
                        McpServerConfig["transport"],
                        { kind: "stdio" }
                      >),
                      cwd: event.target.value || null,
                    },
                  })
                }
              />
            </>
          ) : (
            <TextField
              label="HTTPS MCP endpoint"
              type="url"
              required
              value={config.transport.url}
              onChange={(event) =>
                setConfig({
                  ...config,
                  transport: { kind: "https", url: event.target.value },
                })
              }
            />
          )}
          <ScopeFields
            workspaceIds={workspaceIds}
            profiles={profiles}
            onWorkspaces={setWorkspaceIds}
            onProfiles={setProfiles}
          />
          <Stack direction="row" gap={2} align="center">
            <Switch
              label="Trust this MCP server"
              checked={config.trusted}
              onCheckedChange={(trusted) => setConfig({ ...config, trusted })}
            />
            <Text>I trust this server for the granted scopes.</Text>
          </Stack>
          <Stack direction="row" gap={2} align="center">
            <Switch
              label="MCP server enabled"
              checked={config.enabled}
              onCheckedChange={(enabled) => setConfig({ ...config, enabled })}
            />
            <Text>Enabled</Text>
          </Stack>
          <Grid columns={2} gap={3}>
            <TextField
              label="Timeout (milliseconds)"
              type="number"
              min={1}
              required
              value={config.timeout_ms}
              onChange={(event) =>
                setConfig({ ...config, timeout_ms: Number(event.target.value) })
              }
            />
            <TextField
              label="Maximum output bytes"
              type="number"
              min={1}
              required
              value={config.max_output_bytes}
              onChange={(event) =>
                setConfig({
                  ...config,
                  max_output_bytes: Number(event.target.value),
                })
              }
            />
          </Grid>
          <TextArea
            label="Vault secret references (JSON map)"
            monospace
            rows={3}
            value={secretRefs}
            onChange={(event) => setSecretRefs(event.target.value)}
            hint="Map environment variables (stdio) or headers (HTTPS) to vault secret names. Never put credential values here."
          />
          <TextField
            label="New vault credential name (optional)"
            value={secretName}
            onChange={(event) => setSecretName(event.target.value)}
          />
          <PasswordField
            label="New credential value (optional)"
            value={credential}
            onChange={(event) => setCredential(event.target.value)}
            hint={`Stored for mcp:${config.id}. Add its name to the secret reference map above.`}
          />
          <Stack direction="row" gap={2}>
            <Button type="submit" disabled={action.busy}>
              {action.busy ? "Saving…" : "Save server"}
            </Button>
            <Button variant="ghost" disabled={action.busy} onClick={onCancel}>
              Cancel
            </Button>
          </Stack>
        </Stack>
      </form>
    </Card>
  );
}
