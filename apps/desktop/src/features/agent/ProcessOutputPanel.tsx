import { CodeBlock, SectionHeader } from "@aidex/ui/components";
import { Card, Stack, Text } from "@aidex/ui/primitives";
import type { ProcessTraces } from "./executionOutput";
export function ProcessOutputPanel({ traces }: { traces: ProcessTraces }) {
  if (!Object.keys(traces).length) return null;
  return (
    <Stack gap={4}>
      <SectionHeader title="Live process output" />
      <Text tone="muted">
        Latest output for up to eight processes in this conversation, including
        child tasks. Each stream shows its latest 64 KiB excerpt. Saved tool
        history retains final results; live chunks cannot be replayed after
        reconnecting.
      </Text>
      {Object.entries(traces).map(([id, process]) => (
        <Card key={id} padding="md">
          <Stack gap={3}>
            <Text>{process.label}</Text>
            <Text tone="muted">
              {id} ·{" "}
              {process.ended
                ? process.exitCode !== null
                  ? `Exited with code ${process.exitCode}`
                  : process.signal !== null
                    ? `Exited with signal ${process.signal}`
                    : "Exited; no status code available"
                : process.uncertain
                  ? "Live state unavailable after disconnect"
                  : "Running"}
            </Text>
            {process.stdout && (
              <CodeBlock title="stdout" code={process.stdout} />
            )}
            {process.stderr && (
              <CodeBlock title="stderr" code={process.stderr} />
            )}
          </Stack>
        </Card>
      ))}
    </Stack>
  );
}
