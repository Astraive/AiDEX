import { useEffect, useState } from "react";
import { List, ListRow, Notice, PageHeader } from "@aidex/ui/components";
import { PageFrame } from "@aidex/ui/desktop";
import { Button, SearchField, Stack, Text } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import { useRpc } from "../../hooks/useRpc";
import { useNavigation } from "../../app/providers/navigation";
import { CoreStatusNotice } from "../system/CoreStatusNotice";
import { pageRangeLabel } from "./pageRange";

/** Saved sessions per page (`conversation.list` allows at most 100). */
const PAGE_SIZE = 50;

export function LiveHistoryPage() {
  const [offset, setOffset] = useState(0);
  const sessions = useRpc("conversation.list", { offset, limit: PAGE_SIZE });
  const { navigate } = useNavigation();
  const [query, setQuery] = useState("");
  const [error, setError] = useState<string>();
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
  return (
    <PageFrame width="default">
      <Stack gap={5}>
        <PageHeader
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
        <List>
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
                session.status,
                session.mode ?? "",
                session.created_at
                  ? new Date(session.created_at).toLocaleString()
                  : "",
              ]}
              onSelect={() =>
                navigate({
                  page: session.mode === "agent" ? "agent" : "chat",
                  id: session.id,
                })
              }
              trailing={
                <Stack direction="row" gap={2}>
                  <Button
                    variant="ghost"
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
                  >
                    {session.pinned ? "Unpin" : "Pin"}
                  </Button>
                  <Button
                    variant="ghost"
                    onClick={() => {
                      if (
                        !window.confirm(
                          "Delete this conversation and its AiDEX history? Project files will be kept.",
                        )
                      )
                        return;
                      void call("conversation.delete", { id: session.id })
                        .then(sessions.refetch)
                        .catch((caught) =>
                          setError(toAidexError(caught).message),
                        );
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
          <Stack direction="row" gap={2}>
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
    </PageFrame>
  );
}
