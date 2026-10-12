// Reading artifacts through `artifact.read` pages, choosing how to preview them, and saving them as
// files. Content is untrusted model or web output: it is rendered as text, Markdown, or an image,
// never as HTML or script.
import { call } from "@aidex/client/rpc";
import type { ArtifactContent, ArtifactMeta } from "@aidex/client/dto/artifacts";

/** How much the viewer loads before asking the user to download the rest. */
export const PREVIEW_LIMIT_BYTES = 1024 * 1024;

export type ArtifactData =
  | { encoding: "utf8"; text: string; totalBytes: number; complete: boolean }
  | { encoding: "base64"; bytes: Uint8Array<ArrayBuffer>; totalBytes: number; complete: boolean };

/** Decodes standard base64 (as `artifact.read` returns binary pages). */
export function base64ToBytes(base64: string): Uint8Array<ArrayBuffer> {
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
  return bytes;
}

function concatBytes(parts: readonly Uint8Array[]): Uint8Array<ArrayBuffer> {
  const total = parts.reduce((sum, part) => sum + part.length, 0);
  const bytes = new Uint8Array(total);
  let offset = 0;
  for (const part of parts) {
    bytes.set(part, offset);
    offset += part.length;
  }
  return bytes;
}

/**
 * Reads an artifact page by page, stopping after about `maxBytes` (each page is at most 64 KiB and
 * stands alone: UTF-8 pages never split a character, base64 pages decode independently).
 */
export async function readArtifact(id: string, maxBytes = Number.POSITIVE_INFINITY): Promise<ArtifactData> {
  const texts: string[] = [];
  const parts: Uint8Array[] = [];
  let offset: number | null = 0;
  let read = 0;
  let encoding: ArtifactMeta["encoding"] = "utf8";
  let totalBytes = 0;
  while (offset !== null && read < maxBytes) {
    const page: ArtifactContent = await call("artifact.read", { id, offset });
    encoding = page.encoding;
    totalBytes = page.total_bytes;
    if (page.encoding === "base64") {
      const bytes = base64ToBytes(page.content);
      parts.push(bytes);
      read += bytes.length;
    } else {
      texts.push(page.content);
      read += (page.next_offset ?? page.total_bytes) - page.offset;
    }
    offset = page.next_offset;
  }
  const complete = offset === null;
  return encoding === "base64"
    ? { encoding, bytes: concatBytes(parts), totalBytes, complete }
    : { encoding, text: texts.join(""), totalBytes, complete };
}

export type PreviewKind = "markdown" | "code" | "image" | "text" | "binary";

const markdownMimes = new Set(["text/markdown", "text/x-markdown"]);
const codeMimes = new Set([
  "application/json",
  "application/ld+json",
  "application/xml",
  "application/javascript",
  "application/typescript",
  "application/x-sh",
  "application/toml",
  "application/yaml",
  "text/css",
  "text/csv",
  "text/html",
  "text/javascript",
  "text/xml",
  "text/x-python",
  "text/x-rust",
]);
/** Raster formats an `<img>` decodes; SVG is shown as an image too (scripts never run there). */
const imageMimes = new Set(["image/png", "image/jpeg", "image/gif", "image/webp", "image/bmp", "image/svg+xml"]);

function baseMime(mime: string): string {
  return mime.split(";")[0].trim().toLowerCase();
}

/** How the viewer shows an artifact. */
export function previewKind(meta: Pick<ArtifactMeta, "kind" | "mime" | "language" | "encoding">): PreviewKind {
  const mime = baseMime(meta.mime);
  if (imageMimes.has(mime)) return "image";
  if (meta.encoding === "base64") return "binary";
  if (markdownMimes.has(mime) || meta.kind === "plan") return "markdown";
  if (meta.kind === "code" || meta.kind === "data" || meta.language || codeMimes.has(mime)) return "code";
  if (meta.kind === "document" || meta.kind === "web_page") return mime === "text/plain" ? "text" : "markdown";
  return "text";
}

