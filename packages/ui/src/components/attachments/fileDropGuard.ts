"use client";

import { useEffect, type DragEvent as ReactDragEvent } from "react";
import { isFileDrag } from "./model";

function hasType(types: ArrayLike<string>, type: string): boolean {
  for (let index = 0; index < types.length; index += 1) {
    if (types[index] === type) return true;
  }
  return false;
}

function isEditable(target: EventTarget | null): boolean {
  if (!(target instanceof Element)) return false;
  if (target instanceof HTMLElement && target.isContentEditable) return true;
  return target.closest("input, textarea") !== null;
}

/**
 * Stops dropped files (and links dropped outside a text field) from navigating the window. Drop
 * targets such as `FileDropZone` and `Composer` cancel `dragover` themselves; any drag over the rest
 * of the page is refused (`dropEffect = "none"`), and every file drop is cancelled. Returns the
 * function that removes the listeners.
 */
export function installFileDropGuard(target: Window = window): () => void {
  const guard = (event: DragEvent) => {
    const types = event.dataTransfer?.types;
    if (!types) return;
    const files = isFileDrag(types);
    const link = !files && hasType(types, "text/uri-list") && !isEditable(event.target);
    if (!files && !link) return;
    if (event.type === "drop") {
      event.preventDefault();
      return;
    }
    // A drop target below already accepted this drag.
    if (event.defaultPrevented) return;
    event.preventDefault();
    if (event.dataTransfer) event.dataTransfer.dropEffect = "none";
  };
  target.addEventListener("dragenter", guard);
  target.addEventListener("dragover", guard);
  target.addEventListener("drop", guard);
  return () => {
    target.removeEventListener("dragenter", guard);
    target.removeEventListener("dragover", guard);
    target.removeEventListener("drop", guard);
  };
}

/**
 * Installs `installFileDropGuard` for the lifetime of the calling component. Call it once at the app
 * root, so a file dropped on any screen never replaces the app with the file.
 */
export function useFileDropGuard(): void {
  useEffect(() => installFileDropGuard(window), []);
}

/**
 * `onDragEnter`, `onDragOver`, and `onDrop` handler for a modal surface (`Dialog`, `ImageLightbox`).
 * A file drag over the modal never reaches a drop target behind it (such as a page-level
 * `FileDropZone`, which would attach files to a hidden composer) and is refused (`dropEffect =
 * "none"`); a file drop never navigates the window. A drop target nested inside the modal still
 * works: it handles the event first, and once it has cancelled the drag its `dropEffect` is kept.
 */
export function containFileDrag(event: ReactDragEvent<Element>): void {
  if (!isFileDrag(event.dataTransfer.types)) return;
  event.stopPropagation();
  if (event.type === "drop") {
    event.preventDefault();
    return;
  }
  if (event.defaultPrevented) return;
  event.preventDefault();
  event.dataTransfer.dropEffect = "none";
}
