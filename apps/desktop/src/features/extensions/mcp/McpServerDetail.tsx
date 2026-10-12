import { useState } from "react";
import { Dialog, InlineCode, KeyValueList, List, ListRow, MediaItem, Notice, SectionHeader, formatDateTime } from "@aidex/ui/components";
import { BrandIcon, PencilIcon, PlayIcon, RefreshIcon, StopIcon, TrashIcon } from "@aidex/ui/icons";
import { Badge, Button, Stack, StatusIndicator, Text } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import type { McpServer, McpToolSummary } from "@aidex/client/dto/mcp";
import type { Workspace } from "@aidex/client/dto/workspace";
import { useAction } from "../data";
import { McpLogs } from "./McpLogs";
import { McpSecrets } from "./McpSecrets";
import { canStart, serverBrand, sourceText, statusLabels, transportLabels } from "./mcpLabels";

type Props = {
  server: McpServer;
  workspaces: readonly Workspace[];
  onEdit: (server: McpServer) => void;
  onRemoved: () => void;
};

const lifecycleMethods = { start: "mcp.start", stop: "mcp.stop", restart: "mcp.restart" } as const;

function dateText(value: string | undefined): string | null {
  if (!value) return null;
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? null : formatDateTime(date);
}

type LoadedTools = { list: McpToolSummary[]; truncated: boolean; notes: string[] };

/** What a loaded tool list belongs to: one run of one configuration. */
type ServerRun = { config: string; status: McpServer["status"]; startedAt: string | undefined };

/**
 * The configuration changed, or the run that was running stopped, failed, began restarting, or was
 * replaced by another process, so the tools it listed are gone. A server still starting has no run
 * yet, which keeps the answer of "Start and list tools".
 */
function replacedRun(before: ServerRun, after: ServerRun): boolean {
  if (before.config !== after.config) return true;
  return before.status === "running" && (after.status !== "running" || after.startedAt !== before.startedAt);
}

