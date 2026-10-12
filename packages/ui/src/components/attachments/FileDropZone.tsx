"use client";

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import { DropOverlay } from "./DropOverlay";
import { useFileDrop } from "./useFileDrop";
import styles from "./FileDropZone.module.css";

/** True inside a `FileDropZone`: a `Composer` there leaves drops to the zone. */
export const FileDropZoneContext = createContext(false);

/** Whether the caller renders inside a `FileDropZone`. */
export function useInsideFileDropZone(): boolean {
  return useContext(FileDropZoneContext);
}

export type FileDropZoneProps = {
  /** Files dropped on the zone, in drop order. Folders arrive as unreadable entries for the app to refuse. */
  onFiles: (files: File[]) => void;
  /** Overlay text (default "Drop files to attach"). */
  label?: string;
  /** Ignore drags entirely (the window-level guard still stops navigation). */
  disabled?: boolean;
  /** Show the overlay with this text and refuse drops, e.g. "Attachments work with AiDEX native models". */
  disabledReason?: string;
  children: ReactNode;
};

type Box = { top: number; left: number; width: number; height: number };

/** Inset of the highlight from the zone's visible edges, in pixels. */
const OVERLAY_INSET = 8;

function sameBox(a: Box | null, b: Box | null): boolean {
  if (a === null || b === null) return a === b;
  return a.top === b.top && a.left === b.left && a.width === b.width && a.height === b.height;
}

/**
 * The visible part of the zone: the union of its children's boxes, clipped by every scrolling or
 * clipping ancestor and the window. The zone itself has no box (`display: contents`).
 */
function visibleBox(zone: HTMLElement): Box | null {
  let top = Infinity;
  let left = Infinity;
  let right = -Infinity;
  let bottom = -Infinity;
  for (const child of Array.from(zone.children)) {
    if (child.hasAttribute("data-aidex-drop-overlay")) continue;
    const rect = child.getBoundingClientRect();
    if (rect.width === 0 && rect.height === 0) continue;
    top = Math.min(top, rect.top);
    left = Math.min(left, rect.left);
    right = Math.max(right, rect.right);
    bottom = Math.max(bottom, rect.bottom);
  }
  if (!Number.isFinite(top)) return null;
  for (let element = zone.parentElement; element; element = element.parentElement) {
    const style = getComputedStyle(element);
    if (style.overflowX === "visible" && style.overflowY === "visible") continue;
    const rect = element.getBoundingClientRect();
    top = Math.max(top, rect.top);
    left = Math.max(left, rect.left);
    right = Math.min(right, rect.right);
    bottom = Math.min(bottom, rect.bottom);
  }
  const viewportWidth = document.documentElement.clientWidth || window.innerWidth;
  top = Math.max(top, 0);
  left = Math.max(left, 0);
  right = Math.min(right, viewportWidth);
  bottom = Math.min(bottom, window.innerHeight);
  const width = right - left - 2 * OVERLAY_INSET;
  const height = bottom - top - 2 * OVERLAY_INSET;
  if (width <= 0 || height <= 0) return null;
  return {
    top: Math.round(top + OVERLAY_INSET),
    left: Math.round(left + OVERLAY_INSET),
    width: Math.round(width),
    height: Math.round(height),
  };
}

/**
 * Makes a region (for example a whole conversation page) accept dropped files. While files are
 * dragged over it, a dashed overlay with a paperclip and `label` covers its visible part. The zone
 * adds no box of its own, so it does not change the layout it wraps. Do not nest zones.
 */
export function FileDropZone({
  onFiles,
  label = "Drop files to attach",
  disabled = false,
  disabledReason,
  children,
}: FileDropZoneProps) {
  const zoneRef = useRef<HTMLDivElement>(null);
  const [box, setBox] = useState<Box | null>(null);
  const { active, handlers } = useFileDrop<HTMLDivElement>({
    enabled: !disabled && disabledReason === undefined,
    blockedReason: disabled ? undefined : disabledReason,
    onFiles,
  });

  const measure = useCallback(() => {
    const zone = zoneRef.current;
    const next = zone ? visibleBox(zone) : null;
    setBox((previous) => (sameBox(previous, next) ? previous : next));
  }, []);

  // Measure before the overlay paints, then follow scrolling and resizing while it shows.
  useLayoutEffect(() => {
    if (active) measure();
  }, [active, measure]);
  useEffect(() => {
    if (!active) return;
    let frame = 0;
    const schedule = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(measure);
    };
    window.addEventListener("resize", schedule);
    window.addEventListener("scroll", schedule, true);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("resize", schedule);
      window.removeEventListener("scroll", schedule, true);
    };
  }, [active, measure]);

  const style: CSSProperties | undefined = box
    ? { top: box.top, left: box.left, width: box.width, height: box.height }
    : undefined;

  return (
    <FileDropZoneContext.Provider value>
      <div ref={zoneRef} className={styles.zone} data-aidex-drop-zone="" {...handlers}>
        {children}
        {active && style ? (
          <DropOverlay
            placement="fixed"
            label={label}
            blockedReason={disabled ? undefined : disabledReason}
            style={style}
          />
        ) : null}
      </div>
    </FileDropZoneContext.Provider>
  );
}
