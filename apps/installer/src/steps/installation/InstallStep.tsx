import { useState } from "react";
import { ActivityLog, KeyValueList, Notice, PageHeader, SectionHeader, StepList, type Step } from "@aidex/ui/components";
import { Button, Heading, ProgressBar, Stack, Text } from "@aidex/ui/primitives";
import type { VolumeInfo } from "@aidex/client/dto/drive";
import type { InitReport } from "@aidex/client/dto/installer";
import type { AidexError } from "@aidex/client/errors";
import { formatBytes } from "@aidex/client/format";
import { estimateGb, fitsOnDrive, formatGb, type SetupState } from "../../lib/setup";
import { stepKicker } from "../../lib/steps";
import { REAL_TASK_COUNT, type InstallTask, type RunPhase, type TaskStatus } from "../../lib/useInstallRun";
import type { StepProps } from "../types";

type Props = Omit<StepProps, "update" | "notify"> & {
  phase: RunPhase;
  tasks: readonly InstallTask[];
  completed: number;
  report: InitReport | null;
  error: AidexError | null;
  passwordAccepted: boolean;
};

const PRESET_LABEL = { lite: "Lite", developer: "Developer", custom: "Custom" } as const;
const MODEL_LABEL = { gemma: "Gemma 4", embedding: "Embeddings", custom: "Custom GGUF" } as const;

const STEP_STATUS: Record<TaskStatus, Step["status"]> = {
  pending: "pending",
  active: "active",
  done: "done",
  failed: "failed",
  unavailable: "pending",
  skipped: "pending",
};

function summary(state: SetupState, volume: VolumeInfo | null, passwordAccepted: boolean) {
  const tools = [...state.agents.map((agent) => (agent === "claude" ? "claude code" : agent)), ...state.extensions];
  const vault =
    state.vault === "none" ? "No password (unencrypted)" : passwordAccepted ? "Password-protected (set on first launch)" : "Password not set yet";
  return [
    { label: "Install type", value: state.installMode === "normal" ? "Standard · per-user" : "Portable · removable drive" },
    { label: "Install location", value: state.driveRoot ?? "None chosen" },
    { label: "Free space", value: formatBytes(volume?.free_bytes) },
    { label: "Installation", value: PRESET_LABEL[state.preset] },
    { label: "Models", value: state.models.map((model) => MODEL_LABEL[model]).join(", ") || "None (cloud-ready)" },
    { label: "Agents & extras", value: tools.length ? tools.join(", ") : "AiDEX Native only" },
    { label: "Vault", value: vault },
  ];
}

function steps(tasks: readonly InstallTask[]): Step[] {
  return tasks.map((task) => ({
    title: task.status === "unavailable" ? `${task.title} (not available yet)` : task.status === "skipped" ? `${task.title} (skipped)` : task.title,
    description: task.detail,
    status: STEP_STATUS[task.status],
  }));
}

/** Step 6: review, then prepare the selected target. Unavailable components are never simulated. */
export function InstallStep({ state, titleRef, volume, phase, tasks, completed, report, error, passwordAccepted }: Props) {
  const [showDetails, setShowDetails] = useState(false);
  const percent = Math.round((completed / REAL_TASK_COUNT) * 100);

  if (phase === "review") {
    return (
      <>
        <PageHeader
          eyebrow={stepKicker(5)}
          title="Ready to prepare AiDEX"
          description={`Confirm the ${state.installMode === "normal" ? "standard per-user target" : "portable drive"} before setup.`}
          titleRef={titleRef}
        />
        <Stack gap={3}>
          <SectionHeader title="Your setup" level={3} meta={`About ${formatGb(estimateGb(state))} once every component is available`} />
          <KeyValueList items={summary(state, volume, passwordAccepted)} />
        </Stack>
        <Notice>
          Setup creates AiDEX folders and an aidex-drive.json manifest at {state.driveRoot ?? "the selected target"}. Nothing is formatted
          or deleted. The app bundle, vault, models, and agents are not part of this build yet and are listed as such.
        </Notice>
        {volume && !fitsOnDrive(state, volume) ? (
          <Notice tone="danger" role="alert">
            Not enough free space at {state.driveRoot}. Choose another target or a smaller installation.
          </Notice>
        ) : null}
      </>
    );
  }

  const log = tasks
    .filter((task) => task.status === "done" || task.status === "failed" || task.status === "active")
    .map((task) => ({
      id: task.id,
      time: task.status,
      text: task.detail ? `${task.title}: ${task.detail}` : task.title,
      tone: task.status === "done" ? ("success" as const) : task.status === "failed" ? ("danger" as const) : ("default" as const),
    }));
  const detailsToggle = (
    <Button variant="ghost" size="md" aria-expanded={showDetails} onClick={() => setShowDetails((open) => !open)}>
      {showDetails ? "Hide" : "Show"} details
    </Button>
  );

  if (phase === "running") {
    const active = tasks.find((task) => task.status === "active");
    return (
      <>
        <PageHeader eyebrow="Setting up" title="Preparing AiDEX target" description={state.driveRoot ?? undefined} titleRef={titleRef} />
        <Stack gap={4}>
          <Heading level={2} size="title">
            {percent}%
          </Heading>
          <ProgressBar label="Target setup progress" value={completed} max={REAL_TASK_COUNT} />
          <Text>{active?.title ?? "Working…"}</Text>
        </Stack>
        <StepList steps={steps(tasks)} />
        <Stack gap={3} align="start">
          {detailsToggle}
          {showDetails ? <ActivityLog live entries={log} /> : null}
        </Stack>
      </>
    );
  }

  if (phase === "failed") {
    return (
      <>
        <PageHeader eyebrow="Setup stopped" title="The target was not set up" description={state.driveRoot ?? undefined} titleRef={titleRef} />
        <Notice tone="danger" role="alert" variant="banner">
          {error?.message ?? "Setup stopped."} Nothing else was changed after this step.
        </Notice>
        <StepList steps={steps(tasks)} />
      </>
    );
  }

  const pendingComponents = tasks.filter((task) => task.status === "unavailable");
  return (
    <>
      <PageHeader
        eyebrow="Target ready"
        title="AiDEX target initialized"
        description={report ? `${report.root} now holds the AiDEX layout (schema v${report.health.schema_version}).` : undefined}
        titleRef={titleRef}
      />
      <Notice tone="success" variant="banner">
        {report?.root} is an AiDEX target with {report?.directories.length ?? 0} folders and a verified manifest
        {report?.health.writable ? "" : " (read-only)"}.
      </Notice>
      {pendingComponents.length ? (
        <Notice tone="warning">
          Not installed yet: {pendingComponents.map((task) => task.title.toLowerCase()).join("; ")}. This build prepares the target only.
        </Notice>
      ) : null}
      <StepList steps={steps(tasks)} />
      <Stack gap={3} align="start">
        {detailsToggle}
        {showDetails && report ? (
          <ActivityLog
            entries={[
              ...log,
              { id: "free", time: "free", text: `${formatBytes(report.health.free_bytes)} free` },
              { id: "dirs", time: "folders", text: report.directories.join(", ") },
            ]}
          />
        ) : null}
      </Stack>
    </>
  );
}