/** Language label for a code preview. */
export function previewLanguage(meta: Pick<ArtifactMeta, "mime" | "language">): string | undefined {
  if (meta.language) return meta.language;
  const mime = baseMime(meta.mime);
  if (mime.endsWith("json")) return "json";
  if (mime.endsWith("xml")) return "xml";
  if (mime === "text/csv") return "csv";
  if (mime.endsWith("yaml")) return "yaml";
  return undefined;
}

/** A `data:` URL for an image preview: base64 bytes as they are, SVG text percent-encoded. */
export function imageDataUrl(meta: Pick<ArtifactMeta, "mime">, data: ArtifactData): string {
  const mime = baseMime(meta.mime);
  if (data.encoding === "utf8") return `data:${mime};charset=utf-8,${encodeURIComponent(data.text)}`;
  let binary = "";
  const step = 0x8000;
  for (let index = 0; index < data.bytes.length; index += step)
    binary += String.fromCharCode(...data.bytes.subarray(index, index + step));
  return `data:${mime};base64,${btoa(binary)}`;
}

const extensions: Record<string, string> = {
  "text/markdown": "md",
  "text/x-markdown": "md",
  "text/plain": "txt",
  "text/html": "html",
  "text/css": "css",
  "text/csv": "csv",
  "application/json": "json",
  "application/xml": "xml",
  "text/xml": "xml",
  "application/yaml": "yaml",
  "application/toml": "toml",
  "application/javascript": "js",
  "text/javascript": "js",
  "application/pdf": "pdf",
  "image/png": "png",
  "image/jpeg": "jpg",
  "image/gif": "gif",
  "image/webp": "webp",
  "image/svg+xml": "svg",
};

const languageExtensions: Record<string, string> = {
  typescript: "ts",
  javascript: "js",
  python: "py",
  rust: "rs",
  go: "go",
  java: "java",
  shell: "sh",
  bash: "sh",
  sql: "sql",
  html: "html",
  css: "css",
  json: "json",
  yaml: "yaml",
  toml: "toml",
  markdown: "md",
};

/** A safe file name for saving: the artifact name without path or reserved characters, plus an extension. */
export function artifactFileName(meta: Pick<ArtifactMeta, "name" | "mime" | "language" | "kind">): string {
  const cleaned = Array.from(meta.name)
    .map((char) => (/[<>:"/\\|?*]/.test(char) || char.charCodeAt(0) < 32 ? "_" : char))
    .join("")
    .trim()
    .replace(/^[.\s]+|[.\s]+$/g, "")
    .slice(0, 120);
  const base = cleaned || "artifact";
  if (/\.[A-Za-z0-9]{1,8}$/.test(base)) return base;
  const extension =
    (meta.language && languageExtensions[meta.language.toLowerCase()]) ??
    extensions[baseMime(meta.mime)] ??
    (meta.kind === "plan" || meta.kind === "web_page" ? "md" : "txt");
  return `${base}.${extension}`;
}

/** Saves the whole artifact as a file through the webview's download handling. */
export async function downloadArtifact(id: string): Promise<void> {
  const meta = await call("artifact.get", { id });
  const data = await readArtifact(id);
  const blob =
    data.encoding === "utf8"
      ? new Blob([data.text], { type: baseMime(meta.mime) || "text/plain" })
      : new Blob([data.bytes], { type: baseMime(meta.mime) || "application/octet-stream" });
  saveBlob(blob, artifactFileName(meta));
}

/** Saves text (e.g. a plan) as a file. */
export function downloadText(text: string, fileName: string, mime = "text/markdown"): void {
  saveBlob(new Blob([text], { type: mime }), fileName);
}

/** Saves a blob as a file through an anchor download. */
export function saveBlob(blob: Blob, fileName: string): void {
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = fileName;
  anchor.rel = "noopener";
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  setTimeout(() => URL.revokeObjectURL(url), 30_000);
}
