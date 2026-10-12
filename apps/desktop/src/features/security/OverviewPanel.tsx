import { useState } from "react";
import { DataTable, List, ListRow, MediaItem, SectionHeader, StatTile, type DataTableColumn } from "@aidex/ui/components";
import { BotIcon, ClipboardCheckIcon, DownloadIcon, KeyIcon, LockIcon, ShieldCheckIcon, ShieldIcon } from "@aidex/ui/icons";
import { Button, Card, Grid, IconBadge, Stack, StatusIndicator, Text } from "@aidex/ui/primitives";
import { useToast } from "../../app/providers/toast";
import {
  isolationLabels,
  policyMode,
  sampleAuditLog,
  samplePermissions,
  sampleSecrets,
  type ActivePermission,
  type AuditEntry,
  type AuditResult,
  type IsolationLevel,
} from "./sample";

type Props = { pendingCount: number; isolation: IsolationLevel };

const toneByResult: Record<AuditResult, "success" | "warning" | "neutral"> = {
  Allowed: "success",
  Approved: "success",
  Denied: "warning",
  Sandboxed: "neutral",
  "Awaiting approval": "neutral",
};

const auditColumns: readonly DataTableColumn<AuditEntry>[] = [
  { key: "time", header: "Time", width: "96px" },
  { key: "actor", header: "Actor" },
  { key: "action", header: "Action", rowHeader: true },
  { key: "target", header: "Target" },
  { key: "result", header: "Result", render: (row) => <StatusIndicator tone={toneByResult[row.result]} label={row.result} /> },
];

/** Security dashboard: headline posture, active agent permissions, and the recent audit log. */
export function OverviewPanel({ pendingCount, isolation }: Props) {
  const [permissions, setPermissions] = useState<readonly ActivePermission[]>(samplePermissions);
  const notify = useToast();
  const sandbox = isolationLabels[isolation];

  const revoke = (permission: ActivePermission) => {
    setPermissions((current) => current.filter((item) => item.id !== permission.id));
    notify(`Preview: removed "${permission.permission}" for ${permission.agent} here. AiDEX Core will enforce revocations once connected.`);
  };

  return (
    <Stack gap={10}>
      <Grid columns={4} gap={4}>
        <StatTile label="Policy mode" value={policyMode.value} hint={policyMode.hint} icon={<ShieldIcon />} tone="accent" />
        <StatTile
          label="Pending approvals"
          value={pendingCount}
          hint={pendingCount > 0 ? "Waiting for your decision" : "Nothing waiting"}
          icon={<ClipboardCheckIcon />}
          tone={pendingCount > 0 ? "warning" : "success"}
        />
        <StatTile label="Secrets in vault" value={sampleSecrets.length} hint="Unlocked for this session" icon={<KeyIcon />} />
        <StatTile
          label="Sandbox"
          value={sandbox.value}
          hint={sandbox.hint}
          icon={<LockIcon />}
          tone={isolation === "direct" ? "danger" : "success"}
        />
      </Grid>

      <Card padding="md">
        <MediaItem
          media={<IconBadge tone="accent" icon={<ShieldCheckIcon />} />}
          title="Sensitive actions pass through the execution broker"
          description="Shell commands, file writes, network requests, package installs, and secret use reach the broker as typed requests. Policy then allows, denies, sandboxes, or asks you to approve. Repository instructions, tool output, and web content are treated as untrusted."
        />
      </Card>

      <section>
        <Stack gap={4}>
          <SectionHeader title="Active permissions" meta={`${permissions.length} granted`} />
          {permissions.length === 0 ? (
            <Text tone="secondary">No agent holds an extra permission. New requests will go to Approvals.</Text>
          ) : (
            <List metaWidth="96px">
              {permissions.map((permission) => (
                <ListRow
                  key={permission.id}
                  media={<IconBadge size="md" icon={<BotIcon />} />}
                  title={permission.agent}
                  subtitle={`${permission.permission} · ${permission.scope}`}
                  meta={[permission.granted]}
                  trailing={
                    <Button
                      variant="outline"
                      size="md"
                      aria-label={`Revoke ${permission.permission} for ${permission.agent}`}
                      onClick={() => revoke(permission)}
                    >
                      Revoke
                    </Button>
                  }
                />
              ))}
            </List>
          )}
        </Stack>
      </section>

      <section>
        <Stack gap={4}>
          <SectionHeader
            title="Recent audit log"
            meta="Today"
            actions={
              <Button
                variant="ghost"
                size="md"
                icon={<DownloadIcon />}
                onClick={() => notify("Preview: exporting the audit log will be available when AiDEX Core is connected.")}
              >
                Export
              </Button>
            }
          />
          <DataTable caption="Recent audit log" hideCaption columns={auditColumns} rows={sampleAuditLog} rowKey={(row) => row.id} />
        </Stack>
      </section>
    </Stack>
  );
}
