import { useEffect, useState } from "react";
import { Notice, PageHeader } from "@aidex/ui/components";
import { PageFrame } from "@aidex/ui/desktop";
import { Button, Card, Stack, Text } from "@aidex/ui/primitives";
import type { EventEnvelope } from "@aidex/client/dto/daemon";
import type { Run } from "@aidex/client/dto/sessions";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import { useNavigation } from "../../app/providers/navigation";
import { useAidexStatus } from "../../hooks/useAidexStatus";
import { usePoll } from "../../hooks/usePoll";
import { useRpc } from "../../hooks/useRpc";
import { runDelta } from "../../hooks/sessionEvents";
import { useSessionUpdates } from "../../hooks/useSessionUpdates";
import { HistoryMessages } from "../sessions/HistoryMessages";
import { CoreStatusNotice } from "../system/CoreStatusNotice";
import {
  appendRunText,
  captureProcessEvent,
  dropsLiveOutput,
  liveOutputAfterRead,
  markDisconnected,
  markRunTextsDisconnected,
  pruneRunTexts,
  type ProcessTraces,
  type RunTexts,
} from "./executionOutput";
import { ProcessOutputPanel } from "./ProcessOutputPanel";

const isActive = (run: Run) =>
  run.status === "pending" ||
  run.status === "running" ||
  run.status === "waiting_approval";

/** Executing (not waiting for a person): the daemon holds the vault open for it anyway. */
const isExecuting = (run: Run) =>
  run.status === "pending" || run.status === "running";

/** Poll while a run is active or the event stream is down; otherwise events trigger reloads. */
const ACTIVE_POLL_MS = 1000;
const IDLE_POLL_MS = 5000;

