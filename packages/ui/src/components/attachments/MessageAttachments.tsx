"use client";

import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { FileCodeIcon, FileIcon } from "../../icons/development";
import { DownloadIcon, ImageIcon, LoaderIcon } from "../../icons/interface";
import { FileTextIcon } from "../../icons/objects";
import { IconButton } from "../../primitives/IconButton";
import { Tooltip } from "../../primitives/Tooltip";
import { cx } from "../../utils/cx";
import { formatAttachmentSize } from "../../utils/format";
import { ImageLightbox } from "./ImageLightbox";
import {
  attachmentKindLabel,
  formatPageCount,
  normalizeAttachmentKind,
  safeImageDataUrl,
  type AttachmentKind,
} from "./model";
import styles from "./MessageAttachments.module.css";

export type MessageAttachmentItem = {
  id: string;
  name: string;
  /** Images show in a grid; every other kind as a row. Unknown kinds read as "File". */
  kind: AttachmentKind | (string & {});
  /** Size in bytes. */
  size?: number;
  /** PDF page count. */
  pages?: number;
  /** An image's `data:image/...` URL once loaded; other URLs are ignored. */
  src?: string;
  /**
   * `loading`: an image is being read, or a file is downloading (its button shows a spinner).
   * `error`: the image could not be loaded (its tile keeps the name).
   */
  status?: "loading" | "ready" | "error";
  /** Extra muted text for a row, e.g. "Not sent to models". */
  note?: string;
};

