import {
  DataTable,
  Notice,
  SectionHeader,
  type DataTableColumn,
} from "@aidex/ui/components";
import { Button, Stack, Text } from "@aidex/ui/primitives";
import type { AuditMetadata } from "./auditMetadata";
import { useSecurityAudit } from "./useSecurityAudit";
const columns: DataTableColumn<AuditMetadata>[] = [
  { key: "cursor", header: "Cursor" },
  {
    key: "time",
    header: "Time",
    render: (row) => new Date(row.time).toLocaleString(),
  },
  { key: "type", header: "Recorded event", rowHeader: true },
  { key: "workspace", header: "Workspace" },
  { key: "session", header: "Session" },
  { key: "result", header: "Result" },
];
export function AuditPanel() {
  const audit = useSecurityAudit();
  return (
    <Stack gap={4}>
      <SectionHeader title="Recorded security audit metadata" />
      <Text tone="muted">
        {audit.streamState} ·{" "}
        {audit.tail
          ? "Recent retained events plus live updates"
          : `Browsing after cursor ${audit.start}`}
      </Text>
      {audit.page.error && (
        <Notice tone="danger">
          Cannot read audit metadata ({audit.page.error.kind}). Refresh the
          retained range or reconnect Core.
        </Notice>
      )}
      <Text tone="muted">
        This view includes only recorded event types, timestamps, scope IDs, and
        closed result categories. Commands, messages, secret names, and event
        bodies are omitted. Credential writes and deletions appear as opaque
        record events.
      </Text>
      <DataTable
        caption="Value-free audit metadata"
        columns={columns}
        rows={audit.rows}
        rowKey={(row) => row.cursor}
        emptyText={
          audit.page.loading
            ? "Reading retained audit metadata…"
            : "No recorded events in this range."
        }
      />
      <Stack direction="row" gap={2} wrap>
        <Button
          variant="outline"
          disabled={!audit.canPrevious || audit.page.loading}
          onClick={audit.previous}
        >
          Previous viewed page
        </Button>
        <Button
          variant="outline"
          disabled={!audit.page.data?.hasMore || audit.page.loading}
          onClick={audit.next}
        >
          Next retained page
        </Button>
        <Button variant="ghost" onClick={audit.beginning}>
          Browse retained beginning
        </Button>
        <Button variant="ghost" onClick={audit.recent}>
          Recent and live
        </Button>
        <Button
          variant="ghost"
          disabled={!audit.rows.length}
          onClick={() => {
            const url = URL.createObjectURL(
              new Blob([JSON.stringify(audit.rows, null, 2)], {
                type: "application/json",
              }),
            );
            const link = document.createElement("a");
            link.href = url;
            link.download = "aidex-audit-metadata.json";
            link.click();
            setTimeout(() => URL.revokeObjectURL(url), 0);
          }}
        >
          Export displayed metadata
        </Button>
      </Stack>
    </Stack>
  );
}
