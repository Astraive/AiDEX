import { Notice, SectionHeader, Terminal, type TerminalLine } from "@aidex/ui/components";
import { RefreshIcon } from "@aidex/ui/icons";
import { IconButton, Stack, Text } from "@aidex/ui/primitives";
import type { McpLogLine, McpLogStream, McpServer } from "@aidex/client/dto/mcp";
import { describeError, useExtensionsQuery } from "../data";

const LOG_LIMIT = 200;
const LIVE_POLL_MS = 3000;

const streamLabels: Record<McpLogStream, string> = {
  stderr: "stderr",
  server: "server",
  client: "aidex",
};

function timeOf(at: string): string {
  const date = new Date(at);
  return Number.isNaN(date.getTime()) ? at : date.toLocaleTimeString(undefined, { hour12: false });
}

function toTerminalLine(line: McpLogLine): TerminalLine {
  return { kind: "output", text: `${timeOf(line.at)}  ${streamLabels[line.stream]}  ${line.text}` };
}

/** The server's recent log (stderr, its log notifications, and AiDEX's own notes), polled while it runs. */
export function McpLogs({ server }: { server: McpServer }) {
  const live = server.status === "running" || server.status === "starting";
  const logs = useExtensionsQuery("mcp.logs", { id: server.id, limit: LOG_LIMIT }, { pollMs: live ? LIVE_POLL_MS : undefined });
  const lines = logs.data?.id === server.id ? logs.data.lines : [];
  const dropped = logs.data?.id === server.id ? logs.data.dropped : 0;

  return (
    <Stack gap={3}>
      <SectionHeader level={3} title="Log" meta={live ? "Live" : undefined} />
      {logs.error ? (
        <Notice tone="warning">{describeError(logs.error)}</Notice>
      ) : lines.length > 0 ? (
        <Terminal
          title={`${server.name} log`}
          lines={lines.map(toTerminalLine)}
          actions={<IconButton size="sm" label="Refresh log" icon={<RefreshIcon />} onClick={logs.refetch} />}
        />
      ) : (
        <Text tone="muted">{logs.loading ? "Loading the log…" : "Nothing logged yet. Start the server to see its output."}</Text>
      )}
      {dropped > 0 ? <Text tone="muted">{dropped} older lines are no longer kept.</Text> : null}
    </Stack>
  );
}
