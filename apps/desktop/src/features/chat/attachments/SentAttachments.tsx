import { MessageAttachments, type MessageAttachmentItem } from "@aidex/ui/components";
import type { AttachmentRef } from "@aidex/client/dto/attachments";
import { useAttachmentDownload } from "./attachmentDownload";
import { requestAttachmentImage, setAttachmentImageShown, useAttachmentImages } from "./attachmentImages";

/**
 * The attachments of a sent user message: images load lazily (through `attachment.read` into `data:`
 * URLs, cached per id, kept while on screen) and open in the image viewer; other files download.
 */
export function SentAttachments({ attachments }: { attachments: readonly AttachmentRef[] }) {
  const image = useAttachmentImages();
  const { download, downloading } = useAttachmentDownload();
  const items: MessageAttachmentItem[] = attachments.map((attachment) => {
    const base = {
      id: attachment.id,
      name: attachment.name,
      kind: attachment.kind,
      size: attachment.size,
      pages: attachment.pages,
      note: attachment.excluded ? "Not sent to models" : undefined,
    };
    if (attachment.kind !== "image") return { ...base, status: downloading.has(attachment.id) ? "loading" : undefined };
    const cached = image(attachment.id);
    if (cached?.status === "ready") return { ...base, src: cached.src, status: "ready" };
    return { ...base, status: cached?.status };
  });
  const mimes = new Map(attachments.map((attachment) => [attachment.id, attachment.mime]));
  return (
    <MessageAttachments
      items={items}
      onRequestImage={(id) => requestAttachmentImage(id, mimes.get(id) ?? "image/png")}
      onImageVisibilityChange={setAttachmentImageShown}
      onDownload={download}
    />
  );
}
