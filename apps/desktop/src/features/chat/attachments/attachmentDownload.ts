// Saves a sent attachment as a file: every `attachment.read` page is assembled into a `Blob` and
// saved through an anchor download, like artifacts.
import { useCallback, useState } from "react";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import { useToast } from "../../../app/providers/toast";
import { artifactFileName, saveBlob } from "../artifacts/artifactContent";
import { readAttachmentBytes } from "./attachmentImages";

/** Reads the whole attachment and saves it under its own (sanitized) name. */
export async function downloadAttachment(id: string): Promise<void> {
  const meta = await call("attachment.get", { id });
  const { bytes } = await readAttachmentBytes(id);
  const mime = meta.mime.split(";")[0].trim() || "application/octet-stream";
  saveBlob(new Blob([bytes], { type: mime }), artifactFileName({ name: meta.name, mime, kind: "document", language: null }));
}

/** Download with a per-id busy state; failures (a locked vault, a deleted conversation) show as a toast. */
export function useAttachmentDownload(): { download: (id: string) => void; downloading: ReadonlySet<string> } {
  const notify = useToast();
  const [downloading, setDownloading] = useState<ReadonlySet<string>>(() => new Set());
  const download = useCallback(
    (id: string) => {
      setDownloading((current) => new Set(current).add(id));
      downloadAttachment(id)
        .catch((caught) => notify(toAidexError(caught).message, "danger"))
        .finally(() =>
          setDownloading((current) => {
            const next = new Set(current);
            next.delete(id);
            return next;
          }),
        );
    },
    [notify],
  );
  return { download, downloading };
}
