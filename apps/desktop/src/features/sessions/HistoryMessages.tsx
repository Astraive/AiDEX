import { useEffect, useMemo, useState } from "react";
import { Notice } from "@aidex/ui/components";
import { Button, Stack, Text } from "@aidex/ui/primitives";
import type { HistoryScope } from "@aidex/client/dto/sessions";
import { useRpc } from "../../hooks/useRpc";
import { ArtifactsDialog } from "../chat/artifacts/ArtifactsDialog";
import { useArtifactDownload } from "../chat/artifacts/useArtifactDownload";
import { Transcript } from "../chat/transcript/Transcript";
import {
  TranscriptProvider,
  type ArtifactRef,
  type TranscriptEnvironment,
} from "../chat/transcript/TranscriptContext";
import { useOpenLink } from "../chat/transcript/useOpenLink";
import { pageRangeLabel } from "./pageRange";

/** How often a live history page is re-read while its run is active. */
const LIVE_REFRESH_MS = 1000;

/**
 * A session's or run's stored messages, 20 per page, rendered as a transcript. While `live` (the
 * run is still active) the page is re-read every second, and once more when it stops.
 */
export function HistoryMessages({ scope, live = false }: { scope: HistoryScope; live?: boolean }) {
  const [offset, setOffset] = useState(0);
  const [artifact, setArtifact] = useState<ArtifactRef | null>(null);
  const history = useRpc("history.list", { ...scope, offset, limit: 20 });
  const openLink = useOpenLink();
  const downloadArtifact = useArtifactDownload();
  const environment = useMemo<TranscriptEnvironment>(
    () => ({ scope, openLink, openArtifact: setArtifact, downloadArtifact }),
    [scope.session_id, scope.run_id, openLink, downloadArtifact],
  );
  const { refetch } = history;
  useEffect(() => {
    if (!live) return;
    const timer = setInterval(refetch, LIVE_REFRESH_MS);
    return () => {
      clearInterval(timer);
      // The run stopped (or the page closed): read its final messages once.
      refetch();
    };
  }, [live, refetch]);
  return (
    <TranscriptProvider value={environment}>
      <Stack gap={4}>
        {history.error && <Notice tone="danger">{history.error.message}</Notice>}
        {/* A page may hold a call without its result (or the reverse); those rows show no status. */}
        <Transcript messages={history.data?.messages ?? []} options={{ running: false, complete: false }} />
        <Stack direction="row" gap={2}>
          <Button
            variant="outline"
            disabled={offset === 0}
            onClick={() => setOffset(Math.max(0, offset - 20))}
          >
            Previous messages
          </Button>
          <Text>
            {pageRangeLabel(
              history.data?.offset ?? offset,
              history.data?.messages.length ?? 0,
              history.data?.total_messages ?? 0,
            )}
          </Text>
          <Button
            variant="outline"
            disabled={history.data?.next_offset == null}
            onClick={() => setOffset(history.data!.next_offset!)}
          >
            Next messages
          </Button>
        </Stack>
      </Stack>
      <ArtifactsDialog
        open={artifact !== null}
        sessionId={scope.session_id ?? ""}
        artifact={artifact}
        onClose={() => setArtifact(null)}
        onOpenLink={openLink}
        onDownload={downloadArtifact}
      />
    </TranscriptProvider>
  );
}
