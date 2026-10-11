import { useEffect, useState } from "react";
import {
  List,
  ListRow,
  Notice,
  SectionHeader,
  StatTile,
} from "@aidex/ui/components";
import { Button, Grid, Stack, Text } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import { useRpc } from "../../hooks/useRpc";
import { useSandboxCapabilities } from "../../hooks/useCoreReports";
import { useNavigation } from "../../app/providers/navigation";
import { levelLabel } from "../system/labels";
import { CoreStatusNotice } from "../system/CoreStatusNotice";
import { AuditPanel } from "./AuditPanel";

export function OverviewPanel({
  pendingCount,
}: {
  pendingCount: number | null;
}) {
  const { navigate } = useNavigation();
  const vault = useRpc("vault.status");
  const execution = useRpc("execution.status");
  const network = useRpc("network.status");
  const sandbox = useSandboxCapabilities();
  const [error, setError] = useState<string>();
  const refresh = () => {
    vault.refetch();
    execution.refetch();
    network.refetch();
  };
  useEffect(() => {
    const timer = setInterval(refresh, 3000);
    window.addEventListener("aidex:security-changed", refresh);
    return () => {
      clearInterval(timer);
      window.removeEventListener("aidex:security-changed", refresh);
    };
  }, [vault.refetch, execution.refetch, network.refetch]);
  const active =
    execution.data?.runs.filter((run) =>
      ["pending", "running", "waiting_approval"].includes(run.status),
    ) ?? [];
  return (
    <Stack gap={5}>
      <CoreStatusNotice requires="daemon" />
      {(error ||
        vault.error ||
        execution.error ||
        network.error ||
        sandbox.error) && (
        <Notice tone="danger">
          {error ??
            vault.error?.message ??
            execution.error?.message ??
            network.error?.message ??
            sandbox.error?.message}
        </Notice>
      )}
      <Grid columns={4} gap={3}>
        <StatTile
          label="Vault state"
          value={vault.data?.state ?? "Unavailable"}
          hint={
            vault.data?.secrets !== undefined
              ? `${vault.data.secrets} encrypted records`
              : "Counts require an unlocked vault"
          }
        />
        <StatTile
          label="Pending approvals"
          value={pendingCount ?? "Unknown"}
          hint="Filtered Core total"
        />
        <StatTile
          label="Network gate"
          value={
            network.data
              ? network.data.offline
                ? "Offline"
                : "Online"
              : "Unknown"
          }
          hint="Application gate; see native network enforcement separately"
        />
        <StatTile
          label="Host maximum baseline"
          value={
            sandbox.data?.max_level
              ? levelLabel(sandbox.data.max_level)
              : "Unavailable"
          }
          hint="Capability report, not every process's active scope"
        />
      </Grid>
      <SectionHeader
        title="Active execution recorded by Core"
        meta={
          execution.data
            ? `${active.length} displayed active runs`
            : "Unavailable"
        }
      />
      <List>
        {active.map((run) => (
          <ListRow
            key={run.id}
            title={run.id}
            subtitle={`Session ${run.session_id} · ${run.status}`}
            trailing={
              <Stack direction="row" gap={2}>
                <Button
                  variant="ghost"
                  onClick={() =>
                    navigate({ page: "execution", id: run.session_id })
                  }
                >
                  Inspect execution
                </Button>
                <Button
                  variant="outline"
                  onClick={() =>
                    void call("run.cancel", { id: run.id })
                      .then(refresh)
                      .catch((caught) => setError(toAidexError(caught).message))
                  }
                >
                  Cancel run
                </Button>
              </Stack>
            }
          />
        ))}
      </List>
      {execution.data && !active.length && (
        <Text tone="muted">
          No active runs in the current bounded execution summary.
        </Text>
      )}
      <Text tone="muted">
        Permissions belong to each conversation's configuration and brokered
        request. This screen does not infer session grants or simulate
        revocation.
      </Text>
      <AuditPanel />
    </Stack>
  );
}
