"use client";

import { Dialog as BaseDialog } from "@base-ui/react/dialog";
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type MouseEvent,
  type PointerEvent,
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
import { LAYER_ATTRIBUTE, pushLayer } from "../../primitives/layers";
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
const MAX_ZOOM = 8;
/** Zoom factor per +/- key press. */
const ZOOM_STEP = 1.5;
/** Pointer travel (px) after which a press counts as a drag, not a click. */
const DRAG_THRESHOLD = 4;

const PAN: Record<string, [number, number]> = {
  ArrowLeft: [-PAN_STEP, 0],
  ArrowRight: [PAN_STEP, 0],
  ArrowUp: [0, -PAN_STEP],
  ArrowDown: [0, PAN_STEP],
};

/** The zoom of one image: its fitted size, the scale over it, and where to keep the focus point. */
type Zoom = { key: string; scale: number; baseWidth: number; baseHeight: number };
/** The image point (0 to 1) to keep under a viewport point (px from the viewport's top left). */
type ZoomAnchor = { fx: number; fy: number; vx: number; vy: number };

function wrap(index: number, count: number): number {
  return count > 0 ? ((index % count) + count) % count : 0;
}

function clampScale(scale: number): number {
  return Math.min(MAX_ZOOM, Math.max(1, scale));
}

/**
 * Full-window image viewer on a modal dialog (above other dialogs): the close button takes focus
 * on open; Escape or a click outside the image closes it, and focus returns to the opener.
 * Prev/Next and ArrowLeft/ArrowRight wrap around. Zoom goes from fit to 8x: a click on the image (or
 * the zoom button) toggles fit and 2x, the wheel zooms around the pointer, and + / - / 0 zoom in,
 * out and back to fit. While zoomed, drag or the arrow keys pan. Only `data:` image URLs are shown.
 */
