import { useState } from "react";
import {
  CodeBlock,
  List,
  ListRow,
  Notice,
  PageHeader,
} from "@aidex/ui/components";
import { PageFrame } from "@aidex/ui/desktop";
import { Button, Card, Select, Stack, Text } from "@aidex/ui/primitives";
import { call, callUntyped } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import type { ContentChunk } from "@aidex/client/dto/sessions";
import { useNavigation } from "../../app/providers/navigation";
import { useRpc } from "../../hooks/useRpc";
export function CheckpointsPage() {
  const { route, replace } = useNavigation();
  const sessions = useRpc("conversation.list");
  const session = useRpc(
    "conversation.get",
    { id: route.id ?? "" },
    { enabled: !!route.id },
  );
  const [checkpointId, setCheckpointId] = useState("");
  const [before, setBefore] = useState<ContentChunk>();
  const [current, setCurrent] = useState<string>();
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const compare = useRpc(
    "checkpoint.compare",
    { session_id: route.id ?? "", id: checkpointId },
    { enabled: !!route.id && !!checkpointId },
  );
  const selected = session.data?.checkpoints.find(
    (checkpoint) => checkpoint.id === checkpointId,
  );
  const readBefore = (offset = 0) =>
    void call("checkpoint.content", {
      session_id: route.id!,
      id: checkpointId,
      offset,
    })
      .then(setBefore)
      .catch((caught) => setError(toAidexError(caught).message));
  const open = async (id: string) => {
    setCheckpointId(id);
    setBefore(undefined);
    setCurrent(undefined);
    setError(undefined);
    const cp = session.data?.checkpoints.find(
      (candidate) => candidate.id === id,
    );
    if (!cp || !session.data?.workspace_id) return;
    try {
      const file = (await callUntyped("workspace.files.read", {
        workspace_id: session.data.workspace_id,
        path: cp.path,
      })) as { content: string | null; truncated: boolean; binary: boolean };
      setCurrent(
        file.binary
          ? "[binary content]"
          : `${file.content ?? ""}${file.truncated ? "\n[excerpt]" : ""}`,
      );
    } catch (caught) {
      setCurrent(`[Unavailable: ${toAidexError(caught).message}]`);
    }
  };
  const restore = async () => {
    if (!selected || !route.id || !compare.data || compare.data.conflict)
      return;
    if (
      !window.confirm(`Restore ${selected.path} to its state before this edit?`)
    )
      return;
    setBusy(true);
    try {
      await call("checkpoint.restore", {
        session_id: route.id,
        id: selected.id,
        expected_digest: selected.after_digest,
      });
      setCheckpointId("");
      setBefore(undefined);
      setCurrent(undefined);
      session.refetch();
    } catch (caught) {
      setError(toAidexError(caught).message);
    } finally {
      setBusy(false);
    }
  };
  return (
    <PageFrame width="default">
      <Stack gap={5}>
        <PageHeader
          title="Checkpoints and changes"
          description="Inspect and restore snapshots captured before agent file changes."
        />
        <Select
          label="Conversation"
          value={route.id ?? ""}
          options={[
            { value: "", label: "Choose a conversation" },
            ...(sessions.data?.sessions ?? [])
              .filter((s) => !s.locked)
              .map((s) => ({ value: s.id, label: s.title || s.id })),
          ]}
          onChange={(event) => {
            setCheckpointId("");
            replace({ page: route.page, id: event.target.value || undefined });
          }}
        />
        {(error || session.error || compare.error) && (
          <Notice tone="danger">
            {error ?? session.error?.message ?? compare.error?.message}
          </Notice>
        )}
        <List>
          {session.data?.checkpoints.map((checkpoint) => (
            <ListRow
              key={checkpoint.id}
              title={checkpoint.path}
              subtitle={checkpoint.run_id}
              meta={[
                checkpoint.before_present
                  ? `${checkpoint.before_bytes} original bytes`
                  : "New file",
              ]}
              onSelect={() => void open(checkpoint.id)}
            />
          ))}
        </List>
        {session.data && !session.data.checkpoints.length && (
          <Text tone="muted">
            No file-change snapshots in this conversation.
          </Text>
        )}
        {session.data?.checkpoints_truncated && (
          <Notice>Showing the latest 100 checkpoints.</Notice>
        )}
        {selected && (
          <Card padding="md">
            <Stack gap={3}>
              <Text>{selected.path}</Text>
              {compare.data?.conflict && (
                <Notice tone="warning">
                  The file changed after this checkpoint. Restoration is blocked
                  to preserve those changes.
                </Notice>
              )}
              <Button variant="outline" onClick={() => readBefore()}>
                Read original content
              </Button>
              {before && (
                <CodeBlock
                  title={
                    before.exists
                      ? `Before · bytes ${before.offset} of ${before.total_bytes}`
                      : "Before · file did not exist"
                  }
                  code={before.content}
                />
              )}{" "}
              {before?.next_offset != null && (
                <Button
                  variant="ghost"
                  onClick={() => readBefore(before.next_offset!)}
                >
                  Next original-content page
                </Button>
              )}
              {current !== undefined && (
                <CodeBlock title="Current file" code={current} />
              )}
              <Button
                disabled={busy || !compare.data || compare.data.conflict}
                onClick={() => void restore()}
              >
                Restore checkpoint
              </Button>
            </Stack>
          </Card>
        )}
      </Stack>
    </PageFrame>
  );
}
