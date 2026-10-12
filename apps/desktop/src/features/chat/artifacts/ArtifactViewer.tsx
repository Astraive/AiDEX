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
import { toAidexError } from "@aidex/client/errors";
import type { ArtifactMeta } from "@aidex/client/dto/artifacts";
import { useRpc } from "../../../hooks/useRpc";
import {
  PREVIEW_LIMIT_BYTES,
  imageDataUrl,
  previewKind,
  previewLanguage,
  readArtifact,
  type ArtifactData,
} from "./artifactContent";

type Props = {
  id: string;
  /** Known metadata; read with `artifact.get` otherwise. */
  meta?: ArtifactMeta;
  onOpenLink: (url: string) => void;
  onDownload: (id: string) => void;
};

/**
 * Previews one artifact: Markdown for documents and plans, code for code and data, images (SVG too,
 * through a `data:` URL, where scripts never run), and plain text otherwise. Large artifacts show
 * their first megabyte; Download saves the whole file.
 */
export function ArtifactViewer({ id, meta: knownMeta, onOpenLink, onDownload }: Props) {
  const fetched = useRpc("artifact.get", { id }, { enabled: !knownMeta });
  const meta = knownMeta ?? fetched.data;
  const [content, setContent] = useState<{ id: string; data?: ArtifactData; error?: string }>({ id: "" });

  useEffect(() => {
    let current = true;
    setContent({ id });
    readArtifact(id, PREVIEW_LIMIT_BYTES).then(
      (data) => current && setContent({ id, data }),
      (caught) => current && setContent({ id, error: toAidexError(caught).message }),
    );
    return () => {
      current = false;
    };
  }, [id]);

  const data = content.id === id ? content.data : undefined;
  const error = fetched.error?.message ?? (content.id === id ? content.error : undefined);
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