export function ImageLightbox({ open, images, index, onIndexChange, onClose, label = "Image preview" }: ImageLightboxProps) {
  const [popup, setPopup] = useState<HTMLDivElement | null>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const viewportRef = useRef<HTMLDivElement | null>(null);
  // Also kept as state: the viewport mounts after the dialog opens, and the wheel listener needs it.
  const [viewportElement, setViewportElement] = useState<HTMLDivElement | null>(null);
  const setViewport = useCallback((element: HTMLDivElement | null) => {
    viewportRef.current = element;
    setViewportElement(element);
  }, []);
  const imageRef = useRef<HTMLImageElement>(null);
  const anchorRef = useRef<ZoomAnchor | null>(null);
  const dragRef = useRef<{ pointerId: number; x: number; y: number; left: number; top: number } | null>(null);
  const draggedRef = useRef(false);
  const [dragging, setDragging] = useState(false);
  const [zoom, setZoom] = useState<Zoom | null>(null);
  const [failedSrc, setFailedSrc] = useState<string | null>(null);

  const count = images.length;
  const visible = open && count > 0;
  const current = wrap(index, count);
  const item = visible ? images[current] : undefined;
  const src = safeImageDataUrl(item?.src);
  // State that belongs to another image (or an earlier opening) no longer applies.
  const imageKey = `${current}:${src ?? ""}`;
  const activeZoom = zoom && zoom.key === imageKey && zoom.scale > 1 ? zoom : null;
  const showImage = src !== undefined && failedSrc !== src;
  const several = count > 1;

  // A modal layer while open: the shell's shortcuts stand down.
  useLayoutEffect(() => {
    if (!visible || !popup) return;
    return pushLayer(popup, { modal: true });
  }, [visible, popup]);

  // Closing (or showing another image) starts the next one at fit.
  useEffect(() => {
    if (!visible) setZoom(null);
  }, [visible]);

  /** Zooms to `scale`, keeping the image point under `point` (default the viewport center) in place. */
  const zoomTo = useCallback(
    (scale: number, point?: { x: number; y: number }) => {
      const viewport = viewportRef.current;
      const image = imageRef.current;
      if (!viewport || !image) return;
      const next = clampScale(scale);
      const rect = image.getBoundingClientRect();
      const bounds = viewport.getBoundingClientRect();
      if (rect.width === 0 || rect.height === 0) return;
      const x = point?.x ?? bounds.left + viewport.clientWidth / 2;
      const y = point?.y ?? bounds.top + viewport.clientHeight / 2;
      anchorRef.current = {
        fx: Math.min(1, Math.max(0, (x - rect.left) / rect.width)),
        fy: Math.min(1, Math.max(0, (y - rect.top) / rect.height)),
        vx: x - bounds.left,
        vy: y - bounds.top,
      };
      setZoom((previous) => {
        const base =
          previous && previous.key === imageKey && previous.scale > 1
            ? { baseWidth: previous.baseWidth, baseHeight: previous.baseHeight }
            : { baseWidth: rect.width, baseHeight: rect.height };
        return { key: imageKey, scale: next, ...base };
      });
    },
    [imageKey],
  );

  // Keep the anchored image point under the pointer (or the center) after the size changes.
  useLayoutEffect(() => {
    const viewport = viewportRef.current;
    const anchor = anchorRef.current;
    if (!viewport || !anchor) return;
    anchorRef.current = null;
    if (!activeZoom) return;
    viewport.scrollLeft = anchor.fx * activeZoom.baseWidth * activeZoom.scale - anchor.vx;
    viewport.scrollTop = anchor.fy * activeZoom.baseHeight * activeZoom.scale - anchor.vy;
  }, [activeZoom]);

  // The fitted size changes with the window; start again from fit.
  useEffect(() => {
    if (!visible) return;
    const reset = () => setZoom(null);
    window.addEventListener("resize", reset);
    return () => window.removeEventListener("resize", reset);
  }, [visible]);

  // The wheel zooms around the pointer (a passive listener could not stop the page from scrolling).
  const scale = activeZoom?.scale ?? 1;
  const scaleRef = useRef(scale);
  useLayoutEffect(() => {
    scaleRef.current = scale;
  });
  useEffect(() => {
    const viewport = viewportElement;
    if (!visible || !showImage || !viewport) return;
    const onWheel = (event: WheelEvent) => {
      if (event.deltaY === 0) return;
      event.preventDefault();
      const unit = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? viewport.clientHeight : 1;
      const factor = Math.exp(-event.deltaY * unit * (event.ctrlKey ? 0.01 : 0.002));
      zoomTo(scaleRef.current * factor, { x: event.clientX, y: event.clientY });
    };
    viewport.addEventListener("wheel", onWheel, { passive: false });
    return () => viewport.removeEventListener("wheel", onWheel);
  }, [visible, showImage, zoomTo, viewportElement]);

  const navigate = (step: 1 | -1) => onIndexChange(wrap(current + step, count));

  const toggleZoom = (point?: { x: number; y: number }) => zoomTo(activeZoom ? 1 : 2, point);

  const onImageClick = (event: MouseEvent<HTMLImageElement>) => {
    if (draggedRef.current) return;
    toggleZoom({ x: event.clientX, y: event.clientY });
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (!visible || event.defaultPrevented || event.altKey || event.ctrlKey || event.metaKey) return;
    if (showImage && (event.key === "+" || event.key === "=")) {
      event.preventDefault();
      zoomTo(scale * ZOOM_STEP);
      return;
    }
    if (showImage && (event.key === "-" || event.key === "_")) {
      event.preventDefault();
      zoomTo(scale / ZOOM_STEP);
      return;
    }
    if (showImage && event.key === "0") {
      event.preventDefault();
      zoomTo(1);
      return;
    }
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

  // Drag to pan while zoomed (mouse; touch pans natively).
  const onPointerDown = (event: PointerEvent<HTMLDivElement>) => {
    draggedRef.current = false;
    if (!activeZoom || event.pointerType !== "mouse" || event.button !== 0 || dragRef.current) return;
    const viewport = event.currentTarget;
    dragRef.current = {
      pointerId: event.pointerId,
      x: event.clientX,
      y: event.clientY,
      left: viewport.scrollLeft,
      top: viewport.scrollTop,
    };
    viewport.setPointerCapture(event.pointerId);
    setDragging(true);
  };

  const onPointerMove = (event: PointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    if (Math.hypot(event.clientX - drag.x, event.clientY - drag.y) > DRAG_THRESHOLD) draggedRef.current = true;
    event.currentTarget.scrollLeft = drag.left - (event.clientX - drag.x);
    event.currentTarget.scrollTop = drag.top - (event.clientY - drag.y);
  };

  const endDrag = (event: PointerEvent<HTMLDivElement>) => {
    if (dragRef.current?.pointerId !== event.pointerId) return;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    dragRef.current = null;
    setDragging(false);
  };

  return (
    <BaseDialog.Root
      open={visible}
      onOpenChange={(next) => {
        if (!next) onClose();
      }}
    >
      <BaseDialog.Portal>
        <BaseDialog.Backdrop className={styles.backdrop} />
        <BaseDialog.Popup
          ref={setPopup}
          className={styles.dialog}
          aria-label={label}
          initialFocus={closeRef}
          {...{ [LAYER_ATTRIBUTE]: "" }}
          onKeyDown={onKeyDown}
          // The viewer is modal: files dragged over it never reach the composer or page behind it.
          onDragEnter={containFileDrag}
          onDragOver={containFileDrag}
          onDrop={containFileDrag}
          onClick={(event) => {
            // A drag that ends outside the image is not a click on the backdrop.
            if (draggedRef.current) return;
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
                  ref={setViewport}
                  className={styles.viewport}
                  data-lightbox-backdrop=""
                  data-zoomed={activeZoom ? "true" : undefined}
                  data-dragging={dragging ? "true" : undefined}
                  onPointerDown={onPointerDown}
                  onPointerMove={onPointerMove}
                  onPointerUp={endDrag}
                  onPointerCancel={endDrag}
                  onLostPointerCapture={endDrag}
                >
                  {showImage ? (
                    <img
                      key={imageKey}
                      ref={imageRef}
                      className={styles.image}
                      src={src}
                      alt={item.name}
                      draggable={false}
                      style={
                        activeZoom
                          ? { width: activeZoom.baseWidth * activeZoom.scale, height: activeZoom.baseHeight * activeZoom.scale }
                          : undefined
                      }
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
        </BaseDialog.Popup>
      </BaseDialog.Portal>
    </BaseDialog.Root>
  );
}
