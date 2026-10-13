import { useEffect, useState } from "react";
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
import type { Checkpoint, ContentChunk } from "@aidex/client/dto/sessions";
import { useNavigation } from "../../app/providers/navigation";
import { useRpc } from "../../hooks/useRpc";
import { useSessionUpdates } from "../../hooks/useSessionUpdates";
import { pageRangeLabel } from "../sessions/pageRange";
import { CoreStatusNotice } from "../system/CoreStatusNotice";
import {
  newerWindow,
  olderWindow,
  shownWindow,
  type PageWindow,
} from "./checkpointPages";
import { createLatestRequest } from "./latestRequest";

/** While the session's event stream is down, the list is re-read this often instead. */
const POLL_MS = 5000;

export function CheckpointsPage() {
  const { route, replace } = useNavigation();
  const sessions = useRpc("conversation.list");
  const session = useRpc(
    "conversation.get",
    { id: route.id ?? "" },
    { enabled: !!route.id },
  );
  const total = session.data?.total_checkpoints;
  const [page, setPage] = useState<PageWindow>(null);
  const shown = shownWindow(page, total ?? 0);
  const listEnabled = !!route.id && total !== undefined;
  const checkpoints = useRpc(
    "checkpoint.list",
    { session_id: route.id ?? "", ...shown },
    { enabled: listEnabled },
  );
  const listTotal = checkpoints.data?.total_checkpoints;
  const { refetch: refetchSession } = session;
  const { refetch: refetchList } = checkpoints;
  // Session events (a run editing files) re-read the list; a poll stands in while the stream is down.
  const live = useSessionUpdates(route.id, refetchList);
  useEffect(() => {
    if (!listEnabled || live.active) return;
    const timer = setInterval(refetchList, POLL_MS);
    return () => clearInterval(timer);
  }, [listEnabled, live.active, refetchList]);
  // The newest page is placed by the conversation's count. When the list counts more, re-read the
  // conversation so that page moves on to the new checkpoints; when it counts fewer, re-read the list.
  useEffect(() => {
    if (total === undefined || listTotal === undefined || total === listTotal)
      return;
    if (listTotal > total) refetchSession();
    else refetchList();
  }, [total, listTotal, refetchSession, refetchList]);
  // Kept apart from the list, so re-reading or moving the newest page never closes the selection.
  const [selected, setSelected] = useState<Checkpoint>();
  const [before, setBefore] = useState<ContentChunk>();
  const [current, setCurrent] = useState<string>();
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  // Late workspace and checkpoint-content answers must not replace a newer selection.
  const [selection] = useState(createLatestRequest);
  const [contentReads] = useState(createLatestRequest);
  const compare = useRpc(
    "checkpoint.compare",
    { session_id: route.id ?? "", id: selected?.id ?? "" },
    { enabled: !!route.id && !!selected },
  );
  const listed = checkpoints.data?.checkpoints ?? [];

  const clearSelection = () => {
    selection.invalidate();
    contentReads.invalidate();
    setSelected(undefined);
    setBefore(undefined);
    setCurrent(undefined);
  };
  useEffect(() => {
    clearSelection();
    setPage(null);
    setError(undefined);
    // clearSelection only touches stable guards and state setters.
  }, [route.id]);

  const showPage = (next: PageWindow) => {
    clearSelection();
    setPage(next);
  };
  const showNewer = () => {
    if (!checkpoints.data) return;
    const next = newerWindow(
      checkpoints.data.total_checkpoints,
      checkpoints.data.next_offset,
    );
    if (next !== undefined) showPage(next);
  };

  const readBefore = (offset = 0) => {
    if (!route.id || !selected) return;
    const token = contentReads.begin();
    void call("checkpoint.content", {
      session_id: route.id,
      id: selected.id,
      offset,
    })
      .then((content) => {
        if (contentReads.isCurrent(token)) setBefore(content);
      })
      .catch((caught) => {
        if (contentReads.isCurrent(token))
          setError(toAidexError(caught).message);
      });
  };
  const open = async (checkpoint: Checkpoint) => {
    const token = selection.begin();
    contentReads.invalidate();
    setSelected(checkpoint);
    setBefore(undefined);
    setCurrent(undefined);
    setError(undefined);
    const workspaceId = session.data?.workspace_id;
    if (!workspaceId) return;
    try {
      const file = (await callUntyped("workspace.files.read", {
        workspace_id: workspaceId,
        path: checkpoint.path,
      })) as { content: string | null; truncated: boolean; binary: boolean };
      if (!selection.isCurrent(token)) return;
      setCurrent(
        file.binary
          ? "[binary content]"
          : `${file.content ?? ""}${file.truncated ? "\n[excerpt]" : ""}`,
      );
    } catch (caught) {
      if (selection.isCurrent(token))
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
      clearSelection();
      session.refetch();
      checkpoints.refetch();
    } catch (caught) {
      setError(toAidexError(caught).message);
    } finally {
      setBusy(false);
    }
  };

  const problem =
    error ??
    session.error?.message ??
    checkpoints.error?.message ??
    compare.error?.message;
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
          onChange={(event) =>
            replace({ page: route.page, id: event.target.value || undefined })
          }
        />
        {problem && <Notice tone="danger">{problem}</Notice>}
        <List>
          {[...listed].reverse().map((checkpoint) => (
            <ListRow
              key={checkpoint.id}
              title={checkpoint.path}
              subtitle={checkpoint.run_id}
              meta={[
                checkpoint.before_present
                  ? `${checkpoint.before_bytes} original bytes`
                  : "New file",
              ]}
              onSelect={() => void open(checkpoint)}
            />
          ))}
        </List>
        {checkpoints.data && !checkpoints.data.total_checkpoints && (
          <Text tone="muted">
            No file-change snapshots in this conversation.
          </Text>
        )}
        {checkpoints.data?.checkpoints_truncated && (
          <Notice>
            Some checkpoints of this page were left out to fit the response.
          </Notice>
        )}
        {checkpoints.data && checkpoints.data.total_checkpoints > 0 && (
          <Stack direction="row" gap={2}>
            <Button
              variant="outline"
              disabled={shown.offset === 0}
              onClick={() => showPage(olderWindow(shown))}
            >
              Older checkpoints
            </Button>
            <Text>
              {pageRangeLabel(
                checkpoints.data.offset,
                listed.length,
                checkpoints.data.total_checkpoints,
              )}
            </Text>
            <Button
              variant="outline"
              disabled={checkpoints.data.next_offset == null}
              onClick={showNewer}
            >
              Newer checkpoints
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
              )}
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
