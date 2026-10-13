import { DataTable, KeyValueList, List, ListRow, Notice, SectionHeader, type DataTableColumn } from "@aidex/ui/components";
import { LaptopIcon, MonitorIcon, RefreshIcon, ServerIcon } from "@aidex/ui/icons";
import { Button, Stack, StatusIndicator, Text } from "@aidex/ui/primitives";
import type { BackendKind, Capability, Requirement } from "@aidex/client/dto/sandbox";
import type { ExecutionStatus } from "@aidex/client/dto/execution";
import { useSandboxCapabilities } from "../../hooks/useCoreReports";
import { enforcementDetail, enforcementLabel, enforcementTone, levelLabel, osLabel, requirementLabels } from "../system/labels";
import { PolicyRules } from "./PolicyRules";

type Props = { execution: ExecutionStatus | undefined };

type PlatformBackend = { id: Exclude<BackendKind, "unsupported">; platform: string; mechanism: string; detail: string };

/** How each OS backend isolates processes (the enforcement matrix in crates/sandbox/README.md). */
const platformBackends: readonly PlatformBackend[] = [
  {
    id: "windows_job_object",
    platform: "Windows",
    mechanism: "Job Objects",
    detail:
      "Each process tree runs in a Job Object with a cleared environment; process, memory, and CPU limits are watchdog-enforced. No filesystem or network confinement yet, so the strongest level is Brokered.",
  },
  {
    id: "linux_landlock",
    platform: "Linux",
    mechanism: "Landlock + namespaces",
    detail: "Landlock filesystem rules (Linux 6.2+), rlimits, and private user and network namespaces where the kernel allows; up to Restricted.",
  },
  {
    id: "macos_seatbelt",
    platform: "macOS",
    mechanism: "Seatbelt",
    detail: "A sandbox-exec profile that denies writes outside the mounts and network when requested, plus rlimits; up to Brokered.",
  },
];

const backendIcons = { windows_job_object: MonitorIcon, linux_landlock: ServerIcon, macos_seatbelt: LaptopIcon } as const;

const columns: readonly DataTableColumn<Capability>[] = [
  { key: "requirement", header: "Protection", rowHeader: true, render: (row) => requirementLabels[row.requirement] },
  {
    key: "state",
    header: "Status",
    width: "150px",
    render: (row) => <StatusIndicator tone={enforcementTone(row.enforcement)} label={enforcementLabel(row.enforcement)} />,
  },
  { key: "detail", header: "How", render: (row) => enforcementDetail(row.enforcement) },
  { key: "limitations", header: "Known gaps", render: (row) => row.limitations?.join(" ") || "None stated" },
];

const enforced = (capabilities: readonly Capability[], requirement: Requirement): boolean =>
  capabilities.find((capability) => capability.requirement === requirement)?.enforcement.state === "enforced";

/**
 * Sandbox and policies: the live capability report of this computer (what it can enforce, not the
 * scope of every process), warnings for the protections it lacks, the platform backends, and the
 * policy controls that exist.
 */
export function SandboxPanel({ execution }: Props) {
  const sandbox = useSandboxCapabilities();
  const report = sandbox.data;
  const filesystemConfined =
    report &&
    enforced(report.capabilities, "filesystem_read_confinement") &&
    enforced(report.capabilities, "filesystem_write_confinement");
  const networkDenied = report && enforced(report.capabilities, "network_deny");

  return (
    <Stack gap={10}>
      <section>
        <Stack gap={4}>
          <SectionHeader
            title="This computer"
            meta={report ? `${report.backend.replace(/_/g, " ")} · capability report v${report.version}` : undefined}
            actions={
              <Button variant="ghost" size="md" icon={<RefreshIcon />} disabled={sandbox.loading} onClick={sandbox.refetch}>
                Probe again
              </Button>
            }
          />
          {sandbox.error ? <Notice tone="warning">Cannot read sandbox capabilities: {sandbox.error.message}</Notice> : null}
          {report ? (
            <>
              {report.helper.state === "missing" ? (
                <Notice tone="warning">
                  The sandbox helper is missing ({report.helper.reason}), so no managed process can start. It ships next to aidexd.
                </Notice>
              ) : null}
              {!filesystemConfined ? (
                <Notice tone="warning">
                  Commands are not confined to the project's files on this computer. File tools still stay inside the selected
                  project, but a process running with Full access can reach the rest of the drive and computer.
                </Notice>
              ) : null}
              {!networkDenied ? (
                <Notice tone="warning">
                  Processes are not kept off the network on this computer. Offline mode (Overview) stops AiDEX's own network
                  requests, not those of a process.
                </Notice>
              ) : null}
              <KeyValueList
                items={[
                  { label: "System", value: osLabel(report.os) },
                  { label: "Strongest level", value: report.max_level ? levelLabel(report.max_level) : "None (nothing can launch)" },
                  { label: "Helper", value: report.helper.state === "available" ? report.helper.path : `Missing: ${report.helper.path}` },
                  ...(report.facts?.length ? [{ label: "Probe facts", value: report.facts.join(", ") }] : []),
                ]}
              />
              <DataTable
                caption="Sandbox protections on this computer"
                hideCaption
                columns={columns}
                rows={report.capabilities}
                rowKey={(row) => row.requirement}
              />
              <Text tone="secondary">
                This is what the computer can enforce, not the protection every process has. Isolation fails closed: if a
                requested level cannot be enforced, the broker refuses the action instead of running it with weaker isolation.
                External runtimes run with Full access and use their own tools.
              </Text>
            </>
          ) : sandbox.loading ? (
            <Text tone="secondary">Probing the sandbox backend…</Text>
          ) : null}
        </Stack>
      </section>

      <section>
        <Stack gap={4}>
          <SectionHeader title="Platform backends" />
          <List>
            {platformBackends.map((backend) => {
              const Icon = backendIcons[backend.id];
              const isHost = backend.id === report?.backend;
              return (
                <ListRow
                  key={backend.id}
                  media={<Icon />}
                  title={`${backend.platform}: ${backend.mechanism}`}
                  subtitle={backend.detail}
                  trailing={<StatusIndicator tone={isHost ? "success" : "neutral"} label={isHost ? "This computer" : "Other platform"} />}
                />
              );
            })}
          </List>
        </Stack>
      </section>

      <PolicyRules execution={execution} />
    </Stack>
  );
}
