// Pure attachment rules for the composer: what a file is (from its bytes, like the daemon), the
// names and limits the daemon accepts, user-facing refusals, upload chunks, and when a send must
// wait. No DOM or bridge access, so they run in unit tests.
import {
  MAX_ATTACHMENT_NAME_BYTES,
  MAX_ATTACHMENTS_PER_MESSAGE,
  MAX_IMAGE_BYTES,
  MAX_MESSAGE_ATTACHMENT_BYTES,
  MAX_PDF_BYTES,
  MAX_TEXT_ATTACHMENT_BYTES,
  type AttachmentErrorReason,
} from "@aidex/client/dto/attachments";
import type { ModelInput } from "@aidex/client/dto/models";

/** The format a file's first bytes announce. */
export type SniffedFormat =
  | "png"
  | "jpeg"
  | "gif"
  | "webp"
  | "bmp"
  | "heic"
  | "avif"
  | "pdf"
  | "unknown";

/** Image formats converted to PNG or JPEG when this webview can decode them (refused otherwise). */
export const CONVERTIBLE_IMAGE_FORMATS: ReadonlySet<SniffedFormat> = new Set(["bmp", "heic", "avif"]);

/**
 * Image formats uploaded as they are when within the limits. Every vendor reads PNG and JPEG; WebP
 * (some vendors, e.g. xAI, take only PNG and JPEG) and GIF (first frame) are converted first.
 */
export const STORED_IMAGE_FORMATS: ReadonlySet<SniffedFormat> = new Set(["png", "jpeg"]);

export const FORMAT_MIME: Partial<Record<SniffedFormat, string>> = {
  png: "image/png",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
  pdf: "application/pdf",
};

/** Largest picked image the webview decodes for downscaling (bigger ones are refused unread). */
export const MAX_IMAGE_SOURCE_BYTES = 50 * 1024 * 1024;
/** Longest image edge sent to models; larger images are downscaled to it. */
export const MAX_IMAGE_EDGE = 2000;

const BRANDS_HEIC = new Set(["heic", "heix", "hevc", "hevx", "heim", "heis", "hevm", "hevs", "mif1", "msf1"]);
const BRANDS_AVIF = new Set(["avif", "avis"]);

function ascii(bytes: Uint8Array, start: number, end: number): string {
  let text = "";
  for (let index = start; index < end && index < bytes.length; index += 1) text += String.fromCharCode(bytes[index]);
  return text;
}

/**
 * Sizes of the BMP info header that follows the 14-byte file header. Each is below 256, so a real BMP
 * has NUL bytes there and can never be text that starts with "BM" (`bmi.csv`, "BMW …").
 */
const BMP_INFO_HEADER_SIZES = new Set([12, 16, 40, 52, 56, 64, 108, 124]);

function littleEndianU32(bytes: Uint8Array, at: number): number {
  return (bytes[at] | (bytes[at + 1] << 8) | (bytes[at + 2] << 16) | (bytes[at + 3] << 24)) >>> 0;
}

function startsWith(bytes: Uint8Array, prefix: readonly number[]): boolean {
  if (bytes.length < prefix.length) return false;
  return prefix.every((value, index) => bytes[index] === value);
}

/** The format of a file from its first bytes (at least 12 are needed for WebP and HEIF, 18 for BMP). */
export function sniffFormat(head: Uint8Array): SniffedFormat {
  if (startsWith(head, [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])) return "png";
  if (startsWith(head, [0xff, 0xd8, 0xff])) return "jpeg";
  const six = ascii(head, 0, 6);
  if (six === "GIF87a" || six === "GIF89a") return "gif";
  if (head.length >= 12 && ascii(head, 0, 4) === "RIFF" && ascii(head, 8, 12) === "WEBP") return "webp";
  if (ascii(head, 0, 5) === "%PDF-") return "pdf";
  if (head.length >= 12 && ascii(head, 4, 8) === "ftyp") {
    const brand = ascii(head, 8, 12).toLowerCase();
    if (BRANDS_AVIF.has(brand)) return "avif";
    if (BRANDS_HEIC.has(brand)) return "heic";
  }
  if (head.length >= 18 && ascii(head, 0, 2) === "BM" && BMP_INFO_HEADER_SIZES.has(littleEndianU32(head, 14)))
    return "bmp";
  return "unknown";
}

