import { Notice, SectionHeader, Terminal } from "@aidex/ui/components";
import { Card, Stack, StatusIndicator, Text } from "@aidex/ui/primitives";
import { processStatus, type ProcessTraces } from "./executionOutput";

/** Live stdout and stderr excerpts of the conversation's mediated processes, newest activity first. */
export function ProcessOutputPanel({ traces }: { traces: ProcessTraces }) {
  if (!traces.length) return null;
  return (
    <Stack gap={4}>
      <SectionHeader title="Live process output" />
      <Text tone="muted">
        The latest 64 KiB of each stream for up to eight processes in this
        conversation, child tasks included, received while this page is open.
        Live output is not saved or replayed; saved tool results are in the run
        history.
      </Text>
      {[...traces].reverse().map((trace) => {
        const status = processStatus(trace);
        return (
          <Card key={trace.id} padding="md">
            <Stack gap={3}>
              <Text>{trace.label}</Text>
              <Stack direction="row" gap={2}>
                <StatusIndicator label={status.label} tone={status.tone} />
                <Text tone="muted">{trace.id}</Text>
              </Stack>
              {trace.gap && (
                <Notice tone="warning">
                  Output sent while reconnecting is missing from this excerpt.
                </Notice>
              )}
              {trace.stdout && (
                <Terminal
                  title="stdout"
                  lines={[{ kind: "output", text: trace.stdout }]}
                />
              )}
              {trace.stderr && (
                <Terminal
                  title="stderr"
                  lines={[{ kind: "error", text: trace.stderr }]}
                />
              )}
              {!trace.stdout && !trace.stderr && (
                <Text tone="muted">No output received.</Text>
              )}
            </Stack>
          </Card>
        );
      })}
    </Stack>
  );
}
