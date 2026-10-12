import { useState } from "react";
import { EmptyState, InlineCode, List, ListRow, Notice, SectionHeader } from "@aidex/ui/components";
import { BrandIcon, PlusIcon, ServerIcon, UploadIcon } from "@aidex/ui/icons";
import { Badge, Button, Split, Stack, StatusIndicator, Switch, Text } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import type { McpServer } from "@aidex/client/dto/mcp";
import { describeError, useAction, useExtensionsQuery, useWorkspaceScope } from "../data";
import { McpImportDialog } from "./McpImportDialog";
import { McpServerDetail } from "./McpServerDetail";
import { McpServerForm } from "./McpServerForm";
import { serverBrand, sourceLabels, statusLabels, transportLabels } from "./mcpLabels";

type Props = {
  /** The server shown in the detail pane (route `id`); defaults to the first. */
  selectedId?: string;
  onSelect: (id: string | undefined) => void;
};

/** Poll quickly while a server is starting, slowly otherwise, so status dots stay current. */
const STARTING_POLL_MS = 1500;
const IDLE_POLL_MS = 10_000;

type FormState = { session: number; open: boolean; initial?: McpServer };

/** MCP servers tab: the server list with status and enable switches beside the selected server's details. */
export function McpPanel({ selectedId, onSelect }: Props) {
  const scope = useWorkspaceScope();
  const [starting, setStarting] = useState(false);
  const listing = useExtensionsQuery("mcp.list", undefined, { pollMs: starting ? STARTING_POLL_MS : IDLE_POLL_MS });
  const toggles = useAction();
  const [form, setForm] = useState<FormState>({ session: 0, open: false });
  const [importing, setImporting] = useState({ session: 0, open: false });

  const servers = listing.data?.servers ?? [];
  const anyStarting = servers.some((server) => server.status === "starting");
  if (anyStarting !== starting) setStarting(anyStarting);
  const selected = servers.find((server) => server.id === selectedId) ?? servers[0];
  const running = servers.filter((server) => server.status === "running").length;

  const openForm = (initial?: McpServer) => setForm((current) => ({ session: current.session + 1, open: true, initial }));
  const closeForm = () => setForm((current) => ({ ...current, open: false }));
  const toggle = (server: McpServer, enabled: boolean) =>
    void toggles.run(enabled ? "enable" : "disable", () => call("mcp.set_enabled", { id: server.id, enabled }));

  const list =
    servers.length === 0 ? (
      listing.loading || listing.error ? (
        listing.error ? null : <Text tone="muted">Loading MCP servers…</Text>
      ) : (
        <EmptyState
          icon={<ServerIcon />}
          title="No MCP servers yet"
          description="Add a server by command or URL, import a workspace's .mcp.json, or install a plugin that brings servers."
          actions={
            <Button variant="primary" icon={<PlusIcon />} onClick={() => openForm()}>
              Add server
            </Button>
          }
        />
      )
    ) : (
      <section aria-labelledby="mcp-servers-heading">
        <SectionHeader title={<span id="mcp-servers-heading">Servers</span>} meta={`${running} of ${servers.length} running`} />
        <List>
          {servers.map((server) => {
            const status = statusLabels[server.status];
            return (
              <ListRow
                key={server.id}
                media={<BrandIcon brand={serverBrand(server)} monogramLabel={server.name} size={28} aria-hidden />}
                title={server.name}
                subtitle={`${transportLabels[server.config.transport]} · ${server.endpoint}`}
                meta={[
                  <StatusIndicator key="status" tone={status.tone} label={status.label} />,
                  server.status === "running" ? (
                    <Text as="span" key="tools" tone="muted">
                      {server.tool_count === 1 ? "1 tool" : `${server.tool_count} tools`}
                    </Text>
                  ) : null,
                  <Badge key="source" tone="neutral" size="sm">
                    {sourceLabels[server.source]}
                  </Badge>,
                ]}
                selected={server.id === selected?.id}
                onSelect={() => onSelect(server.id)}
                trailing={
                  <Switch
                    label={`Enable ${server.name}`}
                    checked={server.enabled}
                    disabled={toggles.busy !== null}
                    onCheckedChange={(enabled) => toggle(server, enabled)}
                  />
                }
              />
            );
          })}
        </List>
      </section>
    );

  return (
    <Stack gap={5}>
      <Stack direction="row" gap={3} align="center" justify="between" wrap>
        <Text tone="secondary">Agents use MCP tools in Agent mode. A server starts the first time its tools are needed.</Text>
        <Stack direction="row" gap={2}>
          <Button
            variant="outline"
            icon={<UploadIcon />}
            onClick={() => setImporting((current) => ({ session: current.session + 1, open: true }))}
          >
            Import from .mcp.json
          </Button>
          <Button variant="primary" icon={<PlusIcon />} onClick={() => openForm()}>
            Add server
          </Button>
        </Stack>
      </Stack>
      {listing.error ? (
        <Notice tone="danger" role="alert">
          {describeError(listing.error)}
        </Notice>
      ) : null}
      {toggles.error ? (
        <Notice tone="danger" role="alert">
          {toggles.error}
        </Notice>
      ) : null}
      {selected ? (
        <Split divided emphasis="equal">
          {list}
          <McpServerDetail
            key={selected.id}
            server={selected}
            workspaces={scope.workspaces}
            onEdit={openForm}
            onRemoved={() => onSelect(undefined)}
          />
        </Split>
      ) : (
        list
      )}
      {listing.data && listing.data.invalid.length > 0 ? (
        <Notice tone="warning">
          <Stack gap={2}>
            <Text as="span">These server files could not be read and are ignored:</Text>
            {listing.data.invalid.map((entry) => (
              <Text as="span" key={entry.id} tone="secondary">
                <InlineCode>{entry.id}</InlineCode>: {entry.error}
              </Text>
            ))}
          </Stack>
        </Notice>
      ) : null}
      {form.session > 0 ? (
        <McpServerForm
          key={form.session}
          open={form.open}
          initial={form.initial}
          workspaces={scope.workspaces}
          onClose={closeForm}
          onSaved={(server) => {
            closeForm();
            onSelect(server.id);
          }}
        />
      ) : null}
      {importing.session > 0 ? (
        <McpImportDialog
          key={importing.session}
          open={importing.open}
          workspaces={scope.workspaces}
          initialWorkspaceId={scope.workspaceId}
          onClose={() => setImporting((current) => ({ ...current, open: false }))}
        />
      ) : null}
    </Stack>
  );
}
