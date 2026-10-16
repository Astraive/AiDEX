import { DataTable, formatDateTime, Notice, SectionHeader, type DataTableColumn } from "@aidex/ui/components";
import { ChevronLeftIcon, ChevronRightIcon, DownloadIcon } from "@aidex/ui/icons";
import { Button, SegmentedControl, Stack, StatusIndicator, Text } from "@aidex/ui/primitives";
import { saveBlob } from "../chat/artifacts/artifactContent";
import { auditKindLabels, type AuditMetadata } from "./auditMetadata";
import { useSecurityAudit, type AuditStream } from "./useSecurityAudit";

const formatTime = (time: string): string => {
  const date = new Date(time);
  return Number.isNaN(date.getTime()) ? time : formatDateTime(date);
};

const columns: readonly DataTableColumn<AuditMetadata>[] = [
  { key: "time", header: "Time", width: "180px", render: (row) => formatTime(row.time) },
  { key: "type", header: "Event", rowHeader: true, render: (row) => auditKindLabels[row.type] },
  { key: "result", header: "Result" },
  { key: "workspace", header: "Workspace" },
  { key: "session", header: "Session" },
];

const streamIndicator: Record<AuditStream, { label: string; tone: "success" | "warning" | "neutral" }> = {
  connecting: { label: "Connecting", tone: "neutral" },
  live: { label: "Live", tone: "success" },
  reconnecting: { label: "Reconnecting", tone: "warning" },
  offline: { label: "Not following", tone: "neutral" },
};

const modeOptions = [
  { value: "recent", label: "Recent" },
  { value: "browse", label: "From the start" },
] as const;

/**
 * The security audit log: vault, secret handle, policy, approval, sandbox, process, emergency, and
 * update events as value-free rows (`auditMetadata`). Recent follows the log live; From the start pages
 * through what the drive keeps. Export saves exactly the rows shown.
 */
export function AuditPanel() {
  const audit = useSecurityAudit();
  const browsing = audit.mode === "browse";
  const indicator = streamIndicator[audit.stream];

  const exportRows = () =>
    saveBlob(
      new Blob([JSON.stringify(audit.rows, null, 2)], { type: "application/json" }),
      `aidex-audit-${new Date().toISOString().slice(0, 10)}.json`,
    );

  return (
    <section>
      <Stack gap={4}>
        <SectionHeader
          title="Audit log"
          meta={browsing ? undefined : <StatusIndicator tone={indicator.tone} label={indicator.label} />}
          actions={
            <Stack direction="row" gap={2} align="center">
              <SegmentedControl
                label="Audit range"
                size="sm"
                options={modeOptions}
                value={audit.mode}
                onValueChange={(next) => (next === "browse" ? audit.browse() : audit.recent())}
              />
              <Button variant="ghost" size="md" icon={<DownloadIcon />} disabled={!audit.rows.length} onClick={exportRows}>
                Export
              </Button>
            </Stack>
          }
        />
        {audit.gap ? (
          <Notice tone="warning">Some older events expired from the drive's log before they could be read.</Notice>
        ) : null}
        {audit.error ? (
          <Notice tone="danger">
            <Stack gap={3} align="start">
              <Text as="span">Cannot read the audit log: {audit.error.message}</Text>
              <Button variant="outline" size="md" onClick={audit.recent}>
                Show recent events
              </Button>
            </Stack>
          </Notice>
        ) : null}
        <DataTable
          caption="Audit log"
          hideCaption
          columns={columns}
          rows={audit.rows}
          rowKey={(row) => row.cursor}
          emptyText={audit.loading ? "Reading the audit log…" : browsing ? "No audited events on this page." : "No recent audited events."}
        />
        {browsing ? (
          <Stack direction="row" gap={3} align="center" justify="between">
            <Button
              variant="ghost"
              size="md"
              icon={<ChevronLeftIcon />}
              disabled={audit.loading || !audit.canOlder}
              onClick={audit.older}
            >
              Older
            </Button>
            <Button
              variant="ghost"
              size="md"
              trailing={<ChevronRightIcon />}
              disabled={audit.loading || !audit.canNewer}
              onClick={audit.newer}
            >
              Newer
            </Button>
          </Stack>
        ) : null}
        <Text tone="secondary">
          Rows hold only the event type, time, scope, and a fixed result. Commands, messages, reasons, secret names, and
          consumers are never shown or exported.
        </Text>
      </Stack>
    </section>
  );
}
