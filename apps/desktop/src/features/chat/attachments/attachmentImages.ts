// Images of sent attachments as `data:` URLs (the release CSP allows no `blob:` images), read
// through `attachment.read` pages once and shared by every transcript view in this window. The
// cache is bounded, except that images on screen are never evicted (so tiles in view cannot evict
// each other in a reload loop); evicted images are read again when they are shown again.
import { useSyncExternalStore } from "react";
import { call } from "@aidex/client/rpc";
import type { AttachmentContent } from "@aidex/client/dto/attachments";
import { base64ToBytes } from "../artifacts/artifactContent";
import { bytesToBase64 } from "./attachmentFiles";

export type AttachmentImage =
  | { status: "loading" }
  | { status: "ready"; src: string }
  | { status: "error" };

type Entry = AttachmentImage;

/** Characters of `data:` URLs kept (about 48 MB of images). */
const CACHE_CHARS = 64 * 1024 * 1024;

const entries = new Map<string, Entry>();
/** How many views show each image now (`setAttachmentImageShown`); those are never evicted. */
const shown = new Map<string, number>();
const listeners = new Set<() => void>();
let version = 0;

function emit() {
  version += 1;
  listeners.forEach((listener) => listener());
}

function size(entry: Entry): number {
  return entry.status === "ready" ? entry.src.length : 0;
}

/**
 * Keeps the newest entries within the budget; Map order is least recently used first. Images on
 * screen stay, so the cache exceeds the budget only by what is on screen. Returns whether any went.
 */
function evict(): boolean {
  let total = 0;
  let evicted = false;
  entries.forEach((entry) => (total += size(entry)));
  for (const [id, entry] of entries) {
    if (total <= CACHE_CHARS) break;
    if (entry.status !== "ready" || shown.has(id)) continue;
    total -= size(entry);
    entries.delete(id);
    evicted = true;
  }
  return evicted;
}

function set(id: string, entry: Entry) {
  entries.delete(id);
  entries.set(id, entry);
  evict();
  emit();
}

/** Reads a whole attachment as bytes, page by page. */
export async function readAttachmentBytes(id: string): Promise<{ bytes: Uint8Array<ArrayBuffer>; text: boolean }> {
  const parts: Uint8Array[] = [];
  const texts: string[] = [];
  let offset: number | null = 0;
  let text = false;
  while (offset !== null) {
    const page: AttachmentContent = await call("attachment.read", { id, offset });
    if (page.encoding === "utf8") {
      text = true;
      texts.push(page.content);
    } else parts.push(base64ToBytes(page.content));
    if (page.next_offset !== null && page.next_offset <= offset) throw new Error("The attachment could not be read.");
    offset = page.next_offset;
  }
  if (text) return { bytes: new Uint8Array(new TextEncoder().encode(texts.join(""))), text };
  const total = parts.reduce((sum, part) => sum + part.length, 0);
  const bytes = new Uint8Array(total);
  let at = 0;
  for (const part of parts) {
    bytes.set(part, at);
    at += part.length;
  }
  return { bytes, text };
}

async function load(id: string, mime: string, keepOnFailure: boolean) {
  try {
    const { bytes } = await readAttachmentBytes(id);
    set(id, { status: "ready", src: `data:${mime};base64,${bytesToBase64(bytes)}` });
  } catch {
    if (!keepOnFailure) set(id, { status: "error" });
  }
}

/**
 * Starts reading an image unless it is cached, loading, or failed (a failure is not retried here, so
 * a persistent error cannot become a request loop). `mime` is the attachment's sniffed type.
 */
export function requestAttachmentImage(id: string, mime: string): void {
  const entry = entries.get(id);
  if (entry) {
    if (entry.status === "ready") {
      // Most recently used.
      entries.delete(id);
      entries.set(id, entry);
    }
    return;
  }
  set(id, { status: "loading" });
  void load(id, mime, false);
}

/** Reads an image now and returns its `data:` URL (for tray previews of project images). */
export async function loadAttachmentImage(id: string, mime: string): Promise<string | undefined> {
  const entry = entries.get(id);
  if (entry?.status === "ready") return entry.src;
  await load(id, mime, false);
  const loaded = entries.get(id);
  return loaded?.status === "ready" ? loaded.src : undefined;
}

/**
 * Counts a view that shows the image (`visible`) or stopped showing it, as `MessageAttachments`
 * reports through `onImageVisibilityChange`. Once no view shows it, the budget applies to it again.
 */
export function setAttachmentImageShown(id: string, visible: boolean): void {
  const count = (shown.get(id) ?? 0) + (visible ? 1 : -1);
  if (count > 0) {
    shown.set(id, count);
    return;
  }
  shown.delete(id);
  if (evict()) emit();
}

/**
 * Shows a just-sent image from its local preview. A `provisional` preview (a smaller rendition) is
 * replaced by the stored copy once that has been read; it stays if the read fails.
 */
export function seedAttachmentImage(id: string, src: string, mime: string, provisional: boolean): void {
  set(id, { status: "ready", src });
  if (provisional) void load(id, mime, true);
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

const snapshot = () => version;

/** The cached image of an attachment id, re-rendering whenever the cache changes. */
export function useAttachmentImages(): (id: string) => AttachmentImage | undefined {
  useSyncExternalStore(subscribe, snapshot, snapshot);
  return (id) => entries.get(id);
}
