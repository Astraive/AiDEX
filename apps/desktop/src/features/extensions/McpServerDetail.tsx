import { useState } from "react";
import { CodeBlock, Notice, SectionHeader } from "@aidex/ui/components";
import { Button, Card, Select, Stack, Text } from "@aidex/ui/primitives";
import type { McpServerSummary } from "@aidex/client/dto/extensions";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import { useRpc } from "../../hooks/useRpc";
import { permissionOptions } from "../agent-center/runtimeSelection";
import { useExtensionAction } from "./useExtensionAction";

export function McpServerDetail({
  server,
  onEdit,
  onChanged,
}: {
  server: McpServerSummary;
  onEdit: () => void;
  onChanged: () => void;
}) {
  const workspaces = useRpc("workspace.list");
  const [workspaceId, setWorkspaceId] = useState("");
  const [profile, setProfile] = useState("supervised");
  const [cancelling, setCancelling] = useState(false);
  const [cancelError, setCancelError] = useState<string>();
  const action = useExtensionAction(onChanged);
  const scope = {
    id: server.id,
    workspace_id: workspaceId,
    profile,
    full_access: profile === "full_access",
  };
  const allowed = (grants: string[], value: string) =>
    grants.includes("*") || grants.includes(value);
  const canConnect =
    !server.connecting &&
    server.enabled &&
    server.trusted &&
    !!workspaceId &&
    allowed(server.workspace_ids, workspaceId) &&
    allowed(server.profiles, profile);
  return (
    <Card padding="md">
      <Stack gap={4}>
        <SectionHeader
          title={server.name}
          meta={
            server.connecting
              ? "Connecting…"
              : server.connected
                ? "Connected"
                : "Disconnected"
          }
        />
        {(cancelError ||
          action.error ||
          server.last_error ||
          workspaces.error) && (
          <Notice tone="danger">
            {cancelError ??
              action.error ??
              server.last_error ??
              workspaces.error?.message}
          </Notice>
        )}
        {action.message && <Notice tone="success">{action.message}</Notice>}
        <Text>
          {server.transport.kind === "stdio"
            ? `${server.transport.command} ${server.transport.args.join(" ")}`
            : server.transport.url}
        </Text>
        <Text tone="muted">
          Workspace grants: {server.workspace_ids.join(", ") || "none"} ·
          Profiles: {server.profiles.join(", ") || "none"}
        </Text>
        <Select
          label="Connection workspace"
          disabled={action.busy || server.connecting}
          value={workspaceId}
          options={[
            { value: "", label: "Choose a granted workspace" },
            ...(workspaces.data?.workspaces ?? []).map((workspace) => ({
              value: workspace.id,
              label: workspace.name,
              disabled:
                workspace.availability.state !== "available" ||
                !allowed(server.workspace_ids, workspace.id),
            })),
          ]}
          onChange={(event) => setWorkspaceId(event.target.value)}
        />
        <Select
          label="Connection permission profile"
          disabled={action.busy || server.connecting}
          value={profile}
          options={permissionOptions.map((option) => ({
            ...option,
            disabled: !allowed(server.profiles, option.value),
          }))}
          onChange={(event) => setProfile(event.target.value)}
        />
        {profile === "full_access" && (
          <Notice tone="warning">
            Full access permits the MCP process to run as your current user.
            Server grants still restrict which AiDEX sessions may call it.
          </Notice>
        )}
        {!server.trusted && (
          <Notice>
            Trust and explicitly grant this server a workspace and permission
            profile before connecting.
          </Notice>
        )}
        <Stack direction="row" gap={2} wrap>
          <Button
            disabled={action.busy || !canConnect}
            onClick={() =>
              void action.act(
                () => call("extensions.mcp.connect", scope),
                "MCP connection requested.",
              )
            }
          >
            Connect
          </Button>
          <Button
            variant="outline"
            disabled={action.busy || !canConnect}
            onClick={() =>
              void action.act(
                () => call("extensions.mcp.discover", scope),
                "Tool discovery requested.",
              )
            }
          >
            Test and discover tools
          </Button>
          <Button
            variant="outline"
            disabled={action.busy || !canConnect}
            onClick={() =>
              void action.act(async () => {
                await call("extensions.mcp.disconnect", { id: server.id });
                await call("extensions.mcp.connect", scope);
              }, "MCP restart requested.")
            }
          >
            Restart
          </Button>
          <Button
            variant="ghost"
            disabled={action.busy || !server.connected}
            onClick={() =>
              void action.act(
                () => call("extensions.mcp.disconnect", { id: server.id }),
                "MCP disconnected.",
              )
            }
          >
            Disconnect
          </Button>
          {(action.busy || server.connecting) && (
            <Button
              variant="outline"
              disabled={cancelling}
              onClick={() => {
                setCancelling(true);
                setCancelError(undefined);
                void call("extensions.mcp.cancel", { id: server.id })
                  .then(onChanged)
                  .catch((caught) =>
                    setCancelError(toAidexError(caught).message),
                  )
                  .finally(() => setCancelling(false));
              }}
            >
              {cancelling ? "Cancelling…" : "Cancel connection request"}
            </Button>
          )}
          {!server.plugin_id && (
            <Button variant="ghost" onClick={onEdit}>
              Edit configuration
            </Button>
          )}
        </Stack>
        {server.plugin_id && (
          <Text tone="muted">
            Configuration is managed by plugin {server.plugin_id}.
          </Text>
        )}
        <Text tone="muted">
          Secret references:{" "}
          {Object.keys(server.secret_refs).join(", ") || "none"} · Catalog
          revision: {server.catalog_revision || "not discovered"}
        </Text>
        <SectionHeader
          title="Discovered tools"
          meta={`${server.tools.length}`}
        />
        <Text tone="muted">
          Native invocation tool: {server.call_tool_name}. Stdio calls require
          full access; HTTPS calls use broker approval in supervised mode.
        </Text>
        {server.owner_session_id && (
          <Text tone="muted">Connection owner: {server.owner_session_id}</Text>
        )}
        {server.tools.map((tool) => (
          <details key={tool.name}>
            <summary>
              {tool.name} · {tool.description}
            </summary>
            <CodeBlock
              title="Input schema"
              language="json"
              code={JSON.stringify(tool.inputSchema, null, 2)}
            />
            {tool.outputSchema != null && (
              <CodeBlock
                title="Output schema"
                language="json"
                code={JSON.stringify(tool.outputSchema, null, 2)}
              />
            )}
          </details>
        ))}
        {!server.tools.length && (
          <Text tone="muted">
            Connect and discover to inspect the server's real tools.
          </Text>
        )}
      </Stack>
    </Card>
  );
}
