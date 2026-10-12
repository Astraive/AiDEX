"use client";

import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type MouseEvent,
} from "react";
import {
  ChevronLeftIcon,
  ChevronRightIcon,
  ImageIcon,
  LoaderIcon,
  XIcon,
  ZoomInIcon,
  ZoomOutIcon,
} from "../../icons/interface";
import { IconButton } from "../../primitives/IconButton";
import { containFileDrag } from "./fileDropGuard";
import { safeImageDataUrl } from "./model";
import styles from "./ImageLightbox.module.css";

export type LightboxImage = {
  /** Shown in the caption and used as the image's alternative text. */
  name: string;
  /** A `data:image/...` URL; any other URL shows as unavailable. Leave unset while it loads. */
  src?: string | null;
  /** The image is still loading: shows a spinner instead of "Image unavailable." */
  loading?: boolean;
};

export type ImageLightboxProps = {
  open: boolean;
  images: readonly LightboxImage[];
  /** The image shown; wraps around at both ends. */
  index: number;
  onIndexChange: (index: number) => void;
  onClose: () => void;
  /** Accessible name of the dialog (default "Image preview"). */
  label?: string;
};

/** Pixels a zoomed image scrolls per arrow key. */
const PAN_STEP = 48;

const PAN: Record<string, [number, number]> = {
  ArrowLeft: [-PAN_STEP, 0],
  ArrowRight: [PAN_STEP, 0],
  ArrowUp: [0, -PAN_STEP],
  ArrowDown: [0, PAN_STEP],
};

/** Zoomed size in pixels, the image it belongs to, and the point (0 to 1) to center in view. */
type Zoom = { key: string; width: number; height: number; focusX: number; focusY: number };

function wrap(index: number, count: number): number {
  return count > 0 ? ((index % count) + count) % count : 0;
}

/**
 * Full-window image viewer: a native modal dialog whose close button takes focus on open; Escape or
 * a click outside the image closes it, and focus returns to the opener. Prev/Next and ArrowLeft/
 * ArrowRight wrap around; a click on the image (or the zoom button) toggles between fit and 2x, and
 * arrow keys pan while zoomed. Only `data:` image URLs are displayed.
 */
