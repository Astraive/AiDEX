// Turns a picked, pasted, or dropped file into something the daemon will accept, or a refusal the
// user can act on. Kinds come from the bytes (like the daemon's sniffing), never the declared type.
import { MAX_PDF_BYTES, MAX_TEXT_ATTACHMENT_BYTES } from "@aidex/client/dto/attachments";
import {
  CONVERTIBLE_IMAGE_FORMATS,
  MAX_IMAGE_SOURCE_BYTES,
  containsPrivateKey,
  intakeMessages,
  isUtf8Text,
  renameForMime,
  sniffFormat,
  textMimeOf,
} from "./attachmentFiles";
import { ImageDecodeError, ImageTooLargeError, prepareImage, type PreparedImage } from "./imagePrep";

export type PreparedFile = {
  kind: "image" | "pdf" | "text";
  /** Display name, renamed when an image was converted (`photo.heic` → `photo.jpg`). */
  name: string;
  /** What is uploaded. */
  blob: Blob;
  /** Declared to the daemon: names the type of text files only (the daemon sniffs the rest). */
  mime: string;
  previewUrl?: string;
  previewExact?: boolean;
};

export type IntakeResult = { ok: true; file: PreparedFile } | { ok: false; problem: string };

/** How much of a large file is sampled to tell text that is too large from an unsupported binary. */
const TEXT_SAMPLE_BYTES = 8 * 1024;

async function bytesOf(blob: Blob): Promise<Uint8Array<ArrayBuffer>> {
  return new Uint8Array(await blob.arrayBuffer());
}

/**
 * The bytes of a picked file held in memory. A `File` only points at the file on disk, and Chromium
 * refuses to read it once it changed, moved, or was deleted, so nothing that is uploaded later (a new
 * chat's first send, a re-upload of an expired copy) may keep the `File` itself.
 */
async function inMemory(blob: Blob, type: string): Promise<Blob> {
  return new Blob([await blob.arrayBuffer()], { type });
}

/**
 * Prepares one file. `name` is its display name. Images go through `prepare` (decode, downscale,
 * convert); PDFs and text pass through after their checks. What is uploaded is always held in memory,
 * never the picked file itself (`inMemory`).
 */
export async function prepareFile(
  file: Blob,
  name: string,
  declaredType: string,
  prepare: (file: Blob, format: ReturnType<typeof sniffFormat>) => Promise<PreparedImage> = prepareImage,
): Promise<IntakeResult> {
  const refuse = (problem: string): IntakeResult => ({ ok: false, problem });
  if (file.size === 0) return refuse(intakeMessages.unreadable(name));
  let head: Uint8Array;
  try {
    head = await bytesOf(file.slice(0, 64));
  } catch {
    return refuse(intakeMessages.unreadable(name));
  }
  const format = sniffFormat(head);
  if (format === "pdf") {
    if (file.size > MAX_PDF_BYTES) return refuse(intakeMessages.tooLarge(name));
    let blob: Blob;
    try {
      blob = await inMemory(file, "application/pdf");
    } catch {
      return refuse(intakeMessages.unreadable(name));
    }
    return { ok: true, file: { kind: "pdf", name, blob, mime: "application/pdf" } };
  }
  if (format !== "unknown") {
    if (file.size > MAX_IMAGE_SOURCE_BYTES) return refuse(intakeMessages.tooLarge(name));
    let image: PreparedImage;
    try {
      image = await prepare(file, format);
    } catch (caught) {
      if (caught instanceof ImageTooLargeError) return refuse(intakeMessages.tooLargeAfterCompression(name));
      if (caught instanceof ImageDecodeError && format === "heic") return refuse(intakeMessages.heic(name));
      if (caught instanceof ImageDecodeError && CONVERTIBLE_IMAGE_FORMATS.has(format))
        return refuse(intakeMessages.unsupported(name));
      return refuse(intakeMessages.notImage(name));
    }
    // An image that passes through unchanged is still the picked file: keep its bytes instead.
    let blob = image.blob;
    if (blob === file) {
      try {
        blob = await inMemory(file, image.mime);
      } catch {
        return refuse(intakeMessages.unreadable(name));
      }
    }
    return {
      ok: true,
      file: {
        kind: "image",
        name: renameForMime(name, image.mime),
        blob,
        mime: image.mime,
        previewUrl: image.previewUrl,
        previewExact: image.previewExact,
      },
    };
  }
  // Anything else must be UTF-8 text without NUL bytes.
  if (file.size > MAX_TEXT_ATTACHMENT_BYTES) {
    try {
      const sample = await bytesOf(file.slice(0, TEXT_SAMPLE_BYTES));
      return refuse(isUtf8Text(sample, true) ? intakeMessages.tooLarge(name) : intakeMessages.unsupported(name));
    } catch {
      return refuse(intakeMessages.unreadable(name));
    }
  }
  let bytes: Uint8Array<ArrayBuffer>;
  try {
    bytes = await bytesOf(file);
  } catch {
    return refuse(intakeMessages.unreadable(name));
  }
  if (!isUtf8Text(bytes)) return refuse(intakeMessages.unsupported(name));
  if (containsPrivateKey(new TextDecoder().decode(bytes))) return refuse(intakeMessages.privateKey(name));
  // A copy (like PDFs and images above), so editing the file on disk before the upload cannot change
  // or break what is sent.
  const copy = new Blob([bytes]);
  return { ok: true, file: { kind: "text", name, blob: copy, mime: textMimeOf(name, declaredType) } };
}
