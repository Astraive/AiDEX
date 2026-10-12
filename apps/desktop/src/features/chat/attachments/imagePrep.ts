// Image intake in the webview: decode with `createImageBitmap` (never through a `blob:` URL, which
// the release CSP blocks), downscale or convert to a format every provider reads, and build a
// `data:` URL preview. Pure size rules live in `attachmentFiles.ts`.
import { MAX_IMAGE_BYTES } from "@aidex/client/dto/attachments";
import {
  FORMAT_MIME,
  MAX_IMAGE_EDGE,
  STORED_IMAGE_FORMATS,
  bytesToBase64,
  type SniffedFormat,
} from "./attachmentFiles";

/** Encoder qualities tried in order, then the same at smaller scales (t3code's steps). */
const QUALITY_STEPS = [0.92, 0.85, 0.78, 0.68];
const SCALE_STEPS = [1, 0.75, 0.55];
/** Images up to this size preview as themselves; larger ones get a smaller rendition. */
const EXACT_PREVIEW_BYTES = 1.5 * 1024 * 1024;
/** Longest edge of a preview rendition. */
const PREVIEW_EDGE = 1600;

/** The image could not be decoded by this webview. */
export class ImageDecodeError extends Error {}
/** No encoding fits the image size limit. */
export class ImageTooLargeError extends Error {}

export type PreparedImage = {
  /** What is uploaded: the original bytes, or a PNG or JPEG rendition. */
  blob: Blob;
  mime: string;
  width: number;
  height: number;
  /** A `data:image/...` URL for the tray and the transcript. */
  previewUrl: string;
  /** The preview holds the uploaded bytes themselves (not a smaller rendition). */
  previewExact: boolean;
};

function canvasOf(bitmap: ImageBitmap, scale: number, matte: string | null): HTMLCanvasElement {
  const canvas = document.createElement("canvas");
  canvas.width = Math.max(1, Math.round(bitmap.width * scale));
  canvas.height = Math.max(1, Math.round(bitmap.height * scale));
  const context = canvas.getContext("2d");
  if (!context) throw new ImageDecodeError("no 2D canvas");
  if (matte) {
    context.fillStyle = matte;
    context.fillRect(0, 0, canvas.width, canvas.height);
  }
  context.imageSmoothingEnabled = true;
  context.imageSmoothingQuality = "high";
  context.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
  return canvas;
}

function encode(canvas: HTMLCanvasElement, type: string, quality?: number): Promise<Blob | null> {
  return new Promise((resolve) => canvas.toBlob(resolve, type, quality));
}

/** Whether any pixel of a small rendition is transparent (so a flat JPEG would lose something). */
function hasTransparency(bitmap: ImageBitmap): boolean {
  const scale = Math.min(1, 256 / Math.max(bitmap.width, bitmap.height));
  const canvas = canvasOf(bitmap, scale, null);
  const context = canvas.getContext("2d");
  if (!context) return false;
  const { data } = context.getImageData(0, 0, canvas.width, canvas.height);
  for (let index = 3; index < data.length; index += 4) if (data[index] < 250) return true;
  return false;
}

/** A canvas encoder can silently fall back to PNG, so the result's own type is what counts. */
function accepted(blob: Blob | null, wanted: string): blob is Blob {
  return !!blob && blob.size > 0 && blob.size <= MAX_IMAGE_BYTES && (blob.type === wanted || blob.type === "image/png");
}

/**
 * The first rendition within the limits: longest edge at most 2000 px; PNG first for transparent
 * images and graphics formats (GIF, BMP), then JPEG on white at falling qualities and scales.
 */
async function renditionWithinLimits(
  bitmap: ImageBitmap,
  format: SniffedFormat,
): Promise<{ blob: Blob; scale: number }> {
  const base = Math.min(1, MAX_IMAGE_EDGE / Math.max(bitmap.width, bitmap.height));
  const transparent = format !== "jpeg" && hasTransparency(bitmap);
  const pngFirst = transparent || format === "gif" || format === "bmp";
  for (const step of SCALE_STEPS) {
    const scale = base * step;
    if (pngFirst) {
      const png = await encode(canvasOf(bitmap, scale, null), "image/png");
      if (accepted(png, "image/png")) return { blob: png, scale };
    }
    const flat = canvasOf(bitmap, scale, "#ffffff");
    for (const quality of QUALITY_STEPS) {
      const jpeg = await encode(flat, "image/jpeg", quality);
      if (accepted(jpeg, "image/jpeg")) return { blob: jpeg, scale };
    }
  }
  throw new ImageTooLargeError("no rendition fits");
}

/** A `data:` URL of bytes with an explicit image type (a `File` may have none). */
export async function dataUrlOf(blob: Blob, mime: string): Promise<string> {
  return `data:${mime};base64,${bytesToBase64(new Uint8Array(await blob.arrayBuffer()))}`;
}

function previewRendition(bitmap: ImageBitmap, transparent: boolean): string {
  const scale = Math.min(1, PREVIEW_EDGE / Math.max(bitmap.width, bitmap.height));
  const canvas = canvasOf(bitmap, scale, transparent ? null : "#ffffff");
  return transparent ? canvas.toDataURL("image/png") : canvas.toDataURL("image/jpeg", 0.85);
}

/**
 * Prepares an image for upload. PNG and JPEG within 3,750,000 bytes and 2000 px pass through byte for
 * byte; larger ones are downscaled; WebP becomes PNG when transparent, else JPEG (some vendors, e.g.
 * xAI, read only PNG and JPEG); GIF becomes its first frame (models read only that, and Gemini refuses
 * GIF); BMP, HEIC, and AVIF are converted when this webview can decode them.
 */
export async function prepareImage(file: Blob, format: SniffedFormat): Promise<PreparedImage> {
  let bitmap: ImageBitmap;
  try {
    bitmap = await createImageBitmap(file);
  } catch {
    throw new ImageDecodeError("not decodable");
  }
  try {
    if (!bitmap.width || !bitmap.height) throw new ImageDecodeError("empty image");
    const passThrough =
      STORED_IMAGE_FORMATS.has(format) &&
      file.size <= MAX_IMAGE_BYTES &&
      Math.max(bitmap.width, bitmap.height) <= MAX_IMAGE_EDGE;
    let blob: Blob;
    let mime: string;
    let width = bitmap.width;
    let height = bitmap.height;
    if (passThrough) {
      blob = file;
      mime = FORMAT_MIME[format] ?? "image/png";
    } else {
      const rendition = await renditionWithinLimits(bitmap, format);
      blob = rendition.blob;
      mime = blob.type === "image/jpeg" ? "image/jpeg" : "image/png";
      width = Math.max(1, Math.round(bitmap.width * rendition.scale));
      height = Math.max(1, Math.round(bitmap.height * rendition.scale));
    }
    if (blob.size <= EXACT_PREVIEW_BYTES)
      return { blob, mime, width, height, previewUrl: await dataUrlOf(blob, mime), previewExact: true };
    const transparent = mime !== "image/jpeg" && hasTransparency(bitmap);
    return { blob, mime, width, height, previewUrl: previewRendition(bitmap, transparent), previewExact: false };
  } finally {
    bitmap.close();
  }
}
