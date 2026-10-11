import {
  DataTable,
  KeyValueList,
  Notice,
  SectionHeader,
  type DataTableColumn,
} from "@aidex/ui/components";
import { Button, Stack, StatusIndicator, Text } from "@aidex/ui/primitives";
import type { Capability } from "@aidex/client/dto/sandbox";
import {
  useHostProfile,
  useSandboxCapabilities,
} from "../../hooks/useCoreReports";
import {
  enforcementDetail,
  enforcementLabel,
  enforcementTone,
  levelLabel,
  requirementLabels,
} from "../system/labels";
import { PolicyRules } from "./PolicyRules";

const columns: DataTableColumn<Capability>[] = [
  {
    key: "requirement",
    header: "Protection",
    rowHeader: true,
    render: (row) => requirementLabels[row.requirement],
  },
  {
    key: "state",
    header: "Reported enforcement",
    render: (row) => (
      <StatusIndicator
        tone={enforcementTone(row.enforcement)}
        label={enforcementLabel(row.enforcement)}
      />
    ),
  },
  {
    key: "mechanism",
    header: "Mechanism",
    render: (row) => enforcementDetail(row.enforcement),
  },
  {
    key: "limitations",
    header: "Known limits",
    render: (row) => row.limitations?.join(" ") || "None reported",
  },
];
export function SandboxPanel() {
  const sandbox = useSandboxCapabilities();
  const host = useHostProfile();
  const report = sandbox.data;
  const fsRead =
    report?.capabilities.find(
      (capability) => capability.requirement === "filesystem_read_confinement",
    )?.enforcement.state === "enforced";
  const fsWrite =
    report?.capabilities.find(
      (capability) => capability.requirement === "filesystem_write_confinement",
    )?.enforcement.state === "enforced";
  const network =
    report?.capabilities.find(
      (capability) => capability.requirement === "network_deny",
    )?.enforcement.state === "enforced";
  return (
    <Stack gap={5}>
      <SectionHeader
        title="This host's actual capabilities"
        actions={
          <Button
            variant="ghost"
            onClick={() => {
              sandbox.refetch();
              host.refetch();
            }}
          >
            Probe again
          </Button>
        }
      />
      {(sandbox.error || host.error) && (
        <Notice tone="danger">
          {sandbox.error?.message ?? host.error?.message}
        </Notice>
      )}
      {host.data && (
        <KeyValueList
          items={[
            {
              label: "Platform",
              value: `${host.data.os} · ${host.data.architecture}`,
            },
            {
              label: "CPU threads",
              value: host.data.logical_cpu_count ?? "Unknown",
            },
            { label: "Host network probe", value: host.data.network.state },
          ]}
        />
      )}
      {report ? (
        <>
          <KeyValueList
            items={[
              { label: "Backend", value: report.backend },
              {
                label: "Maximum enforceable baseline",
                value: report.max_level ? levelLabel(report.max_level) : "None",
              },
              {
                label: "Managed-process helper",
                value: `${report.helper.state} · ${report.helper.path}`,
              },
            ]}
          />
          {report.helper.state === "missing" && (
            <Notice tone="warning">
              Managed process launch is unavailable: {report.helper.reason}
            </Notice>
          )}
          {(!fsRead || !fsWrite) && (
            <Notice tone="warning">
              Native process filesystem confinement is not verified. Broker
              filesystem tools still use their selected workspace root; a
              launched full-access process can act outside it.
            </Notice>
          )}
          {!network && (
            <Notice tone="warning">
              Native network denial is not verified by this report. The
              application's offline gate is a separate control.
            </Notice>
          )}
          <DataTable
            caption="Actual sandbox enforcement"
            columns={columns}
            rows={report.capabilities}
            rowKey={(row) => row.requirement}
          />
          {!!report.facts?.length && (
            <Text tone="muted">Probe facts: {report.facts.join(", ")}</Text>
          )}
        </>
      ) : (
        <Text tone="muted">
          {sandbox.loading
            ? "Probing this host…"
            : "Capability report unavailable."}
        </Text>
      )}
      <Notice>
        Capability reports describe what the host can enforce, rather than the
        active policy of every process. External runtimes currently require
        explicit full access and own their tools. Requested unsupported
        protection is refused by Core.
      </Notice>
      <Button
        variant="outline"
        disabled
        title="There is no standalone sandbox-level mutation RPC"
      >
        Change global sandbox level · unavailable
      </Button>
      <PolicyRules />
    </Stack>
  );
}