export function LiveExecutionPage() {
  const { route, navigate } = useNavigation();
  const { connectionKey } = useAidexStatus();
  const session = useRpc(
    "conversation.get",
    { id: route.id ?? "" },
    { enabled: !!route.id },
  );
  const [selected, setSelected] = useState<string>();
  const [error, setError] = useState<string>();
  const [processes, setProcesses] = useState<ProcessTraces>([]);
  const [runTexts, setRunTexts] = useState<RunTexts>([]);
  const live = useSessionUpdates(
    route.id,
    session.refetch,
    (envelope: EventEnvelope) => {
      if (envelope.event.type.startsWith("process.")) {
        setProcesses((previous) => captureProcessEvent(previous, envelope));
        return;
      }
      const delta = runDelta(envelope);
      if (delta)
        setRunTexts((previous) =>
          appendRunText(previous, delta.runId, delta.text),
        );
    },
  );
  // Another conversation, drive, or daemon: nothing shown so far belongs to it.
  useEffect(() => {
    setProcesses([]);
    setRunTexts([]);
    setSelected(undefined);
    setError(undefined);
  }, [route.id, connectionKey]);
  useEffect(() => {
    if (live.active) return;
    setProcesses(markDisconnected);
    setRunTexts(markRunTextsDisconnected);
  }, [live.active]);
  // A read refused because the vault locked: the decrypted excerpts and transcript go with the
  // snapshot (`useBridgeQuery` already dropped it), so nothing read before the lock stays on screen.
  const sessionError = session.error;
  useEffect(() => {
    if (!dropsLiveOutput(sessionError)) return;
    setProcesses((previous) => liveOutputAfterRead(sessionError, previous));
    setRunTexts((previous) => liveOutputAfterRead(sessionError, previous));
    setSelected(undefined);
  }, [sessionError]);
  const runs = session.data?.runs ?? [];
  // A run the snapshot reports as ended loses its excerpt; resumed, it starts a new one.
  const snapshot = session.data;
  useEffect(() => {
    if (snapshot)
      setRunTexts((previous) => pruneRunTexts(previous, snapshot.runs, isActive));
  }, [snapshot]);
  const running = runs.some(isActive);
  // Reads decrypt the conversation (vault use): only while the user is present, except while a run
  // executes, which holds the vault open anyway.
  usePoll(
    session.refetch,
    running || !live.active ? ACTIVE_POLL_MS : IDLE_POLL_MS,
    { enabled: !!route.id, whileAway: runs.some(isExecuting) },
  );
  const selectedRun = runs.find((run) => run.id === selected);
  return (
    <PageFrame width="default">
      <Stack gap={5}>
        <PageHeader
          title="Execution"
          description={
            session.data?.title ||
            "Open an agent conversation to inspect its execution."
          }
        />
        <CoreStatusNotice requires="daemon" />
        {(session.error || error) && (
          <Notice tone="danger">{error ?? session.error?.message}</Notice>
        )}
        {route.id && live.reconnecting && (
          <Notice tone="warning">
            Reconnecting to live updates.{live.error ? ` ${live.error}` : ""}{" "}
            Live output sent meanwhile cannot be replayed; the saved session
            reloads when the connection returns.
          </Notice>
        )}
        {!route.id && (
          <Button onClick={() => navigate({ page: "agent" })}>
            Start an agent task
          </Button>
        )}
        {runs.map((run) => {
          const liveText = isActive(run)
            ? runTexts.find((entry) => entry.runId === run.id)
            : undefined;
          return (
            <Card key={run.id} padding="md">
              <Stack gap={3}>
                <Text>
                  {run.parent_run_id
                    ? `Child of ${run.parent_run_id}`
                    : "Root run"}{" "}
                  · {run.status} · {run.steps} steps
                </Text>
                <Text tone="muted">{run.id}</Text>
                {run.model_used && (
                  <Text>
                    Model: {run.model_used} · output limit{" "}
                    {run.effective_max_tokens}
                  </Text>
                )}
                {!!run.inference_retries && (
                  <Text tone="muted">
                    Retried {run.inference_retries} model{" "}
                    {run.inference_retries === 1 ? "request" : "requests"} after
                    a temporary provider failure.
                  </Text>
                )}
                {!!run.context_trimmed_messages && (
                  <Text tone="muted">
                    Left out up to {run.context_trimmed_messages} older{" "}
                    {run.context_trimmed_messages === 1
                      ? "message"
                      : "messages"}{" "}
                    to fit the model&apos;s context window; the saved history
                    is complete.
                  </Text>
                )}
                {run.error && <Notice tone="danger">{run.error}</Notice>}
                {liveText?.text && (
                  <Stack gap={2}>
                    <Text tone="muted">Live output · latest excerpt</Text>
                    {liveText.gap && (
                      <Notice tone="warning">
                        Text sent while reconnecting is missing from this
                        excerpt; the saved transcript has it once the run ends.
                      </Notice>
                    )}
                    <Text>{liveText.text}</Text>
                  </Stack>
                )}
                <Button
                  variant="outline"
                  onClick={() =>
                    setSelected(selected === run.id ? undefined : run.id)
                  }
                >
                  {selected === run.id
                    ? "Hide tool and model history"
                    : "Inspect tool and model history"}
                </Button>
                <Text>{run.pending_call_count} queued tool calls</Text>
                {isActive(run) && (
                  <Button
                    variant="outline"
                    onClick={() =>
                      void call("run.cancel", { id: run.id })
                        .then(session.refetch)
                        .catch((caught) =>
                          setError(toAidexError(caught).message),
                        )
                    }
                  >
                    Cancel run
                  </Button>
                )}
              </Stack>
            </Card>
          );
        })}
        {session.data?.runs_truncated && (
          <Notice>Only the most recent runs appear in this summary.</Notice>
        )}
        {/* Shown only next to the snapshot it belongs to: a failed or pending read shows none. */}
        {session.data && <ProcessOutputPanel traces={processes} />}
        {selected && session.data && (
          <HistoryMessages
            key={selected}
            scope={{ run_id: selected }}
            live={!!selectedRun && isActive(selectedRun)}
            executing={!!selectedRun && isExecuting(selectedRun)}
          />
        )}
        <Stack direction="row" gap={2}>
          <Button
            variant="outline"
            onClick={() => navigate({ page: "agent", id: route.id })}
          >
            Open conversation
          </Button>
          <Button
            variant="outline"
            onClick={() => navigate({ page: "checkpoints", id: route.id })}
          >
            Checkpoints and diffs
          </Button>
        </Stack>
      </Stack>
    </PageFrame>
  );
}
