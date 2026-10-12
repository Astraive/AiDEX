import { DataTable, KeyValueList, List, ListRow, Notice, SectionHeader, type DataTableColumn } from "@aidex/ui/components";
import { AlertTriangleIcon, LaptopIcon, MonitorIcon, RefreshIcon, ServerIcon } from "@aidex/ui/icons";
import { Badge, Button, ChoiceCardGroup, Stack, StatusIndicator, Text, type ChoiceCardOption } from "@aidex/ui/primitives";
import { rankOf, type BackendCapabilities, type Capability } from "@aidex/client/dto/sandbox";
import { useToast } from "../../app/providers/toast";
import { useSandboxCapabilities } from "../../hooks/useCoreReports";
import { enforcementDetail, enforcementLabel, enforcementTone, levelLabel, requirementLabels } from "../system/labels";
import { PolicyRules } from "./PolicyRules";
import { sandboxBackends, type IsolationLevel } from "./sample";

type Props = { isolation: IsolationLevel; onIsolationChange: (level: IsolationLevel) => void };

const levelOptions: readonly { value: IsolationLevel; title: string; description: string }[] = [
  {
    value: "direct",
    title: "Direct",
    description: "Runs as your user account with no confinement, only with explicit consent. For trusted, one-off work.",
  },
  {
    value: "brokered",
    title: "Brokered",
    description: "Sensitive actions go through the broker; processes get a cleared environment, a killable process tree, and limits.",
  },
  {
    value: "restricted",
    title: "Restricted",
    description: "Adds native OS confinement of file reads and writes to the workspace mounts.",
  },
  {
    value: "ephemeral",
    title: "Ephemeral",
    description: "A disposable isolated environment. No backend implements it yet, so requests are refused.",
  },
];

const backendIcons = { windows_job_object: MonitorIcon, linux_landlock: ServerIcon, macos_seatbelt: LaptopIcon, unsupported: ServerIcon } as const;

const columns: readonly DataTableColumn<Capability>[] = [
  { key: "requirement", header: "Protection", rowHeader: true, render: (row) => requirementLabels[row.requirement] },
  {
    key: "state",
    header: "Status",
    width: "150px",
    render: (row) => <StatusIndicator tone={enforcementTone(row.enforcement)} label={enforcementLabel(row.enforcement)} />,
  },
  { key: "detail", header: "How", render: (row) => enforcementDetail(row.enforcement) },
  { key: "limitations", header: "Known gaps", render: (row) => row.limitations?.join(" ") ?? "None stated" },
];

function levelChoices(capabilities: BackendCapabilities | undefined): readonly ChoiceCardOption<IsolationLevel>[] {
  const maxRank = capabilities?.max_level ? rankOf(capabilities.max_level) : -1;
  return levelOptions.map((option) => {
    const enforceable = capabilities ? rankOf(option.value) <= maxRank : true;
    return {
      ...option,
      title: levelLabel(option.value),
      disabled: !enforceable,
      meta:
        option.value === "direct" ? (
          <Badge tone="neutral" icon={<AlertTriangleIcon size={14} />}>
            Not recommended
          </Badge>
        ) : !enforceable ? (
          <Badge tone="neutral">Not enforceable here</Badge>
        ) : option.value === capabilities?.max_level ? (
          <Badge tone="accent">Strongest here</Badge>
        ) : undefined,
    };
  });
}

/** Sandbox and policies: the live capability report of this host, the level choice, and the sample policy rules. */
export function SandboxPanel({ isolation, onIsolationChange }: Props) {
  const notify = useToast();
  const sandbox = useSandboxCapabilities();
  const report = sandbox.data;

  const changeIsolation = (level: IsolationLevel) => {
    onIsolationChange(level);
    notify(
      level === "direct"
        ? "Preview: Direct runs commands as your user without confinement. The choice is not saved until the policy service is connected."
        : `Preview: ${levelLabel(level)} selected for this session only; the policy service will apply it once connected.`,
      level === "direct" ? "danger" : "info",
    );
  };

  return (
    <Stack gap={10}>
      <section>
        <Stack gap={4}>
          <SectionHeader
            title="This computer"
            meta={report ? `${report.backend.replace(/_/g, " ")} · capability report v${report.version}` : undefined}
            actions={
              <Button variant="ghost" size="md" icon={<RefreshIcon />} onClick={sandbox.refetch}>
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
              <KeyValueList
                items={[
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
            </>
          ) : sandbox.loading ? (
            <Text tone="secondary">Probing the sandbox backend…</Text>
          ) : null}
        </Stack>
      </section>

      <section>
        <Stack gap={4}>
          <SectionHeader title="Isolation level" meta="Session preview" />
          <ChoiceCardGroup
            label="Isolation level"
            labelVisibility="hidden"
            columns={2}
            options={levelChoices(report)}
            value={isolation}
            onValueChange={changeIsolation}
          />
          <Text tone="secondary">
            Isolation fails closed. If this computer cannot enforce a requested level, the broker refuses the action instead of
            running it with weaker isolation.
          </Text>
        </Stack>
      </section>

      <section>
        <Stack gap={4}>
          <SectionHeader title="Platform backends" />
          <List>
            {sandboxBackends.map((backend) => {
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

      <PolicyRules />
    </Stack>
  );
}