/**
 * Whether bytes are UTF-8 text the daemon accepts: valid UTF-8 without NUL. `partial` tolerates a
 * character cut at the end (for a sample of a larger file).
 */
export function isUtf8Text(bytes: Uint8Array, partial = false): boolean {
  if (bytes.includes(0)) return false;
  try {
    new TextDecoder("utf-8", { fatal: true }).decode(bytes, { stream: partial });
    return true;
  } catch {
    return false;
  }
}

/** A `-----BEGIN … PRIVATE KEY-----` header (the daemon refuses such text as `sensitive_content`). */
export function containsPrivateKey(text: string): boolean {
  return /-----BEGIN [^\n]{0,64}?PRIVATE KEY/.test(text);
}

function utf8Length(text: string): number {
  return new TextEncoder().encode(text).length;
}

/** Drops whole code points from the end until the text fits `maxBytes` of UTF-8. */
function truncateUtf8(text: string, maxBytes: number): string {
  const points = Array.from(text);
  while (points.length > 0 && utf8Length(points.join("")) > maxBytes) points.pop();
  return points.join("");
}

/** Control (Cc) and formatting (Cf) characters and the line and paragraph separators. */
const UNSTORABLE_NAME_CHARACTERS = /[\p{Cc}\p{Cf}\u2028\u2029]/gu;

/**
 * The name the daemon stores: the final path component without control or formatting characters,
 * trimmed, at most 255 UTF-8 bytes (the extension is kept when the stem is shortened).
 */
export function attachmentDisplayName(raw: string, fallback = "attachment"): string {
  const last = raw.split(/[\\/]/).pop() ?? "";
  let name = last.replace(UNSTORABLE_NAME_CHARACTERS, "").trim();
  if (name === "." || name === "..") name = "";
  if (!name) name = fallback;
  if (utf8Length(name) <= MAX_ATTACHMENT_NAME_BYTES) return name;
  const dot = name.lastIndexOf(".");
  const extension = dot > 0 && name.length - dot <= 16 ? name.slice(dot) : "";
  const stem = extension ? name.slice(0, dot) : name;
  const kept = truncateUtf8(stem, MAX_ATTACHMENT_NAME_BYTES - utf8Length(extension)).trimEnd();
  return (kept || fallback.slice(0, 32)) + extension;
}

const EXTENSION_BY_MIME: Record<string, string> = {
  "image/png": "png",
  "image/jpeg": "jpg",
  "image/gif": "gif",
  "image/webp": "webp",
};

/** `name` with the extension of `mime` (after a conversion, e.g. `photo.heic` → `photo.jpg`). */
export function renameForMime(name: string, mime: string): string {
  const extension = EXTENSION_BY_MIME[mime];
  if (!extension) return name;
  const dot = name.lastIndexOf(".");
  const stem = dot > 0 ? name.slice(0, dot) : name;
  const current = dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
  if (current === extension || (extension === "jpg" && current === "jpeg")) return name;
  return attachmentDisplayName(`${stem}.${extension}`);
}

const TEXT_MIME_BY_EXTENSION: Record<string, string> = {
  md: "text/markdown",
  markdown: "text/markdown",
  txt: "text/plain",
  log: "text/plain",
  csv: "text/csv",
  html: "text/html",
  htm: "text/html",
  css: "text/css",
  json: "application/json",
  xml: "application/xml",
  yaml: "application/x-yaml",
  yml: "application/x-yaml",
  toml: "application/toml",
  js: "application/javascript",
  mjs: "application/javascript",
  cjs: "application/javascript",
  jsx: "application/javascript",
  ts: "application/typescript",
  tsx: "application/typescript",
  sh: "application/x-sh",
  sql: "application/sql",
  rs: "text/x-rust",
  py: "text/x-python",
};

/** The MIME type to declare for a text file: its own when it names text, else one from the extension. */
export function textMimeOf(name: string, declared: string): string {
  const mime = declared.split(";")[0].trim().toLowerCase();
  if (
    mime.startsWith("text/") ||
    mime === "application/json" ||
    (mime.startsWith("application/") && (mime.endsWith("+json") || mime.endsWith("+xml")))
  )
    return mime;
  const dot = name.lastIndexOf(".");
  const extension = dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
  return TEXT_MIME_BY_EXTENSION[extension] ?? "text/plain";
}

