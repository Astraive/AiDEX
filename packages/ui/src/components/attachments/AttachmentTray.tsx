"use client";

import { useEffect, useId, useState, type ReactNode } from "react";
import { FileCodeIcon, FileIcon } from "../../icons/development";
import { AlertTriangleIcon, ImageIcon, RefreshIcon, XIcon } from "../../icons/interface";
import { FileTextIcon } from "../../icons/objects";
import { IconButton } from "../../primitives/IconButton";
import { cx } from "../../utils/cx";
import { formatAttachmentSize } from "../../utils/format";
import a11y from "../content/a11y.module.css";
import { ImageLightbox, type LightboxImage } from "./ImageLightbox";
import {
  attachmentKindLabel,
  normalizeAttachmentKind,
  progressPercent,
  safeImageDataUrl,
  type AttachmentKind,
} from "./model";
import styles from "./AttachmentTray.module.css";

/** `pending` waits to upload (e.g. until the conversation exists); `ready` is uploaded. */
export type AttachmentUploadStatus = "pending" | "uploading" | "ready" | "failed";

export type AttachmentTrayItem = {
  id: string;
  name: string;
  /** Images show as thumbnails; every other kind as a row. Unknown kinds read as "File". */
  kind: AttachmentKind | (string & {});
  /** Size in bytes, shown as "3.2 MB" or "48 KB". */
  size?: number;
  /** Thumbnail for an image: a `data:image/...` URL. Other URLs are ignored (the release CSP allows `data:` images only). */
  previewUrl?: string;
  status?: AttachmentUploadStatus;
  /** Upload progress from 0 to 1 while `uploading`. */
  progress?: number;
  /** Why the upload failed; the retry button's tooltip. */
  error?: string;
  /** Replaces a row's status text, e.g. "Converting…" or "Remove to send". */
  statusText?: string;
};

export type AttachmentTrayProps = {
  items: readonly AttachmentTrayItem[];
  /** Adds an X button to every attachment (`Remove <name>`). */
  onRemove?: (id: string) => void;
  /** Adds a retry button to failed uploads (`Retry upload for <name>`). */
  onRetry?: (id: string) => void;
  /** Replaces the built-in image preview (an `ImageLightbox` over the tray's images). */
  onPreview?: (id: string) => void;
  /** Makes file-row names buttons, e.g. to open a text preview. */
  onOpen?: (id: string) => void;
  /** Disables remove and retry, e.g. while the message is being sent. */
  disabled?: boolean;
  /** Accessible name of the group (default "Attachments"). */
  label?: string;
  className?: string;
};

const ROW_ICON: Record<AttachmentKind, ReactNode> = {
  image: <ImageIcon />,
  pdf: <FileTextIcon />,
  text: <FileCodeIcon />,
  file: <FileIcon />,
};

function failureText(item: AttachmentTrayItem): string {
  return item.error?.trim() || "Upload failed";
}

/**
 * Pending attachments above the composer text: images as 64px thumbnails (preview, remove, upload
 * progress, retry), other files as rows (kind icon, name, size or status, retry, remove). Upload
 * failures are announced politely.
 */
