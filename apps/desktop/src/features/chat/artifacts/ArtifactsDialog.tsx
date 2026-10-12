import { useEffect, useState } from "react";
import { ArtifactCard, Dialog, EmptyState, Notice } from "@aidex/ui/components";
import { ArrowLeftIcon, DownloadIcon, LayersIcon } from "@aidex/ui/icons";
import { Button, Stack, Text } from "@aidex/ui/primitives";
import type { ArtifactMeta } from "@aidex/client/dto/artifacts";
import { useRpc } from "../../../hooks/useRpc";
import type { ArtifactRef } from "../transcript/TranscriptContext";
import { ArtifactViewer } from "./ArtifactViewer";

const PAGE_SIZE = 25;
const planStatusLabels: Record<string, string> = { superseded: "Superseded", completed: "Completed" };

type Props = {
  open: boolean;
  sessionId: string;
  /** Opens straight to this artifact (from a transcript card); null opens the list. */
  artifact: ArtifactRef | null;
  onClose: () => void;
  onOpenLink: (url: string) => void;
  onDownload: (id: string) => void;
};

/** The conversation's artifacts (newest first) with a viewer; opened from the header or a card. */
export function ArtifactsDialog({ open, sessionId, artifact, onClose, onOpenLink, onDownload }: Props) {
  // An artifact picked from the list; `artifact` (from a card) shows its viewer directly.
  const [picked, setPicked] = useState<ArtifactRef | null>(null);
  useEffect(() => {
    if (!open) setPicked(null);
  }, [open]);
  const viewing = artifact
    ? { ref: artifact, fromList: false }
    : picked
      ? { ref: picked, fromList: true }
      : null;

  return (
    <Dialog
      open={open}
      size="xl"
      title={viewing ? viewing.ref.name : "Artifacts"}
      description={viewing ? undefined : "Files, plans, and saved web pages from this conversation."}
      onClose={onClose}
    >
      {open && viewing ? (
        <Stack gap={4}>
          {viewing.fromList && (
            <Stack direction="row">
              <Button variant="ghost" icon={<ArrowLeftIcon />} onClick={() => setPicked(null)}>
                All artifacts
              </Button>
            </Stack>
          )}
          <ArtifactViewer
            key={viewing.ref.id}
            id={viewing.ref.id}
            meta={viewing.ref.meta}
            onOpenLink={onOpenLink}
            onDownload={onDownload}
          />
        </Stack>
      ) : open ? (
        <ArtifactList
          sessionId={sessionId}
          onOpen={(meta) => setPicked({ id: meta.id, name: meta.name, meta })}
          onDownload={onDownload}
        />
      ) : null}
    </Dialog>
  );
}

function ArtifactList({
  sessionId,
  onOpen,
  onDownload,
}: {
  sessionId: string;
  onOpen: (meta: ArtifactMeta) => void;
  onDownload: (id: string) => void;
}) {
  const [offset, setOffset] = useState(0);
  const list = useRpc("artifact.list", { session_id: sessionId, offset, limit: PAGE_SIZE });
  const artifacts = list.data?.artifacts ?? [];
  if (list.error) return <Notice tone="danger">{list.error.message}</Notice>;
  if (list.loading && !list.data) return <Text tone="muted">Loading artifacts…</Text>;
  if (!artifacts.length && offset === 0)
    return (
      <EmptyState
        icon={<LayersIcon />}
        title="No artifacts yet"
        description="Documents and code the assistant saves, proposed plans, and long web pages it reads appear here."
      />
    );
  const total = list.data?.total_artifacts ?? artifacts.length;
  return (
    <Stack gap={3}>
      {artifacts.map((meta) => (
        <ArtifactCard
          key={meta.id}
          name={meta.name}
          kind={meta.kind}
          size={meta.size}
          createdAt={meta.updated_at || meta.created_at}
          detail={planStatusLabels[meta.plan_status ?? ""] ?? meta.language ?? undefined}
          onOpen={() => onOpen(meta)}
          actions={
            <>
              <Button variant="ghost" onClick={() => onOpen(meta)}>
                Open
              </Button>
              <Button variant="ghost" icon={<DownloadIcon />} onClick={() => onDownload(meta.id)}>
                Download
              </Button>
            </>
          }
        />
      ))}
      {(offset > 0 || list.data?.next_offset != null) && (
        <Stack direction="row" gap={2} align="center">
          <Button
            variant="outline"
            disabled={offset === 0}
            onClick={() => setOffset(Math.max(0, offset - PAGE_SIZE))}
          >
            Newer
          </Button>
          <Text tone="muted">
            {offset + 1}–{offset + artifacts.length} of {total}
          </Text>
          <Button
            variant="outline"
            disabled={list.data?.next_offset == null}
            onClick={() => setOffset(list.data!.next_offset!)}
          >
            Older
          </Button>
        </Stack>
      )}
    </Stack>
  );
}