/** Duplicate check like t3code's: the same name, size, and declared type. */
export function dedupeKey(file: { name: string; size: number; type: string }): string {
  return `${file.name}\u0000${file.size}\u0000${file.type}`;
}

/** `pasted-text.txt`, then `pasted-text-2.txt`, … — the first name not already taken. */
export function nextPasteName(taken: Iterable<string>): string {
  const names = new Set(taken);
  if (!names.has("pasted-text.txt")) return "pasted-text.txt";
  for (let index = 2; ; index += 1) {
    const name = `pasted-text-${index}.txt`;
    if (!names.has(name)) return name;
  }
}

/** Refusals shown when files are added (t3code wording where it has one). */
export const intakeMessages = {
  tooMany: `You can attach up to ${MAX_ATTACHMENTS_PER_MESSAGE} files per message.`,
  unsupported: (name: string) =>
    `'${name}' is not a supported file. Attach PNG, JPEG, GIF or WebP images, PDFs, or text files.`,
  tooLarge: (name: string) => `'${name}' is too large to attach.`,
  tooLargeAfterCompression: (name: string) => `'${name}' is too large to attach, even after compression.`,
  unreadable: (name: string) => `'${name}' is empty or could not be read.`,
  notImage: (name: string) => `'${name}' could not be read as an image.`,
  heic: (name: string) =>
    `'${name}' is a HEIC image this computer can't convert. Save it as JPEG or PNG and attach it again.`,
  privateKey: (name: string) => `'${name}' contains a private key, so it can't be attached.`,
  changedOnDisk: (name: string) =>
    `'${name}' changed or was moved after it was attached. Remove it and attach it again.`,
  messageTotal: (name: string) =>
    `'${name}' would take this message's attachments over ${MAX_MESSAGE_ATTACHMENT_BYTES / (1024 * 1024)} MB. Send it in another message.`,
  pasteTooLarge: "Pasted text is too large to attach.",
} as const;

/** The per-kind size cap the daemon enforces. */
export function sizeLimitOf(kind: "image" | "pdf" | "text"): number {
  return kind === "image" ? MAX_IMAGE_BYTES : kind === "pdf" ? MAX_PDF_BYTES : MAX_TEXT_ATTACHMENT_BYTES;
}

/** Whether a file of this size is worth reading as a text candidate at all. */
export function textSizeAllowed(size: number): boolean {
  return size > 0 && size <= MAX_TEXT_ATTACHMENT_BYTES;
}

/** Whether adding `size` keeps the message within its total (`existing` = sizes already attached). */
export function fitsMessageTotal(existing: readonly number[], size: number): boolean {
  return existing.reduce((sum, value) => sum + value, 0) + size <= MAX_MESSAGE_ATTACHMENT_BYTES;
}

/** Byte ranges of an upload: every chunk but the last is exactly `chunkBytes`. */
export function chunkRanges(size: number, chunkBytes: number): Array<[number, number]> {
  if (chunkBytes <= 0) throw new Error("chunk size must be positive");
  const ranges: Array<[number, number]> = [];
  for (let start = 0; start < size; start += chunkBytes) ranges.push([start, Math.min(size, start + chunkBytes)]);
  return ranges;
}

/** Standard base64 of bytes (as `attachment.upload.chunk` takes them). */
export function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  const step = 0x8000;
  for (let index = 0; index < bytes.length; index += step)
    binary += String.fromCharCode(...bytes.subarray(index, index + step));
  return btoa(binary);
}

/** Lowercase hex of a digest. */
export function toHex(buffer: ArrayBuffer): string {
  return Array.from(new Uint8Array(buffer), (byte) => byte.toString(16).padStart(2, "0")).join("");
}

/** What the composer knows about an attachment when deciding whether a send can go out. */
export type SendBlockItem = {
  kind: "image" | "pdf" | "text" | "file";
  status: "processing" | "pending" | "uploading" | "ready" | "failed";
};

export type SendBlockModel = {
  name: string;
  /** From `model.list`; absent on daemons that predate attachments. */
  input?: ModelInput;
  vision?: boolean;
};

