import { useEffect, useRef, useState } from "react";
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
import { CoreStatusNotice } from "../system/CoreStatusNotice";
export function CheckpointsPage() {
  const { route, replace } = useNavigation();
  const sessions = useRpc("conversation.list");
  const session = useRpc(
    "conversation.get",
    { id: route.id ?? "" },
    { enabled: !!route.id },
  );
  const [checkpointId, setCheckpointId] = useState("");
  const [offset, setOffset] = useState(0);
  const selection = useRef("");
  const checkpoints = useRpc(
    "checkpoint.list",
    { session_id: route.id ?? "", offset, limit: 20 },
    { enabled: !!route.id },
  );
  const [before, setBefore] = useState<ContentChunk>();
  const [current, setCurrent] = useState<string>();
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const compare = useRpc(
    "checkpoint.compare",
    { session_id: route.id ?? "", id: checkpointId },
    { enabled: !!route.id && !!checkpointId },
  );
  useEffect(() => {
    setCheckpointId("");
    setOffset(0);
    setBefore(undefined);
    setCurrent(undefined);
    selection.current = "";
  }, [route.id]);
  const selected = checkpoints.data?.checkpoints.find(
    (checkpoint) => checkpoint.id === checkpointId,
  );
  const readBefore = (offset = 0) => {
    const key = selection.current;
    return void call("checkpoint.content", {
      session_id: route.id!,
      id: checkpointId,
      offset,
    })
      .then((content) => {
        if (selection.current === key) setBefore(content);
      })
      .catch((caught) => setError(toAidexError(caught).message));
  };
  const open = async (id: string) => {
    setCheckpointId(id);
    const key = `${route.id}/${id}`;
    selection.current = key;
    setBefore(undefined);
    setCurrent(undefined);
    setError(undefined);
    const cp = checkpoints.data?.checkpoints.find(
      (candidate) => candidate.id === id,
    );
    if (!cp || !session.data?.workspace_id) return;
    try {
      const file = (await callUntyped("workspace.files.read", {
        workspace_id: session.data.workspace_id,
        path: cp.path,
      })) as { content: string | null; truncated: boolean; binary: boolean };
      if (selection.current !== key) return;
      setCurrent(
        file.binary
          ? "[binary content]"
          : `${file.content ?? ""}${file.truncated ? "\n[excerpt]" : ""}`,
      );
    } catch (caught) {
      if (selection.current === key)
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
      checkpoints.refetch();
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
        <CoreStatusNotice requires="daemon" />
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
        {(error || session.error || compare.error || checkpoints.error) && (
          <Notice tone="danger">
            {error ??
              session.error?.message ??
              compare.error?.message ??
              checkpoints.error?.message}
          </Notice>
        )}
        <List>
          {checkpoints.data?.checkpoints.map((checkpoint) => (
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
        {checkpoints.data && !checkpoints.data.total_checkpoints && (
          <Text tone="muted">
            No file-change snapshots in this conversation.
          </Text>
        )}
        {checkpoints.data && (
          <Stack direction="row" gap={2}>
            <Button
              variant="outline"
              disabled={!offset || checkpoints.loading}
              onClick={() => {
                setCheckpointId("");
                setOffset(Math.max(0, offset - 20));
              }}
            >
              Previous checkpoints
            </Button>
            <Text>
              {offset + (checkpoints.data.checkpoints.length ? 1 : 0)}–
              {offset + checkpoints.data.checkpoints.length} of{" "}
              {checkpoints.data.total_checkpoints}
            </Text>
            <Button
              variant="outline"
              disabled={
                checkpoints.loading || checkpoints.data.next_offset == null
              }
              onClick={() => {
                setCheckpointId("");
                setOffset(checkpoints.data!.next_offset!);
              }}
            >
              Next checkpoints
            </Button>
          </Stack>
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