export function ImageLightbox({ open, images, index, onIndexChange, onClose, label = "Image preview" }: ImageLightboxProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const viewportRef = useRef<HTMLDivElement>(null);
  const imageRef = useRef<HTMLImageElement>(null);
  const openerRef = useRef<Element | null>(null);
  const visibleRef = useRef(false);
  const [zoom, setZoom] = useState<Zoom | null>(null);
  const [failedSrc, setFailedSrc] = useState<string | null>(null);

  const count = images.length;
  const visible = open && count > 0;
  const current = wrap(index, count);
  const item = visible ? images[current] : undefined;
  const src = safeImageDataUrl(item?.src);
  // State that belongs to another image (or an earlier opening) no longer applies.
  const imageKey = `${current}:${src ?? ""}`;
  const activeZoom = zoom && zoom.key === imageKey ? zoom : null;
  const showImage = src !== undefined && failedSrc !== src;
  const several = count > 1;

  useEffect(() => {
    visibleRef.current = visible;
    const dialog = dialogRef.current;
    if (!dialog) return;
    if (visible && !dialog.open) {
      openerRef.current = document.activeElement;
      dialog.showModal();
      closeRef.current?.focus({ preventScroll: true });
    }
    if (!visible && dialog.open) dialog.close();
  }, [visible]);

  // Return focus to the opener however the viewer closes, including when it unmounts while open.
  useEffect(() => {
    if (!visible) return;
    return () => {
      setZoom(null);
      const opener = openerRef.current;
      openerRef.current = null;
      if (opener instanceof HTMLElement && opener.isConnected) opener.focus({ preventScroll: true });
    };
  }, [visible]);

  useLayoutEffect(() => {
    const viewport = viewportRef.current;
    if (!viewport || !activeZoom) return;
    viewport.scrollLeft = activeZoom.focusX * activeZoom.width - viewport.clientWidth / 2;
    viewport.scrollTop = activeZoom.focusY * activeZoom.height - viewport.clientHeight / 2;
  }, [activeZoom]);

  const navigate = (step: 1 | -1) => onIndexChange(wrap(current + step, count));

  const toggleZoom = (focusX = 0.5, focusY = 0.5) => {
    if (activeZoom) {
      setZoom(null);
      return;
    }
    const rect = imageRef.current?.getBoundingClientRect();
    if (!rect || rect.width === 0 || rect.height === 0) return;
    setZoom({ key: imageKey, width: Math.round(rect.width * 2), height: Math.round(rect.height * 2), focusX, focusY });
  };

  const onImageClick = (event: MouseEvent<HTMLImageElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    toggleZoom((event.clientX - rect.left) / rect.width, (event.clientY - rect.top) / rect.height);
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDialogElement>) => {
    if (!visible || event.defaultPrevented || event.altKey || event.ctrlKey || event.metaKey) return;
    const pan = PAN[event.key];
    const viewport = viewportRef.current;
    if (activeZoom && pan && viewport) {
      event.preventDefault();
      viewport.scrollBy({ left: pan[0], top: pan[1] });
      return;
    }
    if (!several) return;
    if (event.key === "ArrowLeft") {
      event.preventDefault();
      navigate(-1);
    } else if (event.key === "ArrowRight") {
      event.preventDefault();
      navigate(1);
    }
  };

  return (
    <dialog
      ref={dialogRef}
      className={styles.dialog}
      aria-label={label}
      onCancel={(event) => {
        event.preventDefault();
        onClose();
      }}
      onClose={() => {
        // The browser can close a modal itself (a repeated Escape skips `cancel`); keep the owner in sync.
        if (visibleRef.current) onClose();
      }}
      onKeyDown={onKeyDown}
      // The viewer is modal: files dragged over it never reach the composer or page behind it.
      onDragEnter={containFileDrag}
      onDragOver={containFileDrag}
      onDrop={containFileDrag}
      onClick={(event) => {
        if (event.target instanceof HTMLElement && event.target.dataset.lightboxBackdrop !== undefined) onClose();
      }}
    >
      {item ? (
        <div className={styles.frame} data-lightbox-backdrop="">
          <div className={styles.toolbar} data-lightbox-backdrop="">
            {showImage ? (
              <IconButton
                variant="soft"
                size="sm"
                label={activeZoom ? "Fit image to window" : "Zoom image to 2x"}
                aria-pressed={activeZoom !== null}
                icon={activeZoom ? <ZoomOutIcon /> : <ZoomInIcon />}
                onClick={() => toggleZoom()}
              />
            ) : null}
            <IconButton ref={closeRef} variant="soft" size="sm" label="Close image preview" icon={<XIcon />} onClick={onClose} />
          </div>
          <div className={styles.stage} data-lightbox-backdrop="">
            {several ? (
              <IconButton
                variant="soft"
                shape="circle"
                label="Previous image"
                icon={<ChevronLeftIcon />}
                onClick={() => navigate(-1)}
              />
            ) : null}
            <div
              ref={viewportRef}
              className={styles.viewport}
              data-lightbox-backdrop=""
              data-zoomed={activeZoom ? "true" : undefined}
            >
              {showImage ? (
                <img
                  key={imageKey}
                  ref={imageRef}
                  className={styles.image}
                  src={src}
                  alt={item.name}
                  draggable={false}
                  style={activeZoom ? { width: activeZoom.width, height: activeZoom.height } : undefined}
                  onClick={onImageClick}
                  onError={() => setFailedSrc(src)}
                />
              ) : item.loading && src === undefined ? (
                <p className={styles.state} role="status">
                  <LoaderIcon size={18} className={styles.spinner} />
                  Loading image…
                </p>
              ) : (
                <p className={styles.state} role="status">
                  <ImageIcon size={18} />
                  Image unavailable.
                </p>
              )}
            </div>
            {several ? (
              <IconButton
                variant="soft"
                shape="circle"
                label="Next image"
                icon={<ChevronRightIcon />}
                onClick={() => navigate(1)}
              />
            ) : null}
          </div>
          <p className={styles.caption} aria-live="polite" aria-atomic="true" data-lightbox-backdrop="">
            <span className={styles.captionText}>{item.name}</span>
            {several ? <span className={styles.captionCount}>{` (${current + 1}/${count})`}</span> : null}
          </p>
        </div>
      ) : null}
    </dialog>
  );
}