const plural = (count: number, one: string, many: string) => (count === 1 ? one : many);

/**
 * Why the composer cannot send its attachments yet, or undefined. External runtimes take none;
 * preparing, uploading, and failed attachments block until they are ready or removed; the model must
 * read images and PDFs (from `model.list` `input`).
 */
export function attachmentSendBlock(input: {
  items: readonly SendBlockItem[];
  external: boolean;
  model?: SendBlockModel;
}): string | undefined {
  const { items, external, model } = input;
  if (items.length === 0) return undefined;
  if (external) return "Attachments work with AiDEX native models. Remove them to send.";
  const count = (status: SendBlockItem["status"]) => items.filter((item) => item.status === status).length;
  const processing = count("processing");
  if (processing) return plural(processing, "Preparing attachment…", "Preparing attachments…");
  const uploading = count("uploading");
  if (uploading) return plural(uploading, "Attachment still uploading", "Attachments still uploading");
  const failed = count("failed");
  if (failed) return plural(failed, "Retry or remove the failed attachment", "Retry or remove the failed attachments");
  if (!model) return undefined;
  const accepts = model.input ?? { images: !!model.vision, pdf: false };
  if (!accepts.images && items.some((item) => item.kind === "image"))
    return `${model.name} can't read images. Pick a vision model or remove the images.`;
  if (!accepts.pdf && items.some((item) => item.kind === "pdf"))
    return `${model.name} can't read PDFs. Pick a model that reads PDFs or remove the PDFs.`;
  return undefined;
}

/**
 * Whether reading a picked file failed because it changed, moved, or was deleted on disk after it was
 * attached (Chromium's `NotReadableError` and `NotFoundError`); reading it again fails the same way.
 */
export function isFileReadError(error: unknown): boolean {
  if (typeof error !== "object" || error === null) return false;
  const name = (error as { name?: unknown }).name;
  return name === "NotReadableError" || name === "NotFoundError";
}

/** `data.reason` of a refused attachment call, when it carries one. */
export function reasonOf(data: unknown): AttachmentErrorReason | undefined {
  if (typeof data !== "object" || data === null) return undefined;
  const reason = (data as Record<string, unknown>).reason;
  return typeof reason === "string" ? (reason as AttachmentErrorReason) : undefined;
}

function sentence(text: string): string {
  const trimmed = text.trim();
  if (!trimmed) return "The upload failed.";
  const capital = trimmed[0].toUpperCase() + trimmed.slice(1);
  return /[.!?…]$/.test(capital) ? capital : `${capital}.`;
}

/**
 * The message for a failed upload or project attach of `name`: refusals with a reason read as the
 * intake refusals do; a locked vault asks to unlock; anything else keeps the daemon's message.
 */
export function uploadFailureMessage(
  name: string,
  kind: SendBlockItem["kind"],
  error: { kind?: string; message: string; data?: unknown },
): string {
  if (error.kind === "vault_locked") return "Unlock the vault, then retry the upload.";
  switch (reasonOf(error.data)) {
    case "too_large":
      return intakeMessages.tooLarge(name);
    case "too_many_attachments":
      // The daemon caps unsent attachments per conversation and across all conversations alike.
      return "Too many attachments are waiting to be sent in this or other conversations. Send or remove some first.";
    case "busy":
      return "Too many uploads are in progress. Retry in a moment.";
    case "bad_offset":
    case "hash_mismatch":
      return "The upload was interrupted. Retry it.";
    case "unsupported_type":
      return kind === "pdf"
        ? `'${name}' is an encrypted or damaged PDF and can't be attached.`
        : intakeMessages.unsupported(name);
    case "sensitive_content":
      return `'${name}' looks like it holds credentials or a private key, so it can't be attached.`;
    case "too_many_pages":
      return `'${name}' has more than 100 pages.`;
    case "invalid_name":
      return `'${name}' has a name AiDEX can't store. Rename the file and attach it again.`;
    default:
      return sentence(error.message);
  }
}

/** A short project-relative path: trimmed, forward slashes, no leading `./`. */
export function normalizeProjectPath(raw: string): string {
  return raw
    .trim()
    .replace(/\\/g, "/")
    .replace(/^(\.\/)+/, "");
}
