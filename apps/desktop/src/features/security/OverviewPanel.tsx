import { useState } from "react";
import { List, ListRow, MediaItem, Notice, SectionHeader, SettingRow, StatTile } from "@aidex/ui/components";
import { BotIcon, ClipboardCheckIcon, GlobeIcon, KeyIcon, LockIcon, ShieldCheckIcon, StopIcon } from "@aidex/ui/icons";
import { Button, Card, Grid, IconBadge, Stack, StatusIndicator, Switch, Text } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import type { RunSummary } from "@aidex/client/dto/execution";
import type { RunStatus } from "@aidex/client/dto/sessions";
import { useNavigation } from "../../app/providers/navigation";
import { CoreStatusNotice } from "../system/CoreStatusNotice";
import { levelLabel } from "../system/labels";
import { AuditPanel } from "./AuditPanel";
import type { SecurityTab } from "./tabs";
import { activeRun, lockedByVault, type SecurityStatus } from "./useSecurityStatus";

type Props = { status: SecurityStatus; onOpenTab: (tab: SecurityTab) => void };

const OFFLINE_ID = "security-offline";

const runStatusLabels: Partial<Record<RunStatus, string>> = {
  pending: "Starting",
  running: "Running",
  waiting_approval: "Waiting for approval",
};

const vaultLabels = { unlocked: "Unlocked", locked: "Locked", missing: "Not set up", unavailable: "Unavailable" } as const;

/**
 * Security dashboard from live Core data: vault, pending approvals, the offline switch, and this
 * computer's strongest sandbox level; the runs still active, with Inspect and Cancel; and the audit log.
 */
export function OverviewPanel({ status, onOpenTab }: Props) {
  const { navigate } = useNavigation();
  const [busy, setBusy] = useState<string>();
  const [error, setError] = useState<string>();
  const vault = status.vault.data;
  const pending = status.pending.data?.total_approvals;
  const network = status.network.data;
  const execution = status.execution.data;
  const maxLevel = execution?.capabilities.sandbox.max_level;
  const active = execution?.runs.filter(activeRun) ?? [];
  // `execution.status` lists at most 100 conversations; a run of one beyond them is named by its id.
  const titles = new Map(execution?.sessions.map((session) => [session.id, session.title || "Untitled conversation"]) ?? []);

  const act = async (key: string, perform: () => Promise<unknown>) => {
    setBusy(key);
    setError(undefined);
    try {
      await perform();
    } catch (caught) {
      setError(toAidexError(caught).message);
    } finally {
      setBusy(undefined);
      status.refresh();
    }
  };

  // A locked vault hides approval requests; that is the vault tile's state, not a failure.
  const pendingLocked = lockedByVault(status.pending.error);
  const loadError =
    status.vault.error ?? (pendingLocked ? null : status.pending.error) ?? status.network.error ?? status.execution.error;

  return (
    <Stack gap={10}>
      <CoreStatusNotice requires="daemon" />
      {loadError ? <Notice tone="danger">Some security status could not be read: {loadError.message}</Notice> : null}
      {error ? (
        <Notice tone="danger" role="alert">
          {error}
        </Notice>
      ) : null}

      <Grid columns={4} gap={4}>
        <StatTile
          label="Vault"
          value={vault ? vaultLabels[vault.state] : "Unknown"}
          hint={vault?.secrets !== undefined ? `${vault.secrets} secrets stored` : "Counts need an unlocked vault"}
          icon={<KeyIcon />}
          tone={vault?.state === "unlocked" ? "success" : "warning"}
        />
        <StatTile
          label="Pending approvals"
          value={pendingLocked ? "Locked" : (pending ?? "Unknown")}
          hint={
            pendingLocked
              ? "Unlock the vault to read requests"
              : pending
                ? "Waiting for your decision"
                : pending === 0
                  ? "Nothing waiting"
                  : undefined
          }
          icon={<ClipboardCheckIcon />}
          tone={pendingLocked || pending === undefined ? "default" : pending ? "warning" : "success"}
        />
        <StatTile
          label="Network"
          value={network ? (network.offline ? "Offline" : "Online") : "Unknown"}
          hint={network?.offline ? "AiDEX refuses network use" : "AiDEX may use the network"}
          icon={<GlobeIcon />}
          tone={network?.offline ? "accent" : "default"}
        />
        <StatTile
          label="Sandbox"
          value={maxLevel ? levelLabel(maxLevel) : execution ? "None" : "Unknown"}
          hint="Strongest level this computer enforces"
          icon={<LockIcon />}
          tone={maxLevel ? "success" : "warning"}
        />
      </Grid>

      <Card padding="md">
        <SettingRow
          title="Offline mode"
          description="While on, AiDEX refuses provider, web search, and extension network requests. It does not confine processes that run with Full access."
          controlId={OFFLINE_ID}
          control={
            <Switch
              id={OFFLINE_ID}
              checked={network?.offline ?? false}
              disabled={!network || busy !== undefined}
              onCheckedChange={(enabled) => void act("offline", () => call("network.offline", { enabled }))}
            />
          }
        />
      </Card>

      <Card padding="md">
        <MediaItem
          media={<IconBadge tone="accent" icon={<ShieldCheckIcon />} />}
          title="Sensitive actions pass through the execution broker"
          description="Shell commands, file writes, network requests, package installs, and secret use reach the broker as typed requests. Policy then allows, denies, sandboxes, or asks you to approve. Repository instructions, tool output, and web content are treated as untrusted."
          trailing={
            pending ? (
              <Button variant="outline" size="md" onClick={() => onOpenTab("approvals")}>
                Review approvals
              </Button>
            ) : undefined
          }
        />
      </Card>

      <section>
        <Stack gap={4}>
          <SectionHeader title="Active runs" meta={execution ? `${active.length} active` : undefined} />
          {!execution ? (
            <Text tone="secondary">{status.execution.loading ? "Reading runs…" : "Runs are unavailable."}</Text>
          ) : active.length === 0 ? (
            <Text tone="secondary">No run is active. Permissions belong to each conversation; nothing is granted here.</Text>
          ) : (
            <List>
              {active.map((run: RunSummary) => (
                <ListRow
                  key={run.id}
                  media={<IconBadge size="md" icon={<BotIcon />} />}
                  title={titles.get(run.session_id) ?? `Conversation ${run.session_id}`}
                  subtitle={`Run ${run.id}`}
                  trailing={
                    <Stack direction="row" gap={3} align="center">
                      <StatusIndicator
                        tone={run.status === "waiting_approval" ? "warning" : "success"}
                        label={runStatusLabels[run.status] ?? run.status}
                      />
                      <Button variant="ghost" size="md" onClick={() => navigate({ page: "execution", id: run.session_id })}>
                        Inspect
                      </Button>
                      <Button
                        variant="outline"
                        size="md"
                        icon={<StopIcon />}
                        disabled={busy !== undefined}
                        aria-label={`Cancel run ${run.id}`}
                        onClick={() => void act(run.id, () => call("run.cancel", { id: run.id }))}
                      >
                        {busy === run.id ? "Cancelling…" : "Cancel"}
                      </Button>
                    </Stack>
                  }
                />
              ))}
            </List>
          )}
          {execution && (execution.runs_truncated || execution.total_runs > execution.runs.length) ? (
            <Text tone="muted">Only the most recent runs fit in one status read; older active runs may be missing.</Text>
          ) : null}
        </Stack>
      </section>

      <AuditPanel />
    </Stack>
  );
}
