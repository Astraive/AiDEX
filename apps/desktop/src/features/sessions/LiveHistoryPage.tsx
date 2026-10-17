import { useEffect, useRef, useState } from "react";
import {
  Dialog,
  List,
  ListRow,
  Notice,
  PageHeader,
  formatDateTime,
} from "@aidex/ui/components";
import { PageFrame } from "@aidex/ui/desktop";
import { PinIcon, TrashIcon } from "@aidex/ui/icons";
import {
  Button,
  IconButton,
  SearchField,
  Stack,
  Text,
} from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import type { SessionSummary } from "@aidex/client/dto/sessions";
import { useRpc } from "../../hooks/useRpc";
import { firstControl, useRemovalFocus } from "../../hooks/useRemovalFocus";
import { useNavigation } from "../../app/providers/navigation";
import { CoreStatusNotice } from "../system/CoreStatusNotice";
import { pageRangeLabel } from "./pageRange";

/** Saved sessions per page (`conversation.list` allows at most 100). */
const PAGE_SIZE = 50;

/** The creation date in the viewer's locale; empty when missing or not a valid time. */
function createdLabel(value: number | undefined): string {
  const date = value ? new Date(value) : undefined;
  return date && !Number.isNaN(date.getTime()) ? formatDateTime(date) : "";
}

export function LiveHistoryPage() {
  const [offset, setOffset] = useState(0);
  const sessions = useRpc("conversation.list", { offset, limit: PAGE_SIZE });
  const { navigate } = useNavigation();
  const [query, setQuery] = useState("");
  const [error, setError] = useState<string>();
  // The delete confirmation; a failed delete is reported inside it.
  const [deleting, setDeleting] = useState<SessionSummary>();
  const [deleteBusy, setDeleteBusy] = useState(false);
  const [deleteError, setDeleteError] = useState<string>();
  const page = sessions.data;
  useEffect(() => {
    // Deleting the last session of the last page leaves it empty: show the new last page.
    if (page && !page.sessions.length && offset > 0)
      setOffset(
        Math.max(
          0,
          Math.floor((page.total_sessions - 1) / PAGE_SIZE) * PAGE_SIZE,
        ),
      );
  }, [page, offset]);
  const items = (page?.sessions ?? []).filter((session) =>
    `${session.title ?? ""} ${session.id}`
      .toLowerCase()
      .includes(query.toLowerCase()),
  );
  // After a delete, focus moves to the row that took its place, else to the page title.
  const titleRef = useRef<HTMLHeadingElement>(null);
  const listRef = useRef<HTMLUListElement>(null);
  const removalFocus = useRemovalFocus(
    items.map((session) => session.id),
    (index) => firstControl(listRef.current?.children[index]),
    () => titleRef.current,
  );
  const remove = async (session: SessionSummary) => {
    setDeleteBusy(true);
    setDeleteError(undefined);
    try {
      await call("conversation.delete", { id: session.id });
      removalFocus.removed(session.id);
      setDeleting(undefined);
      sessions.refetch();
    } catch (caught) {
      setDeleteError(toAidexError(caught).message);
    } finally {
      setDeleteBusy(false);
    }
  };
  const closeDelete = () => {
    if (deleteBusy) return;
    setDeleting(undefined);
    setDeleteError(undefined);
  };
  return (
    <PageFrame width="default">
      <Stack gap={5}>
        <PageHeader
          titleRef={titleRef}
          title="Session history"
          description="Conversations and task history stored on the portable drive."
        />
        <CoreStatusNotice requires="daemon" />
        <SearchField
          label="Search this page of saved sessions"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
        {(error || sessions.error) && (
          <Notice tone="danger">{error ?? sessions.error?.message}</Notice>
        )}
        {/* Two meta columns, the state and then the date, each wide enough for a date on one line (as in Agents). */}
        <List ref={listRef} metaWidth="160px">
          {items.map((session) => (
            <ListRow
              key={session.id}
              title={session.title ?? "Locked conversation"}
              subtitle={
                session.locked
                  ? "Unlock the drive vault to read this session."
                  : `${session.provider_id ?? ""}/${session.model_id ?? ""}`
              }
              meta={[
                [session.status, session.mode].filter(Boolean).join(" · "),
                createdLabel(session.created_at),
              ]}
              onSelect={() =>
                navigate({
                  page: session.mode === "agent" ? "agent" : "chat",
                  id: session.id,
                })
              }
              trailing={
                <Stack direction="row" gap={2} align="center">
                  {/* One fixed-size toggle, so pinned and unpinned rows keep the same columns. */}
                  <IconButton
                    label={session.pinned ? "Unpin" : "Pin"}
                    icon={<PinIcon />}
                    selected={!!session.pinned}
                    disabled={session.locked}
                    onClick={() =>
                      void call("conversation.pin", {
                        id: session.id,
                        pinned: !session.pinned,
                      })
                        .then(sessions.refetch)
                        .catch((caught) =>
                          setError(toAidexError(caught).message),
                        )
                    }
                  />
                  <Button
                    variant="ghost"
                    aria-haspopup="dialog"
                    onClick={() => {
                      setDeleteError(undefined);
                      setDeleting(session);
                    }}
                  >
                    Delete history
                  </Button>
                </Stack>
              }
            />
          ))}
        </List>
        {page && (
          <Stack direction="row" gap={2} align="center" wrap>
            <Button
              variant="outline"
              disabled={offset === 0 || sessions.loading}
              onClick={() => setOffset(Math.max(0, offset - PAGE_SIZE))}
            >
              Previous page
            </Button>
            <Text>
              {pageRangeLabel(
                page.offset,
                page.sessions.length,
                page.total_sessions,
              )}
            </Text>
            <Button
              variant="outline"
              disabled={sessions.loading || page.next_offset == null}
              onClick={() => setOffset(page.next_offset ?? offset)}
            >
              Next page
            </Button>
          </Stack>
        )}
        {!sessions.loading && !items.length && (
          <Text tone="muted">No saved conversations match this search.</Text>
        )}
      </Stack>
      <Dialog
        open={deleting !== undefined}
        title="Delete this conversation?"
        description={`${deleting?.title || (deleting?.locked ? "This locked conversation" : "This conversation")} and its AiDEX history are deleted. Project files are kept.`}
        onClose={closeDelete}
        returnFocus={removalFocus.returnFocus}
      >
        <Stack gap={4}>
          {deleteError && (
            <Notice tone="danger" role="alert">
              {deleteError}
            </Notice>
          )}
          <Stack direction="row" gap={2} justify="end">
            <Button variant="ghost" disabled={deleteBusy} onClick={closeDelete}>
              Cancel
            </Button>
            <Button
              variant="primary"
              icon={<TrashIcon />}
              disabled={deleteBusy || !deleting}
              focusableWhenDisabled={deleteBusy}
              onClick={() => deleting && void remove(deleting)}
            >
              {deleteBusy ? "Deleting…" : "Delete history"}
            </Button>
          </Stack>
        </Stack>
      </Dialog>
    </PageFrame>
  );
}
