import { useMemo, useState } from "react";
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

/** A session's or run's stored messages, 20 per page, rendered as a transcript. */
export function HistoryMessages({ scope }: { scope: HistoryScope }) {
  const [offset, setOffset] = useState(0);
  const [artifact, setArtifact] = useState<ArtifactRef | null>(null);
  const history = useRpc("history.list", { ...scope, offset, limit: 20 });
  const openLink = useOpenLink();
  const downloadArtifact = useArtifactDownload();
  const environment = useMemo<TranscriptEnvironment>(
    () => ({ scope, openLink, openArtifact: setArtifact, downloadArtifact }),
    [scope.session_id, scope.run_id, openLink, downloadArtifact],
  );
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
            {offset + 1}–{offset + (history.data?.messages.length ?? 0)} of{" "}
            {history.data?.total_messages ?? 0}
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
