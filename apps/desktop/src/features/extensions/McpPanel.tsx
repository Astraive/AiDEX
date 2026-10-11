import { useEffect, useState } from "react";
import { List, ListRow, Notice } from "@aidex/ui/components";
import { Badge, Button, Stack, Text } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { useRpc } from "../../hooks/useRpc";
import { McpConfigForm } from "./McpConfigForm";
import { McpServerDetail } from "./McpServerDetail";
import { useExtensionAction } from "./useExtensionAction";

export function McpPanel({
  initialServerId,
  adding,
  onCloseAdd,
  onChanged,
}: {
  initialServerId?: string;
  adding: boolean;
  onCloseAdd: () => void;
  onChanged: () => void;
}) {
  const servers = useRpc("extensions.mcp.list");
  const connecting = servers.data?.some((server) => server.connecting) ?? false;
  useEffect(() => {
    if (!connecting) return;
    const timer = setInterval(servers.refetch, 1000);
    return () => clearInterval(timer);
  }, [connecting, servers.refetch]);
  const [selectedId, setSelectedId] = useState(initialServerId);
  const [editing, setEditing] = useState(false);
  const refresh = () => {
    servers.refetch();
    onChanged();
  };
  const action = useExtensionAction(refresh);
  const selected = servers.data?.find((server) => server.id === selectedId);
  return (
    <Stack gap={5}>
      {(servers.error || action.error) && (
        <Notice tone="danger">{action.error ?? servers.error?.message}</Notice>
      )}
      {servers.loading && (
        <Text tone="muted">Loading configured MCP servers…</Text>
      )}
      {adding && (
        <McpConfigForm
          onCancel={onCloseAdd}
          onSaved={(id) => {
            onCloseAdd();
            setSelectedId(id);
            refresh();
          }}
        />
      )}
      <List>
        {servers.data?.map((server) => (
          <ListRow
            key={server.id}
            title={server.name}
            subtitle={`${server.transport.kind} · ${server.id}`}
            selected={server.id === selectedId}
            meta={[
              <Badge
                key="connection"
                tone={server.connected ? "success" : "neutral"}
              >
                {server.connecting
                  ? "Connecting…"
                  : server.connected
                    ? "Connected"
                    : "Disconnected"}
              </Badge>,
              server.trusted ? "Trusted" : "Untrusted",
              server.enabled ? "Enabled" : "Disabled",
            ]}
            onSelect={() => {
              setSelectedId(server.id);
              setEditing(false);
            }}
            trailing={
              !server.plugin_id ? (
                <Button
                  variant="ghost"
                  disabled={action.busy}
                  onClick={() => {
                    if (window.confirm(`Remove MCP server ${server.name}?`))
                      void action.act(async () => {
                        await call("extensions.mcp.remove", { id: server.id });
                        if (selectedId === server.id) setSelectedId(undefined);
                      });
                  }}
                >
                  Remove
                </Button>
              ) : undefined
            }
          />
        ))}
      </List>
      {servers.data && !servers.data.length && (
        <Text tone="muted">
          No MCP servers configured. Add a trusted server and explicit
          workspace/profile grants.
        </Text>
      )}
      {selected &&
        (editing ? (
          <McpConfigForm
            key={selected.id}
            initial={selected}
            onCancel={() => setEditing(false)}
            onSaved={() => {
              setEditing(false);
              refresh();
            }}
          />
        ) : (
          <McpServerDetail
            key={selected.id}
            server={selected}
            onEdit={() => setEditing(true)}
            onChanged={refresh}
          />
        ))}
    </Stack>
  );
}
