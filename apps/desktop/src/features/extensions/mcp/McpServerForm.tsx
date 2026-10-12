import { useState, type FormEvent } from "react";
import { Dialog, Notice } from "@aidex/ui/components";
import { Button, CheckboxGroup, Grid, SegmentedControl, Stack, Text, TextArea, TextField } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import type { McpEnvSecret, McpHeaderSecret, McpServer, McpServerSpec, McpTransport } from "@aidex/client/dto/mcp";
import type { Workspace } from "@aidex/client/dto/workspace";
import { useAction } from "../data";
import { SECRET_NAME_PATTERN } from "../vaultSecrets";
import { lines, transportLabels } from "./mcpLabels";

type Props = {
  open: boolean;
  /** The server being edited; omit to add a new one. */
  initial?: McpServer;
  workspaces: readonly Workspace[];
  onClose: () => void;
  onSaved: (server: McpServer) => void;
};

type Errors = Partial<Record<"id" | "name" | "command" | "url" | "envSecrets" | "headerSecrets" | "envAllowlist", string>>;

const SERVER_ID_PATTERN = /^[a-z0-9][a-z0-9_-]{0,63}$/;
const ENV_NAME_PATTERN = /^[A-Za-z_][A-Za-z0-9_]*$/;
const HEADER_NAME_PATTERN = /^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/;

const transportOptions = (Object.keys(transportLabels) as McpTransport[]).map((value) => ({
  value,
  label: transportLabels[value],
}));

/** A server id from its display name: lowercase words joined by hyphens. */
function slugOf(name: string): string {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9]+/gu, "-")
    .replace(/^-+|-+$/gu, "")
    .slice(0, 64);
}

/** `NAME=secret-name` lines to mappings; reports the first malformed line. */
function parseMappings(
  text: string,
  keyPattern: RegExp,
  what: string,
): { pairs: { key: string; secret: string }[]; error?: string } {
  const pairs: { key: string; secret: string }[] = [];
  for (const [index, line] of lines(text).entries()) {
    const equals = line.indexOf("=");
    const key = equals > 0 ? line.slice(0, equals).trim() : "";
    const secret = equals > 0 ? line.slice(equals + 1).trim() : "";
    if (!keyPattern.test(key) || !SECRET_NAME_PATTERN.test(secret))
      return { pairs, error: `Line ${index + 1}: write ${what}=secret-name, with a vault secret name after the =.` };
    pairs.push({ key, secret });
  }
  return { pairs };
}

const mappingText = (pairs: readonly { key: string; secret: string }[]) =>
  pairs.map((pair) => `${pair.key}=${pair.secret}`).join("\n");

/**
 * Adds or edits an MCP server file (`mcp.add` / `mcp.update`). It holds the launch command or URL
 * and the NAMES of vault secrets only; values are stored separately in the vault. Saving an edit
 * stops a running server so its next start uses the new settings. Give it a new `key` per opening.
 */
