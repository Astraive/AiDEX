import { useCallback, useRef, useState } from "react";
import type { DriveInspection, InitReport } from "@aidex/client/dto/installer";
import { toAidexError, type AidexError } from "@aidex/client/errors";
import { initDrive, initNormalTarget, inspectDrive, inspectNormalTarget } from "@aidex/client/installer";
import type { SetupState } from "./setup";

export type RunPhase = "review" | "running" | "finished" | "failed";

export type TaskStatus = "pending" | "active" | "done" | "failed" | "unavailable" | "skipped";

export type InstallTask = {
  id: string;
  title: string;
  status: TaskStatus;
  /** What happened, or which component this step waits for. */
  detail?: string;
};

/** Steps whose backend is not part of this build. They are listed, never simulated or marked done. */
function laterTasks(state: SetupState): InstallTask[] {
  return [
    {
      id: "bundle",
      title: "Copy the AiDEX app and runtime to the target",
      status: "unavailable",
      detail: "Available when the verified desktop bundle is packaged (issue #55).",
    },
    state.vault === "password"
      ? {
          id: "vault",
          title: "Create the encrypted vault",
          status: "unavailable",
          detail: "Created in AiDEX on first launch, where you set the password. Setup did not keep it.",
        }
      : { id: "vault", title: "Vault", status: "skipped", detail: "No-password mode chosen; no vault is created." },
    state.models.length
      ? {
          id: "models",
          title: "Import the selected models",
          status: "unavailable",
          detail: "Available when model import is installed (issue #15).",
        }
      : { id: "models", title: "Models", status: "skipped", detail: "No local models selected." },
    state.agents.length || state.extensions.length
      ? {
          id: "agents",
          title: "Install agents and extensions",
          status: "unavailable",
          detail: "Available when agent adapters and extensions are installed (issues #23, #24).",
        }
      : { id: "agents", title: "Agents and extensions", status: "skipped", detail: "AiDEX Native only." },
  ];
}

const REAL_TASKS: readonly InstallTask[] = [
  { id: "inspect", title: "Check the installation target", status: "pending" },
  { id: "init", title: "Create the AiDEX folders and drive manifest", status: "pending" },
  { id: "verify", title: "Verify the installation target", status: "pending" },
];

/** Steps with a backend today; progress counts only these. */
export const REAL_TASK_COUNT = REAL_TASKS.length;

/** Runs the real part of setup: inspect the selected target, initialize only the AiDEX folders
 * and manifest, then inspect again to verify. Application components without a packaged bundle
 * stay unavailable.
 */
export function useInstallRun() {
  const [phase, setPhase] = useState<RunPhase>("review");
  const [tasks, setTasks] = useState<readonly InstallTask[]>([]);
  const [report, setReport] = useState<InitReport | null>(null);
  const [error, setError] = useState<AidexError | null>(null);
  const runId = useRef(0);

  const start = useCallback(async (state: SetupState) => {
    const root = state.driveRoot;
    if (!root) return;
    const id = ++runId.current;
    const live = () => runId.current === id;
    let current: InstallTask[] = [...REAL_TASKS.map((task) => ({ ...task })), ...laterTasks(state)];
    const mark = (taskId: string, status: TaskStatus, detail?: string) => {
      current = current.map((task) => (task.id === taskId ? { ...task, status, detail } : task));
      if (live()) setTasks(current);
    };
    setPhase("running");
    setReport(null);
    setError(null);
    setTasks(current);

    const fail = (taskId: string, caught: unknown) => {
      const failure = toAidexError(caught);
      mark(taskId, "failed", failure.message);
      if (!live()) return;
      setError(failure);
      setPhase("failed");
    };

    let inspection: DriveInspection;
    mark("inspect", "active");
    try {
      inspection = state.installMode === "normal" ? await inspectNormalTarget() : await inspectDrive(root);
    } catch (caught) {
      return fail("inspect", caught);
    }
    if (!inspection.eligible) {
      return fail("inspect", new Error(inspection.ineligible_reason ?? "This installation target cannot be used."));
    }
    mark(
      "inspect",
      "done",
      inspection.health
        ? `Existing AiDEX target, schema v${inspection.health.schema_version ?? "?"}`
        : state.installMode === "normal"
          ? "Per-user location ready"
          : "Removable drive ready",
    );

    let initialized: InitReport;
    mark("init", "active");
    try {
      initialized = state.installMode === "normal" ? await initNormalTarget() : await initDrive(root);
    } catch (caught) {
      return fail("init", caught);
    }
    const migrated =
      initialized.previous_schema_version !== null && initialized.previous_schema_version < initialized.health.current_schema_version;
    mark(
      "init",
      "done",
      migrated
        ? `Migrated schema v${initialized.previous_schema_version} to v${initialized.health.schema_version} (original kept as aidex-drive.v1.json)`
        : initialized.previous_schema_version !== null
          ? "Already initialized; folders verified"
          : `${initialized.directories.length} folders and the manifest created`,
    );

    mark("verify", "active");
    try {
      const verified =
        state.installMode === "normal" ? await inspectNormalTarget() : await inspectDrive(root);
      const health = verified.health;
      if (!health || health.migration_required || health.schema_version !== health.current_schema_version) {
        return fail("verify", new Error(verified.health_error?.message ?? "The installation target did not verify."));
      }
      mark("verify", "done", `Schema v${health.schema_version}${health.writable ? ", writable" : ", read-only"}`);
    } catch (caught) {
      return fail("verify", caught);
    }

    if (!live()) return;
    setReport(initialized);
    setPhase("finished");
  }, []);

  const reset = useCallback(() => {
    runId.current += 1;
    setPhase("review");
    setTasks([]);
    setReport(null);
    setError(null);
  }, []);

  const completed = tasks.filter((task) => REAL_TASKS.some((real) => real.id === task.id) && task.status === "done").length;
  return { phase, tasks, report, error, completed, start, reset };
}
