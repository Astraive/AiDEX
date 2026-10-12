"use client";

import { useCallback, useEffect, useRef, useState, type DragEvent } from "react";
import { isFileDrag } from "./model";

export type FileDropOptions = {
  /** Accept dropped files. */
  enabled: boolean;
  /** Show the overlay with this text and refuse the drop instead of accepting it. */
  blockedReason?: string;
  onFiles: (files: File[]) => void;
};

export type FileDropHandlers<T extends Element> = {
  onDragEnter: (event: DragEvent<T>) => void;
  onDragOver: (event: DragEvent<T>) => void;
  onDragLeave: (event: DragEvent<T>) => void;
  onDrop: (event: DragEvent<T>) => void;
};

/**
 * Drag-over state for a file drop target. Nested elements fire `dragenter`/`dragleave` pairs, so the
 * highlight follows a counter, and a `relatedTarget` outside the target ends it at once. A drop
 * anywhere, a `dragend`, or Escape also resets it, so a cancelled drag never leaves it stuck.
 */
export function useFileDrop<T extends Element>({ enabled, blockedReason, onFiles }: FileDropOptions) {
  const [active, setActive] = useState(false);
  const depth = useRef(0);
  const onFilesRef = useRef(onFiles);
  useEffect(() => {
    onFilesRef.current = onFiles;
  });

  const accepting = enabled || blockedReason !== undefined;
  const blocked = !enabled && blockedReason !== undefined;

  const reset = useCallback(() => {
    depth.current = 0;
    setActive(false);
  }, []);

  // Stop highlighting when the target stops taking files mid-drag.
  useEffect(() => {
    if (!accepting) reset();
  }, [accepting, reset]);

  useEffect(() => {
    if (!active) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") reset();
    };
    window.addEventListener("dragend", reset);
    window.addEventListener("drop", reset, true);
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("blur", reset);
    return () => {
      window.removeEventListener("dragend", reset);
      window.removeEventListener("drop", reset, true);
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("blur", reset);
    };
  }, [active, reset]);

  const handlers: FileDropHandlers<T> = {
    onDragEnter(event) {
      if (!accepting || !isFileDrag(event.dataTransfer.types)) return;
      event.preventDefault();
      depth.current += 1;
      setActive(true);
    },
    onDragOver(event) {
      if (!accepting || !isFileDrag(event.dataTransfer.types)) return;
      // Cancelling `dragover` is what makes the element a drop target; `none` refuses a blocked drop.
      event.preventDefault();
      event.dataTransfer.dropEffect = blocked ? "none" : "copy";
      if (!active) setActive(true);
    },
    onDragLeave(event) {
      if (!accepting || !isFileDrag(event.dataTransfer.types)) return;
      const next = event.relatedTarget;
      if (next instanceof Node && !event.currentTarget.contains(next)) {
        reset();
        return;
      }
      depth.current = Math.max(0, depth.current - 1);
      if (depth.current === 0) reset();
    },
    onDrop(event) {
      if (!accepting || !isFileDrag(event.dataTransfer.types)) return;
      // Always cancel, so the webview never navigates to a dropped file.
      event.preventDefault();
      reset();
      if (blocked) return;
      const files = Array.from(event.dataTransfer.files);
      if (files.length > 0) onFilesRef.current(files);
    },
  };

  return { active, blocked, handlers };
}
