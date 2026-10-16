import { useEffect, useState } from "react";
import {
  CodeBlock,
  Markdown,
  Notice,
  artifactKindLabel,
  formatByteSize,
  formatDateTime,
} from "@aidex/ui/components";
import { DownloadIcon } from "@aidex/ui/icons";
import { Button, Stack, Text } from "@aidex/ui/primitives";
import { ErrorKind, toAidexError } from "@aidex/client/errors";
import type { ArtifactMeta } from "@aidex/client/dto/artifacts";
import { usePoll } from "../../../hooks/usePoll";
import { useRpc } from "../../../hooks/useRpc";
import { useVaultLocks } from "../../../hooks/useVaultLocks";
import {
  PREVIEW_LIMIT_BYTES,
  imageDataUrl,
  previewKind,
  previewLanguage,
  readArtifact,
  type ArtifactData,
} from "./artifactContent";
import {
  EMPTY_PREVIEW,
  LOCKED_RETRY_MS,
  previewFailed,
  previewRead,
  previewStarted,
  shownPreview,
  type PreviewState,
} from "./artifactPreview";

type Props = {
  id: string;
  /** Known metadata; read with `artifact.get` otherwise. */
  meta?: ArtifactMeta;
  onOpenLink: (url: string) => void;
  onDownload: (id: string) => void;
  /** Told whenever the vault starts or stops keeping this artifact off the screen (false on unmount). */
  onLockedChange?: (locked: boolean) => void;
};

/**
 * Previews one artifact: Markdown for documents and plans, code for code and data, images (SVG too,
 * through a `data:` URL, where scripts never run), and plain text otherwise. Large artifacts show
 * their first megabyte; Download saves the whole file.
 *
 * The content is decrypted under one vault session, so a vault lock (also from outside the app:
 * `useVaultLocks`) or a read refused as `vault_locked` takes it, and the metadata, off the screen at
 * once (`artifactPreview.ts`). While the viewer stays open it tries again, so the preview comes back
 * once the vault is unlocked.
 */
export function ArtifactViewer({ id, meta: knownMeta, onOpenLink, onDownload, onLockedChange }: Props) {
  const fetched = useRpc("artifact.get", { id }, { enabled: !knownMeta });
  const locks = useVaultLocks();
  const [content, setContent] = useState<PreviewState>(EMPTY_PREVIEW);
  const [attempt, setAttempt] = useState(0);

  // A lock reads again: an answer to a read started before it is dropped, and what was read before it
  // is already hidden (`shownPreview`) in the render that sees the lock.
  useEffect(() => {
    let current = true;
    setContent((previous) => previewStarted(previous, id, locks));
    readArtifact(id, PREVIEW_LIMIT_BYTES).then(
      (data) => current && setContent(previewRead(id, locks, data)),
      (caught) => current && setContent(previewFailed(id, locks, toAidexError(caught))),
    );
    return () => {
      current = false;
    };
  }, [id, locks, attempt]);

  const preview = shownPreview(content, id, locks);
  const metaLocked = fetched.error?.kind === ErrorKind.vaultLocked;
  const locked = preview.status === "locked" || metaLocked;
  const { refetch: refetchMeta } = fetched;
  // Each try is refused at once while the vault stays locked; the first one after an unlock reads.
  usePoll(
    () => {
      setAttempt((value) => value + 1);
      if (metaLocked) refetchMeta();
    },
    LOCKED_RETRY_MS,
    { enabled: locked },
  );
  useEffect(() => {
    onLockedChange?.(locked);
    return () => onLockedChange?.(false);
  }, [locked, onLockedChange]);

  if (locked) return <Notice tone="warning">The vault is locked. Unlock it to see this artifact again.</Notice>;

  const meta = knownMeta ?? fetched.data;
  const data = preview.status === "ready" ? preview.data : undefined;
  const error = fetched.error?.message ?? (preview.status === "failed" ? preview.message : undefined);
  const facts = meta
    ? [
        artifactKindLabel(meta.kind),
        meta.language ?? meta.mime,
        formatByteSize(meta.size),
        formatDateTime(new Date(meta.updated_at || meta.created_at)),
      ]
    : [];

  return (
    <Stack gap={4}>
      <Stack direction="row" gap={3} align="center" justify="between" wrap>
        <Text tone="muted">{facts.join(" · ")}</Text>
        <Button variant="outline" icon={<DownloadIcon />} onClick={() => onDownload(id)}>
          Download
        </Button>
      </Stack>
      {error && <Notice tone="danger">{error}</Notice>}
      {!error && (!meta || !data) && <Text tone="muted">Loading…</Text>}
      {meta && data && <Preview meta={meta} data={data} onOpenLink={onOpenLink} />}
      {meta && data && !data.complete && (
        <Notice tone="info">
          Showing the first {formatByteSize(PREVIEW_LIMIT_BYTES)} of {formatByteSize(data.totalBytes)}. Download the
          file to see all of it.
        </Notice>
      )}
    </Stack>
  );
}

function Preview({
  meta,
  data,
  onOpenLink,
}: {
  meta: ArtifactMeta;
  data: ArtifactData;
  onOpenLink: (url: string) => void;
}) {
  const kind = previewKind(meta);
  if (kind === "image") {
    if (!data.complete) return <Text tone="muted">This image is too large to preview. Download it instead.</Text>;
    return <img src={imageDataUrl(meta, data)} alt={meta.name} />;
  }
  if (data.encoding === "base64" || kind === "binary")
    return <Text tone="muted">There is no preview for this file type. Download it to open it.</Text>;
  if (kind === "markdown") return <Markdown source={data.text} onOpenLink={onOpenLink} />;
  return <CodeBlock title={meta.name} language={kind === "code" ? previewLanguage(meta) : undefined} code={data.text} />;
}
