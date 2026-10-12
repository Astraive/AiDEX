import { useEffect, useRef, useState } from "react";
import { Dialog, InlineCode, List, ListRow, Notice } from "@aidex/ui/components";
import { UploadIcon } from "@aidex/ui/icons";
import { Badge, Button, Stack, Text, type BadgeTone } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import type { McpImportEntry, McpImportResult, McpImportStatus } from "@aidex/client/dto/mcp";
import type { Workspace } from "@aidex/client/dto/workspace";
import { useToast } from "../../../app/providers/toast";
import { useAction } from "../data";
import { WorkspaceSelect } from "../WorkspaceSelect";

type Props = {
  open: boolean;
  workspaces: readonly Workspace[];
  /** The workspace to start with, e.g. the one the Skills tab shows. */
  initialWorkspaceId: string | undefined;
  onClose: () => void;
};

const statusBadges: Record<McpImportStatus, { label: string; tone: BadgeTone }> = {
  new: { label: "New", tone: "accent" },
  added: { label: "Added", tone: "success" },
  exists: { label: "Already added", tone: "neutral" },
  invalid: { label: "Can't import", tone: "danger" },
};

function entryDetail(entry: McpImportEntry): string {
  const config = entry.config;
  if (!config) return entry.status === "invalid" ? (entry.error ?? "This entry is not a valid server.") : "";
  if (config.transport === "http") return config.url ?? "";
  return [config.command, ...config.args].filter(Boolean).join(" ");
}

function droppedText(entry: McpImportEntry): string | null {
  const names = [...entry.dropped_env, ...entry.dropped_headers];
  if (names.length === 0) return null;
  return `Values for ${names.join(", ")} were not imported. Map them to vault secrets before starting the server.`;
}

/**
 * Reads a workspace's `.mcp.json` (Claude Code format) and saves each valid entry as a DISABLED
 * server offered in that workspace. A preview (`dry_run`) comes first; environment and header values
 * from the file are never imported. Give it a new `key` per opening.
 */
export function McpImportDialog({ open, workspaces, initialWorkspaceId, onClose }: Props) {
  const [workspaceId, setWorkspaceId] = useState(initialWorkspaceId ?? workspaces[0]?.id);
  const [preview, setPreview] = useState<McpImportResult | null>(null);
  const action = useAction();
  const notify = useToast();
  const importable = preview?.servers.filter((entry) => entry.status === "new").length ?? 0;

  const chosen = useRef(workspaceId);
  chosen.current = workspaceId;

  const runPreview = async (id: string | undefined) => {
    setPreview(null);
    if (!id) return;
    const result = await action.run("preview", () => call("mcp.import", { workspace_id: id, dry_run: true }));
    // A slower answer for a workspace the user has since switched away from is dropped.
    if (result && chosen.current === id) setPreview(result);
  };

  // Read the file as soon as the dialog opens and whenever another workspace is chosen.
  useEffect(() => {
    if (open && workspaceId) void runPreview(workspaceId);
    // `runPreview` reads the workspace only through its argument, so it is not a dependency.
  }, [open, workspaceId]);

  const runImport = async () => {
    if (!workspaceId) return;
    const result = await action.run("import", () => call("mcp.import", { workspace_id: workspaceId }));
    if (!result) return;
    const added = result.servers.filter((entry) => entry.status === "added").length;
    notify(
      added === 1 ? "Imported 1 server. Turn it on when you're ready." : `Imported ${added} servers. Turn each on when you're ready.`,
      "success",
    );
    onClose();
  };

  return (
    <Dialog
      open={open}
      size="lg"
      title="Import from .mcp.json"
      description="AiDEX reads the .mcp.json at a workspace's root and adds its servers turned off, offered only in that workspace."
      onClose={onClose}
    >
      <Stack gap={5}>
        {workspaces.length === 0 ? (
          <Notice tone="info">Add a workspace first; .mcp.json files are read from workspace folders.</Notice>
        ) : (
          <Stack direction="row" gap={3} align="end">
            <WorkspaceSelect
              label="Workspace"
              workspaces={workspaces}
              value={workspaceId}
              onChange={setWorkspaceId}
            />
            <Button variant="outline" disabled={!workspaceId || action.busy !== null} onClick={() => void runPreview(workspaceId)}>
              {action.busy === "preview" ? "Reading…" : "Read again"}
            </Button>
          </Stack>
        )}
        {action.error ? (
          <Notice tone="danger" role="alert">
            {action.error}
          </Notice>
        ) : null}
        {preview ? (
          <Stack gap={3}>
            <Text tone="secondary">
              <InlineCode>{preview.path}</InlineCode>
            </Text>
            {preview.servers.length > 0 ? (
              <List>
                {preview.servers.map((entry) => {
                  const badge = statusBadges[entry.status];
                  return (
                    <ListRow
                      key={entry.original_name}
                      title={entry.config?.name ?? entry.original_name}
                      subtitle={[entryDetail(entry), droppedText(entry), ...entry.warnings].filter(Boolean).join(" · ")}
                      meta={[
                        <Badge key="status" tone={badge.tone} size="sm">
                          {badge.label}
                        </Badge>,
                      ]}
                    />
                  );
                })}
              </List>
            ) : (
              <Text tone="muted">The file lists no servers.</Text>
            )}
          </Stack>
        ) : null}
        <Stack direction="row" gap={2} justify="end">
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            icon={<UploadIcon />}
            disabled={!preview || importable === 0 || action.busy !== null}
            onClick={() => void runImport()}
          >
            {action.busy === "import"
              ? "Importing…"
              : !preview
                ? "Import"
                : importable === 1
                  ? "Import 1 server"
                  : `Import ${importable} servers`}
          </Button>
        </Stack>
      </Stack>
    </Dialog>
  );
}
