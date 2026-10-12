import { hasBrandIcon } from "@aidex/ui/icons";
import type { McpServer, McpSourceKind, McpStatus, McpTransport } from "@aidex/client/dto/mcp";
import type { Workspace } from "@aidex/client/dto/workspace";

type Tone = "success" | "warning" | "danger" | "neutral";

export const statusLabels: Record<McpStatus, { label: string; tone: Tone }> = {
  stopped: { label: "Stopped", tone: "neutral" },
  starting: { label: "Starting", tone: "warning" },
  running: { label: "Running", tone: "success" },
  failed: { label: "Failed", tone: "danger" },
  needs_auth: { label: "Needs credentials", tone: "warning" },
};

export const transportLabels: Record<McpTransport, string> = {
  stdio: "Local command",
  http: "Remote URL",
};

export const sourceLabels: Record<McpSourceKind, string> = {
  drive: "Drive",
  workspace: "Workspace",
  plugin: "Plugin",
};

/** "Workspace · finapp" or "Plugin · code-review"; drive servers need no owner. */
export function sourceText(server: McpServer, workspaces: readonly Workspace[]): string {
  if (!server.source_owner || server.source === "drive") return sourceLabels[server.source];
  const owner =
    server.source === "workspace"
      ? (workspaces.find((workspace) => workspace.id === server.source_owner)?.name ?? server.source_owner)
      : server.source_owner;
  return `${sourceLabels[server.source]} · ${owner}`;
}

/** A bundled brand glyph when the server is named after a known service (GitHub, Brave, ...), else the MCP glyph. */
export function serverBrand(server: Pick<McpServer, "id" | "name">): string {
  const words = `${server.id} ${server.name}`.toLowerCase().split(/[^a-z0-9]+/u);
  return words.find((word) => word.length > 1 && word !== "mcp" && hasBrandIcon(word)) ?? "mcp";
}

/** The server can be started (it is enabled and not already up). */
export function canStart(server: McpServer): boolean {
  return server.enabled && (server.status === "stopped" || server.status === "failed" || server.status === "needs_auth");
}

/** The vault consumer an MCP server's secrets must be released to. */
export function mcpConsumer(serverId: string): string {
  return `mcp:${serverId}`;
}

/** Splits a one-per-line text field into trimmed, non-empty lines. */
export function lines(text: string): string[] {
  return text
    .split(/\r?\n/u)
    .map((line) => line.trim())
    .filter(Boolean);
}
