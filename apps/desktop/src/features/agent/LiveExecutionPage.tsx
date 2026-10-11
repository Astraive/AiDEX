import { useEffect, useState } from "react";
import { Notice, PageHeader } from "@aidex/ui/components";
import { PageFrame } from "@aidex/ui/desktop";
import { Button, Card, Stack, Text } from "@aidex/ui/primitives";
import { useNavigation } from "../../app/providers/navigation";
import { useRpc } from "../../hooks/useRpc";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import { HistoryMessages } from "../sessions/HistoryMessages";
import { useSessionUpdates } from "../../hooks/useSessionUpdates";
import { captureProcessEvent, type ProcessTraces } from "./executionOutput";
import { ProcessOutputPanel } from "./ProcessOutputPanel";
export function LiveExecutionPage() {
  const { route, navigate } = useNavigation();
  const session = useRpc(
    "conversation.get",
    { id: route.id ?? "" },
    { enabled: !!route.id },
  );
  const [selected, setSelected] = useState<string>();
  const [error, setError] = useState<string>();
  const [childText, setChildText] = useState<Record<string, string>>({});
  const [processes, setProcesses] = useState<ProcessTraces>({});
  const live = useSessionUpdates(route.id, session.refetch, (envelope) => {
    if (envelope.event.type.startsWith("process.")) {
      setProcesses((previous) => captureProcessEvent(previous, envelope));
      return;
    }
    if (envelope.event.type !== "agent.run_delta") return;
    const { agent_id, text } = envelope.event.data ?? {};
    if (typeof agent_id !== "string" || typeof text !== "string") return;
    setChildText((previous) =>
      Object.fromEntries(
        [
          ...Object.entries(previous).filter(([id]) => id !== agent_id),
          [agent_id, `${previous[agent_id] ?? ""}${text}`.slice(-65536)],
        ].slice(-16),
      ),
    );
  });
  useEffect(() => {
    setChildText({});
    setProcesses({});
    setSelected(undefined);
  }, [route.id]);
  useEffect(() => {
    if (!live.active)
      setProcesses((previous) =>
        Object.fromEntries(
          Object.entries(previous).map(([id, process]) => [
            id,
            process.ended ? process : { ...process, uncertain: true },
          ]),
        ),
      );
  }, [live.active]);
  useEffect(() => {
    if (!route.id) return;
    const timer = setInterval(session.refetch, 1000);
    return () => clearInterval(timer);
  }, [route.id, session.refetch]);
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
        {(session.error || error) && (
          <Notice tone="danger">{error ?? session.error?.message}</Notice>
        )}
        {live.reconnecting && (
          <Notice tone="warning">
            Reconnecting execution updates. {live.error}
          </Notice>
        )}
        {!route.id && (
          <Button onClick={() => navigate({ page: "agent" })}>
            Start an agent task
          </Button>
        )}
        {session.data?.runs.map((run) => (
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
              {run.error && <Notice tone="danger">{run.error}</Notice>}
              {run.parent_run_id &&
                ["pending", "running", "waiting_approval"].includes(
                  run.status,
                ) &&
                childText[run.id] && (
                  <Stack gap={2}>
                    <Text tone="muted">Live child output · latest excerpt</Text>
                    <Text>{childText[run.id]}</Text>
                  </Stack>
                )}
              <Button variant="outline" onClick={() => setSelected(run.id)}>
                Inspect tool and model history
              </Button>
              <Text>{run.pending_call_count} queued tool calls</Text>
              {["pending", "running", "waiting_approval"].includes(
                run.status,
              ) && (
                <Button
                  variant="outline"
                  onClick={() =>
                    void call("run.cancel", { id: run.id })
                      .then(session.refetch)
                      .catch((caught) => setError(toAidexError(caught).message))
                  }
                >
                  Cancel run
                </Button>
              )}
            </Stack>
          </Card>
        ))}
        {session.data?.runs_truncated && (
          <Notice>Only the most recent runs appear in this summary.</Notice>
        )}
        <ProcessOutputPanel traces={processes} />
        {selected && (
          <HistoryMessages
            key={selected}
            scope={{ run_id: selected }}
            live={session.data?.runs.some(
              (run) =>
                run.id === selected &&
                ["pending", "running", "waiting_approval"].includes(run.status),
            )}
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