export function AttachmentTray({
  items,
  onRemove,
  onRetry,
  onPreview,
  onOpen,
  disabled = false,
  label = "Attachments",
  className,
}: AttachmentTrayProps) {
  const baseId = useId();
  const [lightboxIndex, setLightboxIndex] = useState<number | null>(null);
  const images = items.filter((item) => normalizeAttachmentKind(item.kind) === "image");
  const files = items.filter((item) => normalizeAttachmentKind(item.kind) !== "image");
  const failed = items.filter((item) => item.status === "failed");
  const lightboxImages: LightboxImage[] = images.map((item) => ({
    name: item.name,
    src: safeImageDataUrl(item.previewUrl),
  }));
  // Forget an open preview once its images are gone, so new ones never open it by themselves.
  const hasImages = images.length > 0;
  useEffect(() => {
    if (!hasImages) setLightboxIndex(null);
  }, [hasImages]);

  const openPreview = (id: string) => {
    if (onPreview) {
      onPreview(id);
      return;
    }
    const index = images.findIndex((item) => item.id === id);
    if (index >= 0) setLightboxIndex(index);
  };

  if (items.length === 0) return null;

  return (
    <div className={cx(styles.tray, className)} role="group" aria-label={label}>
      {images.length > 0 ? (
        <ul className={styles.tiles}>
          {images.map((item, index) => {
            const src = safeImageDataUrl(item.previewUrl);
            const isFailed = item.status === "failed";
            const percent = progressPercent(item.progress);
            const errorId = `${baseId}-error-${index}`;
            return (
              <li key={item.id} className={cx(styles.tile, isFailed && styles.tileFailed)}>
                {src ? (
                  <button
                    type="button"
                    className={styles.preview}
                    aria-label={`Preview ${item.name}`}
                    title={item.name}
                    onClick={() => openPreview(item.id)}
                  >
                    <img className={styles.thumb} src={src} alt="" draggable={false} />
                  </button>
                ) : (
                  <span className={styles.placeholder} title={item.name}>
                    <ImageIcon size={16} />
                    <span className={styles.placeholderName}>{item.name}</span>
                  </span>
                )}
                {item.status === "uploading" ? (
                  <span
                    className={styles.progress}
                    role="progressbar"
                    aria-label={`Uploading ${item.name}`}
                    aria-valuemin={0}
                    aria-valuemax={100}
                    aria-valuenow={percent}
                  >
                    <span className={styles.progressFill} style={{ width: `${percent}%` }} />
                    <span className={styles.progressText}>{percent}%</span>
                  </span>
                ) : null}
                {isFailed ? (
                  <>
                    <span id={errorId} className={a11y.visuallyHidden}>
                      {failureText(item)}
                    </span>
                    {onRetry ? (
                      <button
                        type="button"
                        className={cx(styles.tileButton, styles.retry)}
                        aria-label={`Retry upload for ${item.name}`}
                        aria-describedby={errorId}
                        title={failureText(item)}
                        disabled={disabled}
                        onClick={() => onRetry(item.id)}
                      >
                        <RefreshIcon size={12} strokeWidth={2.25} />
                      </button>
                    ) : (
                      <span className={cx(styles.tileButton, styles.retry, styles.alert)} title={failureText(item)}>
                        <AlertTriangleIcon size={12} strokeWidth={2.25} />
                      </span>
                    )}
                  </>
                ) : null}
                {onRemove ? (
                  <button
                    type="button"
                    className={cx(styles.tileButton, styles.remove)}
                    aria-label={`Remove ${item.name}`}
                    title={`Remove ${item.name}`}
                    disabled={disabled}
                    onClick={() => onRemove(item.id)}
                  >
                    <XIcon size={12} strokeWidth={2.25} />
                  </button>
                ) : null}
              </li>
            );
          })}
        </ul>
      ) : null}
      {files.length > 0 ? (
        <ul className={styles.rows}>
          {files.map((item, index) => {
            const kind = normalizeAttachmentKind(item.kind);
            const isFailed = item.status === "failed";
            const uploading = item.status === "uploading";
            const percent = progressPercent(item.progress);
            const errorId = `${baseId}-row-error-${index}`;
            const size = item.size !== undefined ? formatAttachmentSize(item.size) : "";
            const facts = [attachmentKindLabel(kind), size].filter(Boolean).join(" · ");
            const status = item.statusText ?? (uploading ? `${percent}%` : isFailed ? "Upload failed" : facts);
            return (
              <li key={item.id} className={cx(styles.row, isFailed && styles.rowFailed)}>
                <span className={styles.rowIcon} aria-hidden="true">
                  {ROW_ICON[kind]}
                </span>
                {onOpen ? (
                  <button type="button" className={styles.rowName} title={item.name} onClick={() => onOpen(item.id)}>
                    {item.name}
                  </button>
                ) : (
                  <span className={styles.rowName} title={item.name}>
                    {item.name}
                  </span>
                )}
                {uploading ? (
                  <span
                    className={styles.rowStatus}
                    role="progressbar"
                    aria-label={`Uploading ${item.name}`}
                    aria-valuemin={0}
                    aria-valuemax={100}
                    aria-valuenow={percent}
                  >
                    {status}
                  </span>
                ) : (
                  <span className={cx(styles.rowStatus, isFailed && styles.rowStatusFailed)}>{status}</span>
                )}
                {isFailed ? (
                  <span id={errorId} className={a11y.visuallyHidden}>
                    {failureText(item)}
                  </span>
                ) : null}
                {isFailed && onRetry ? (
                  <IconButton
                    size="sm"
                    label={`Retry upload for ${item.name}`}
                    title={failureText(item)}
                    aria-describedby={errorId}
                    icon={<RefreshIcon />}
                    disabled={disabled}
                    onClick={() => onRetry(item.id)}
                  />
                ) : null}
                {onRemove ? (
                  <IconButton
                    size="sm"
                    label={`Remove ${item.name}`}
                    icon={<XIcon />}
                    disabled={disabled}
                    onClick={() => onRemove(item.id)}
                  />
                ) : null}
              </li>
            );
          })}
        </ul>
      ) : null}
      <span className={a11y.visuallyHidden} role="status">
        {failed.length > 0 ? `Upload failed for ${failed.map((item) => item.name).join(", ")}.` : ""}
      </span>
      <ImageLightbox
        open={lightboxIndex !== null && lightboxImages.length > 0}
        images={lightboxImages}
        index={Math.min(lightboxIndex ?? 0, Math.max(0, lightboxImages.length - 1))}
        onIndexChange={setLightboxIndex}
        onClose={() => setLightboxIndex(null)}
      />
    </div>
  );
}