/** The selected server: status and lifecycle actions, facts, tools, secrets, and its log. */
export function McpServerDetail({ server, workspaces, onEdit, onRemoved }: Props) {
  const action = useAction();
  const [tools, setTools] = useState<LoadedTools | null>(null);
  const [confirmingRemove, setConfirmingRemove] = useState(false);
  const status = statusLabels[server.status];
  const live = server.status === "running" || server.status === "starting";
  // `mcp.tools` answers for the run it listed. An edit (which stops the server), a stop or failure, or a
  // restart elsewhere drops it, so the list falls back to what `mcp.list` reports now.
  const run: ServerRun = { config: JSON.stringify(server.config), status: server.status, startedAt: server.started_at };
  const [seenRun, setSeenRun] = useState(run);
  if (seenRun.config !== run.config || seenRun.status !== run.status || seenRun.startedAt !== run.startedAt) {
    setSeenRun(run);
    if (replacedRun(seenRun, run)) setTools(null);
  }
  const shownTools = tools?.list ?? server.tools;
  const autoApproved = new Set(server.config.auto_approve_tools);
  const fromPlugin = server.source === "plugin";
  const started = dateText(server.started_at);
  const workspaceNames = server.config.workspaces.map(
    (id) => workspaces.find((workspace) => workspace.id === id)?.name ?? id,
  );

  const lifecycle = (verb: keyof typeof lifecycleMethods) => {
    // Start, Stop, and Restart all replace the running instance, and with it the tools it listed.
    setTools(null);
    void action.run(verb, () => call(lifecycleMethods[verb], { id: server.id }));
  };

  const loadTools = async () => {
    const result = await action.run("tools", () => call("mcp.tools", { id: server.id }));
    if (result) setTools({ list: result.tools, truncated: result.truncated, notes: result.notes });
  };

  const remove = async () => {
    const removed = await action.run("remove", () => call("mcp.remove", { id: server.id }));
    if (!removed) return;
    setConfirmingRemove(false);
    onRemoved();
  };

  return (
    <section aria-label={server.name}>
      <Stack gap={5}>
        <MediaItem
          titleLevel={2}
          media={<BrandIcon brand={serverBrand(server)} monogramLabel={server.name} size={32} aria-hidden />}
          title={server.name}
          description={transportLabels[server.config.transport]}
          trailing={<StatusIndicator tone={status.tone} label={status.label} />}
        />
        <Stack direction="row" gap={2} wrap>
          {live ? (
            <>
              <Button variant="outline" icon={<RefreshIcon />} disabled={action.busy !== null} onClick={() => lifecycle("restart")}>
                Restart
              </Button>
              <Button variant="outline" icon={<StopIcon />} disabled={action.busy !== null} onClick={() => lifecycle("stop")}>
                Stop
              </Button>
            </>
          ) : (
            <Button
              variant="primary"
              icon={<PlayIcon />}
              disabled={!canStart(server) || action.busy !== null}
              onClick={() => lifecycle(server.status === "failed" ? "restart" : "start")}
            >
              {action.busy === "start" || action.busy === "restart" ? "Starting…" : "Start"}
            </Button>
          )}
          <Button variant="ghost" icon={<PencilIcon />} onClick={() => onEdit(server)}>
            Edit configuration
          </Button>
          {fromPlugin ? null : (
            <Button variant="ghost" icon={<TrashIcon />} onClick={() => setConfirmingRemove(true)}>
              Remove
            </Button>
          )}
        </Stack>
        {action.error && !confirmingRemove ? (
          <Notice tone="danger" role="alert">
            {action.error}
          </Notice>
        ) : null}
        {!server.enabled ? (
          <Notice tone="info">
            This server is off. Turn it on in the list to start it and offer its tools to agents in Agent mode.
          </Notice>
        ) : server.status === "needs_auth" ? (
          <Notice tone="warning">The server asked for credentials. Store its secrets below, then start it again.</Notice>
        ) : server.error ? (
          <Notice tone="danger" variant="banner">
            {server.error.message}
          </Notice>
        ) : null}
        <KeyValueList
          items={[
            {
              label: server.config.transport === "http" ? "URL" : "Command",
              value: <InlineCode>{server.endpoint}</InlineCode>,
            },
            { label: "Source", value: sourceText(server, workspaces) },
            { label: "Workspaces", value: workspaceNames.length > 0 ? workspaceNames.join(", ") : "All workspaces" },
            ...(server.server
              ? [
                  {
                    label: "Server",
                    value: `${server.server.name} ${server.server.version} · MCP ${server.server.protocol_version}`,
                  },
                ]
              : []),
            ...(started && live ? [{ label: "Started", value: started }] : []),
          ]}
        />
        {fromPlugin ? (
          <Text tone="muted">
            This server comes from the {server.source_owner} plugin. Disabling or uninstalling the plugin removes it.
          </Text>
        ) : null}
        <Stack gap={3}>
          <SectionHeader
            level={3}
            title="Tools"
            meta={live || tools ? `${tools?.list.length ?? server.tool_count}` : undefined}
            actions={
              <Button
                variant="ghost"
                icon={<RefreshIcon />}
                disabled={!server.enabled || action.busy !== null}
                onClick={() => void loadTools()}
              >
                {action.busy === "tools" ? "Listing…" : live ? "Refresh tools" : "Start and list tools"}
              </Button>
            }
          />
          {shownTools.length > 0 ? (
            <List>
              {shownTools.map((tool) => (
                <ListRow
                  key={tool.name}
                  title={<InlineCode>{tool.name}</InlineCode>}
                  subtitle={tool.description || undefined}
                  meta={
                    autoApproved.has(tool.name)
                      ? [
                          <Badge key="approval" tone="neutral" size="sm">
                            Runs without asking
                          </Badge>,
                        ]
                      : undefined
                  }
                />
              ))}
            </List>
          ) : (
            <Text tone="muted">
              {live ? "The server offers no tools." : "Tools are listed while the server runs."}
            </Text>
          )}
          {tools?.truncated ? <Text tone="muted">More tools exist than fit in this list.</Text> : null}
          {tools?.notes.map((note) => (
            <Text key={note} tone="muted">
              {note}
            </Text>
          ))}
        </Stack>
        <McpSecrets server={server} />
        <McpLogs server={server} />
      </Stack>
      <Dialog
        open={confirmingRemove}
        title={`Remove ${server.name}?`}
        description="AiDEX stops the server and deletes its configuration file. Vault secrets it used stay in the vault."
        onClose={() => {
          setConfirmingRemove(false);
          action.clearError();
        }}
      >
        <Stack gap={4}>
          {action.error ? (
            <Notice tone="danger" role="alert">
              {action.error}
            </Notice>
          ) : null}
          <Stack direction="row" gap={2} justify="end">
            <Button variant="ghost" onClick={() => setConfirmingRemove(false)}>
              Cancel
            </Button>
            <Button variant="primary" icon={<TrashIcon />} disabled={action.busy !== null} onClick={() => void remove()}>
              {action.busy === "remove" ? "Removing…" : "Remove server"}
            </Button>
          </Stack>
        </Stack>
      </Dialog>
    </section>
  );
}