export type MessageAttachmentsProps = {
  items: readonly MessageAttachmentItem[];
  /**
   * Asks the app for an image's data URL: called when a tile without `src` scrolls into view (or
   * loses its `src` while in view) or the viewer shows it. It can repeat for the same id, so cache
   * by id.
   */
  onRequestImage?: (id: string) => void;
  /**
   * Reports when an image comes on screen (its tile within 200px of the viewport, or the viewer
   * showing it) and when it leaves. Every id reported `true` is reported `false` again, at the
   * latest when the component unmounts, so an app can keep on-screen images out of cache eviction.
   */
  onImageVisibilityChange?: (id: string, visible: boolean) => void;
  /** Adds a Download button to file rows (`Download <name>`). */
  onDownload?: (id: string) => void;
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

/**
 * Attachments of a sent message, for a transcript bubble: images in a two-column grid of 4:3 tiles
 * (at most 210px wide; a placeholder with the name while loading or after a failure; a click opens
 * the image viewer), other files as rows with kind, name, size, and a Download button.
 */
export function MessageAttachments({
  items,
  onRequestImage,
  onImageVisibilityChange,
  onDownload,
  label = "Attachments",
  className,
}: MessageAttachmentsProps) {
  const images = items.filter((item) => normalizeAttachmentKind(item.kind) === "image");
  const files = items.filter((item) => normalizeAttachmentKind(item.kind) !== "image");
  const [viewerIndex, setViewerIndex] = useState<number | null>(null);
  const tileRefs = useRef(new Map<string, HTMLElement>());
  const requestRef = useRef(onRequestImage);
  const visibilityRef = useRef(onImageVisibilityChange);
  const pendingRef = useRef(new Set<string>());
  // Tiles in view, the image the viewer shows, and the ids reported on screen (either of the two).
  const inViewRef = useRef(new Set<string>());
  const viewedRef = useRef<string | undefined>(undefined);
  const shownRef = useRef(new Set<string>());
  const observerRef = useRef<IntersectionObserver | null>(null);
  const observedRef = useRef(new Map<string, Element>());

  // Images still to ask for: no source yet, and not loading or failed.
  const pendingKey = images
    .filter((item) => !safeImageDataUrl(item.src) && item.status !== "loading" && item.status !== "error")
    .map((item) => item.id)
    .join("\n");
  useEffect(() => {
    requestRef.current = onRequestImage;
    visibilityRef.current = onImageVisibilityChange;
    pendingRef.current = new Set(pendingKey ? pendingKey.split("\n") : []);
  });

  // Reports a change in whether an image is on screen.
  const syncShown = useCallback((id: string) => {
    const shown = inViewRef.current.has(id) || viewedRef.current === id;
    if (shown === shownRef.current.has(id)) return;
    if (shown) shownRef.current.add(id);
    else shownRef.current.delete(id);
    visibilityRef.current?.(id, shown);
  }, []);

  // One observer for the component's lifetime watches every image tile: it tracks which are in
  // view and asks for those without a source as they come into view.
  useEffect(() => {
    if (typeof IntersectionObserver === "undefined") return;
    const inView = inViewRef.current;
    const observed = observedRef.current;
    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          const id = (entry.target as HTMLElement).dataset.attachmentId;
          if (!id || observed.get(id) !== entry.target) continue;
          if (entry.isIntersecting) inView.add(id);
          else inView.delete(id);
          syncShown(id);
          if (entry.isIntersecting && pendingRef.current.has(id)) requestRef.current?.(id);
        }
      },
      { rootMargin: "200px" },
    );
    observerRef.current = observer;
    return () => {
      observer.disconnect();
      observerRef.current = null;
      observed.clear();
      const ids = [...inView];
      inView.clear();
      ids.forEach(syncShown);
    };
  }, [syncShown]);

  // Keep the observer on the current tiles; a tile that went away is no longer on screen.
  useEffect(() => {
    const observer = observerRef.current;
    if (!observer) return;
    const observed = observedRef.current;
    const tiles = tileRefs.current;
    for (const [id, tile] of observed) {
      if (tiles.get(id) === tile) continue;
      observer.unobserve(tile);
      observed.delete(id);
      inViewRef.current.delete(id);
      syncShown(id);
    }
    for (const [id, tile] of tiles) {
      if (observed.get(id) === tile) continue;
      observed.set(id, tile);
      observer.observe(tile);
    }
  });

  // A tile in view that needs its image (again) asks for it now. Without an observer, every
  // pending image is asked for.
  useEffect(() => {
    if (!pendingKey || !requestRef.current) return;
    const watching = observerRef.current !== null;
    for (const id of pendingKey.split("\n")) {
      if (!watching || inViewRef.current.has(id)) requestRef.current?.(id);
    }
  }, [pendingKey]);

  const hasImages = images.length > 0;
  useEffect(() => {
    if (!hasImages) setViewerIndex(null);
  }, [hasImages]);

  // The viewer asks for the image it shows when the grid has not asked yet. A failed image is not
  // asked again here, so a persistent failure cannot turn into a request loop.
  const viewed = viewerIndex !== null ? images[viewerIndex] : undefined;
  const viewedId =
    viewed && !safeImageDataUrl(viewed.src) && viewed.status !== "loading" && viewed.status !== "error"
      ? viewed.id
      : undefined;
  const shownInViewer = viewed?.id;
  useEffect(() => {
    if (!shownInViewer) return;
    viewedRef.current = shownInViewer;
    syncShown(shownInViewer);
    return () => {
      viewedRef.current = undefined;
      syncShown(shownInViewer);
    };
  }, [shownInViewer, syncShown]);
  useEffect(() => {
    if (viewedId) requestRef.current?.(viewedId);
  }, [viewedId]);

  if (items.length === 0) return null;

  return (
    <div className={cx(styles.attachments, className)} role="group" aria-label={label}>
      {images.length > 0 ? (
        <ul className={cx(styles.grid, images.length === 1 && styles.single)}>
          {images.map((item, index) => {
            const src = safeImageDataUrl(item.src);
            return (
              <li
                key={item.id}
                className={styles.tile}
                data-attachment-id={item.id}
                ref={(node) => {
                  if (node) tileRefs.current.set(item.id, node);
                  else tileRefs.current.delete(item.id);
                }}
              >
                <Tooltip
                  content={item.status === "error" ? `${item.name}: image unavailable` : item.name}
                  describe={item.status === "error"}
                >
                  <button
                    type="button"
                    className={styles.tileButton}
                    aria-label={`Preview ${item.name}`}
                    onClick={() => setViewerIndex(index)}
                  >
                    {src ? (
                      <img className={styles.image} src={src} alt="" draggable={false} />
                    ) : (
                      <span className={styles.placeholder}>
                        {item.status === "loading" ? (
                          <LoaderIcon size={16} className={styles.spinner} />
                        ) : (
                          <ImageIcon size={16} />
                        )}
                        <span className={styles.placeholderName}>{item.name}</span>
                      </span>
                    )}
                  </button>
                </Tooltip>
              </li>
            );
          })}
        </ul>
      ) : null}
      {files.length > 0 ? (
        <ul className={styles.rows}>
          {files.map((item) => {
            const kind = normalizeAttachmentKind(item.kind);
            const facts = [
              attachmentKindLabel(kind),
              formatPageCount(item.pages),
              item.size !== undefined ? formatAttachmentSize(item.size) : "",
            ].filter(Boolean);
            const downloading = item.status === "loading";
            return (
              <li key={item.id} className={styles.row}>
                <span className={styles.rowIcon} aria-hidden="true">
                  {ROW_ICON[kind]}
                </span>
                <span className={styles.rowText}>
                  <Tooltip content={item.name} describe={false}>
                    <span className={styles.rowName}>{item.name}</span>
                  </Tooltip>
                  <span className={styles.rowMeta}>
                    {facts.join(" · ")}
                    {item.note ? <span className={styles.note}>{` · ${item.note}`}</span> : null}
                  </span>
                </span>
                {onDownload ? (
                  <IconButton
                    size="sm"
                    label={downloading ? `Downloading ${item.name}` : `Download ${item.name}`}
                    icon={downloading ? <LoaderIcon className={styles.spinner} /> : <DownloadIcon />}
                    aria-busy={downloading || undefined}
                    disabled={downloading}
                    onClick={() => onDownload(item.id)}
                  />
                ) : null}
              </li>
            );
          })}
        </ul>
      ) : null}
      <ImageLightbox
        open={viewerIndex !== null && images.length > 0}
        images={images.map((item) => ({
          name: item.name,
          src: safeImageDataUrl(item.src),
          loading: item.status === "loading" || (item.status !== "error" && onRequestImage !== undefined),
        }))}
        index={Math.min(viewerIndex ?? 0, Math.max(0, images.length - 1))}
        onIndexChange={setViewerIndex}
        onClose={() => setViewerIndex(null)}
      />
    </div>
  );
}