export function McpServerForm({ open, initial, workspaces, onClose, onSaved }: Props) {
  const editing = initial !== undefined;
  const config = initial?.config;
  const [name, setName] = useState(config?.name ?? "");
  const [id, setId] = useState(config?.id ?? "");
  const [idTouched, setIdTouched] = useState(editing);
  const [transport, setTransport] = useState<McpTransport>(config?.transport ?? "stdio");
  const [command, setCommand] = useState(config?.command ?? "");
  const [args, setArgs] = useState((config?.args ?? []).join("\n"));
  const [envAllowlist, setEnvAllowlist] = useState((config?.env_allowlist ?? []).join("\n"));
  const [envSecrets, setEnvSecrets] = useState(
    mappingText((config?.env_from_secrets ?? []).map((entry) => ({ key: entry.env, secret: entry.secret_name }))),
  );
  const [url, setUrl] = useState(config?.url ?? "");
  const [headerSecrets, setHeaderSecrets] = useState(
    mappingText((config?.headers_from_secrets ?? []).map((entry) => ({ key: entry.header, secret: entry.secret_name }))),
  );
  const [chosenWorkspaces, setChosenWorkspaces] = useState<string[]>(config?.workspaces ?? []);
  const [autoApprove, setAutoApprove] = useState((config?.auto_approve_tools ?? []).join("\n"));
  const [errors, setErrors] = useState<Errors>({});
  const action = useAction();

  const effectiveId = idTouched ? id.trim() : slugOf(name);
  const clear = (field: keyof Errors) => setErrors((current) => ({ ...current, [field]: undefined }));

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const next: Errors = {};
    if (!SERVER_ID_PATTERN.test(effectiveId))
      next.id = "Use 1-64 lowercase letters, digits, `-`, or `_`, starting with a letter or digit.";
    if (!name.trim()) next.name = "Name the server.";
    const env = parseMappings(envSecrets, ENV_NAME_PATTERN, "VARIABLE");
    const headers = parseMappings(headerSecrets, HEADER_NAME_PATTERN, "Header-Name");
    const allowlist = lines(envAllowlist);
    if (transport === "stdio") {
      if (!command.trim()) next.command = "Enter the program that starts the server, e.g. npx or uvx.";
      if (env.error) next.envSecrets = env.error;
      const badName = allowlist.find((entry) => !ENV_NAME_PATTERN.test(entry));
      if (badName) next.envAllowlist = `${badName} is not an environment variable name.`;
    } else {
      if (!/^https?:\/\//iu.test(url.trim())) next.url = "Enter the server's http(s) URL.";
      if (headers.error) next.headerSecrets = headers.error;
    }
    setErrors(next);
    if (Object.keys(next).length > 0) return;

    const envFromSecrets: McpEnvSecret[] = env.pairs.map((pair) => ({ env: pair.key, secret_name: pair.secret }));
    const headersFromSecrets: McpHeaderSecret[] = headers.pairs.map((pair) => ({ header: pair.key, secret_name: pair.secret }));
    const spec: McpServerSpec = {
      id: effectiveId,
      name: name.trim(),
      transport,
      ...(transport === "stdio"
        ? {
            command: command.trim(),
            args: args.split(/\r?\n/u).filter((line) => line.trim() !== ""),
            env_allowlist: allowlist,
            env_from_secrets: envFromSecrets,
            url: null,
            headers_from_secrets: [],
          }
        : { url: url.trim(), headers_from_secrets: headersFromSecrets, command: null, args: [], env_allowlist: [], env_from_secrets: [] }),
      // `mcp.update` replaces the whole file, so an edit keeps the current switch; new servers start off.
      enabled: initial?.enabled ?? false,
      workspaces: chosenWorkspaces,
      auto_approve_tools: lines(autoApprove),
    };
    const saved = await action.run("save", () => call(editing ? "mcp.update" : "mcp.add", spec));
    if (saved) onSaved(saved);
  };

  return (
    <Dialog
      open={open}
      size="lg"
      title={editing ? `Edit ${config?.name ?? "server"}` : "Add MCP server"}
      description="MCP servers give agents extra tools. AiDEX launches local servers in its sandbox with a cleared environment and asks before their tools run."
      onClose={onClose}
    >
      <form onSubmit={(event) => void submit(event)} noValidate aria-label={editing ? "Edit MCP server" : "Add MCP server"}>
        <Stack gap={5}>
          <Grid columns={2} gap={4}>
            <TextField
              label="Name"
              size="md"
              placeholder="GitHub"
              value={name}
              error={errors.name}
              onChange={(event) => {
                setName(event.target.value);
                clear("name");
              }}
            />
            <TextField
              label="ID"
              size="md"
              autoComplete="off"
              spellCheck={false}
              readOnly={editing}
              value={effectiveId}
              error={errors.id}
              hint={editing ? "The ID names the server's file and can't change." : "Agents see tools as mcp__<id>__<tool>."}
              onChange={(event) => {
                setIdTouched(true);
                setId(event.target.value);
                clear("id");
              }}
            />
          </Grid>
          <SegmentedControl label="Transport" options={transportOptions} value={transport} onValueChange={setTransport} />
          {transport === "stdio" ? (
            <Stack gap={4}>
              <TextField
                label="Command"
                size="md"
                placeholder="npx"
                autoComplete="off"
                spellCheck={false}
                value={command}
                error={errors.command}
                onChange={(event) => {
                  setCommand(event.target.value);
                  clear("command");
                }}
              />
              <TextArea
                label="Arguments"
                monospace
                rows={4}
                spellCheck={false}
                placeholder={"-y\n@modelcontextprotocol/server-github"}
                hint="One argument per line."
                value={args}
                onChange={(event) => setArgs(event.target.value)}
              />
              <Grid columns={2} gap={4}>
                <TextArea
                  label="Environment variables to pass through"
                  monospace
                  rows={3}
                  spellCheck={false}
                  placeholder={"HOME\nNODE_OPTIONS"}
                  hint="One name per line, copied from AiDEX's environment. Everything else is cleared."
                  value={envAllowlist}
                  error={errors.envAllowlist}
                  onChange={(event) => {
                    setEnvAllowlist(event.target.value);
                    clear("envAllowlist");
                  }}
                />
                <TextArea
                  label="Environment variables from vault secrets"
                  monospace
                  rows={3}
                  spellCheck={false}
                  placeholder="GITHUB_TOKEN=github-token"
                  hint="One VARIABLE=secret-name per line."
                  value={envSecrets}
                  error={errors.envSecrets}
                  onChange={(event) => {
                    setEnvSecrets(event.target.value);
                    clear("envSecrets");
                  }}
                />
              </Grid>
            </Stack>
          ) : (
            <Stack gap={4}>
              <TextField
                label="URL"
                size="md"
                type="url"
                placeholder="https://mcp.example.com/mcp"
                autoComplete="off"
                spellCheck={false}
                value={url}
                error={errors.url}
                hint="AiDEX checks the offline switch and your network policy before connecting."
                onChange={(event) => {
                  setUrl(event.target.value);
                  clear("url");
                }}
              />
              <TextArea
                label="Headers from vault secrets"
                monospace
                rows={3}
                spellCheck={false}
                placeholder="Authorization=example-api-token"
                hint="One Header-Name=secret-name per line; the header carries the secret's value."
                value={headerSecrets}
                error={errors.headerSecrets}
                onChange={(event) => {
                  setHeaderSecrets(event.target.value);
                  clear("headerSecrets");
                }}
              />
            </Stack>
          )}
          {workspaces.length > 0 ? (
            <Stack gap={2}>
              <CheckboxGroup
                label="Offer in workspaces"
                options={workspaces.map((workspace) => ({ value: workspace.id, title: workspace.name, description: workspace.path }))}
                value={chosenWorkspaces}
                onValueChange={setChosenWorkspaces}
              />
              <Text tone="muted">Leave every workspace unchecked to offer the server everywhere.</Text>
            </Stack>
          ) : null}
          <TextArea
            label="Tools that run without asking"
            monospace
            rows={3}
            spellCheck={false}
            placeholder="search_issues"
            hint="One tool name per line. In Supervised mode every other tool asks for approval first."
            value={autoApprove}
            onChange={(event) => setAutoApprove(event.target.value)}
          />
          <Notice tone="info">
            Secret values never go in this file. After saving, store each named secret from the server&apos;s Secrets section.
          </Notice>
          {action.error ? (
            <Notice tone="danger" role="alert">
              {action.error}
            </Notice>
          ) : null}
          <Stack direction="row" gap={2} justify="end">
            <Button variant="ghost" onClick={onClose}>
              Cancel
            </Button>
            <Button type="submit" variant="primary" disabled={action.busy !== null}>
              {action.busy ? "Saving…" : editing ? "Save changes" : "Add server"}
            </Button>
          </Stack>
        </Stack>
      </form>
    </Dialog>
  );
}
