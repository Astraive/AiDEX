// Mirrors the `attachment.*` methods in crates/aidexd/src/methods/attachments.rs and the
// attachment limits and records of `aidex_native_sessions`
// (crates/native-sessions/src/attachments/model.rs). Attachments are sealed at rest with
// the vault-held record key and unavailable while the vault is locked (`vault_locked`);
// only `attachment.upload.abort` works then.

/** Decided by the daemon from the bytes, never from the client's MIME type. `unknown` is a
 * kind a newer daemon wrote: listed, never sent. */
export type AttachmentKind = "image" | "pdf" | "text" | "unknown";

/** `staged`: uploaded, waiting in the composer. `sent`: part of a message, kept (and deleted)
 * with the conversation. */
export type AttachmentState = "staged" | "sent";

/** Where the bytes came from: a folded paste, a file the user picked or dropped, or a file of
 * a registered project (`attachment.from_workspace`). */
export type AttachmentSource = "paste" | "file" | "workspace" | "other";

/** How `attachment.read` returns a page: `utf8` for text attachments, `base64` otherwise. */
export type AttachmentEncoding = "utf8" | "base64";

/** `Attachment`: result of `attachment.get`, `attachment.upload.commit`,
 * `attachment.from_workspace`, `attachment.exclude`, and the entries of `attachment.list`. */
export type Attachment = {
  /** `att_<ULID>`. */
  id: string;
  session_id: string;
  /** Display name (a final path component), never a path. */
  name: string;
  kind: AttachmentKind;
  /** Sniffed MIME type (`image/png|jpeg|gif|webp`, `application/pdf`, or a text type). */
  mime: string;
  /** Plaintext bytes. */
  size: number;
  /** SHA-256 of the plaintext (lowercase hex). */
  sha256: string;
  /** PDF page count. */
  pages?: number;
  /** Image size in pixels. */
  width?: number;
  height?: number;
  /** An animated GIF, PNG, or WebP. */
  animated: boolean;
  state: AttachmentState;
  source?: AttachmentSource;
  /** Unix milliseconds. */
  created_at: number;
  /** The run and user message that sent it (`sent` only). */
  run_id?: string;
  message_index?: number;
  /** 0-based order within its message. */
  position?: number;
  /** Kept out of the model context by `attachment.exclude`. */
  excluded: boolean;
};

/** What a user message in `conversation.get` and session `history.list` says about each
 * attachment sent with it, in the order they were sent. */
export type AttachmentRef = {
  id: string;
  name: string;
  kind: AttachmentKind;
  mime: string;
  size: number;
  pages?: number;
  width?: number;
  height?: number;
  /** Present (true) when the attachment is kept out of the model context. */
  excluded?: boolean;
};

/** Params of `attachment.upload.begin`. `size` and `sha256` are declared up front (the daemon
 * seals each chunk as it arrives) and verified at commit. `mime` only names the type of text
 * files. `source` defaults to `file`. */
export type AttachmentUploadBeginParams = {
  session_id: string;
  name: string;
  size: number;
  /** SHA-256 of the whole file, hex. */
  sha256: string;
  mime?: string;
  source?: "paste" | "file";
};

/** Result of `attachment.upload.begin`. Every chunk but the last must be exactly
 * `chunk_bytes` long. */
export type AttachmentUploadTicket = { upload_id: string; chunk_bytes: number };

/** Params of `attachment.upload.chunk`: `offset` must equal the bytes received so far (a chunk
 * resent at an earlier chunk offset with the same length is acknowledged without writing);
 * `data` is standard base64. */
export type AttachmentUploadChunkParams = {
  upload_id: string;
  offset: number;
  data: string;
};

/** Params of `attachment.from_workspace`: a project-relative path, read through the project's
 * confined root. */
export type AttachmentFromWorkspaceParams = {
  session_id: string;
  workspace_id: string;
  path: string;
};

/** Params of `attachment.list` (`limit` 1-200, default 200). */
export type AttachmentListParams = {
  session_id: string;
  offset?: number;
  limit?: number;
};

/** Result of `attachment.list`: staged and sent attachments, newest first. */
export type AttachmentList = {
  attachments: Attachment[];
  total_attachments: number;
  offset: number;
  next_offset: number | null;
  truncated: boolean;
};

/** Params of `attachment.read`. Pages are at most 512 KiB (`base64`) or 128 KiB (`utf8`);
 * larger limits are clamped. */
export type AttachmentReadParams = { id: string; offset?: number; limit?: number };

/** Result of `attachment.read`. `utf8` pages never split a character. */
export type AttachmentContent = {
  id: string;
  encoding: AttachmentEncoding;
  content: string;
  offset: number;
  /** Null on the last page. */
  next_offset: number | null;
  total_bytes: number;
  sha256: string;
};

/** Params of `attachment.exclude` (`excluded` defaults to true). Only sent attachments. */
export type AttachmentExcludeParams = { id: string; excluded?: boolean };

/** `data.reason` of refused attachment operations and sends. All are `invalid_params`
 * (-32602) except `busy`, which is `unavailable` (-32006). */
export type AttachmentErrorReason =
  | "too_large"
  | "too_many_attachments"
  | "busy"
  | "bad_offset"
  | "hash_mismatch"
  | "unsupported_type"
  | "sensitive_content"
  | "too_many_pages"
  | "attachment_sent"
  | "attachment_unavailable"
  | "invalid_name"
  | "invalid_attachment"
  /** `message.send`/`agent.start`: the model or runtime cannot take these attachments. */
  | "attachments_unsupported"
  /** `message.send`/`agent.start`: the conversation is too large for its external runtime. */
  | "context_too_large_for_runtime";

/** Stored as the content of a user message sent with attachments and no text. Hide exactly
 * this text on a message whose `attachments` is non-empty. */
export const ATTACHMENT_ONLY_CONTENT = "[Attached files]";

/** Most attachments sent with one message. */
export const MAX_ATTACHMENTS_PER_MESSAGE = 20;
/** Largest image (bytes); larger images are downscaled before upload. */
export const MAX_IMAGE_BYTES = 3_750_000;
/** Longest image side the daemon accepts (pixels). */
export const MAX_IMAGE_DIMENSION = 8000;
/** Largest PDF (16 MiB), at most `MAX_PDF_PAGES` pages. */
export const MAX_PDF_BYTES = 16 * 1024 * 1024;
export const MAX_PDF_PAGES = 100;
/** Largest text attachment (1 MiB). */
export const MAX_TEXT_ATTACHMENT_BYTES = 1024 * 1024;
/** Largest total of the attachments of one message (32 MiB). */
export const MAX_MESSAGE_ATTACHMENT_BYTES = 32 * 1024 * 1024;
/** Largest upload of any kind (16 MiB), checked at `attachment.upload.begin`. */
export const MAX_UPLOAD_BYTES = 16 * 1024 * 1024;
/** Most staged (unsent) attachments per conversation, uploads in flight included. */
export const MAX_STAGED_PER_SESSION = 40;
/** Most staged plus in-flight bytes across all conversations (256 MiB). */
export const MAX_STAGED_BYTES = 256 * 1024 * 1024;
/** Size of every upload chunk but the last (256 KiB raw). */
export const UPLOAD_CHUNK_BYTES = 256 * 1024;
/** Largest `base64` page of `attachment.read` (512 KiB raw). */
export const ATTACHMENT_PAGE_BYTES = 512 * 1024;
/** Largest `utf8` page of `attachment.read` (128 KiB raw). */
export const ATTACHMENT_TEXT_PAGE_BYTES = 128 * 1024;
/** An upload with no chunk for this long (10 minutes) is dropped. */
export const UPLOAD_IDLE_TIMEOUT_MS = 10 * 60 * 1000;
/** Staged attachments older than a day are deleted when the daemon starts. */
export const STAGED_TTL_MS = 24 * 60 * 60 * 1000;
/** Most uploads in flight per daemon, and their declared bytes (64 MiB). */
export const MAX_INFLIGHT_UPLOADS = 8;
export const MAX_INFLIGHT_UPLOAD_BYTES = 64 * 1024 * 1024;
/** How much of a text attachment a prompt inlines; the rest is cut with a note. */
export const INLINE_TEXT_ATTACHMENT_CHARS = 256 * 1024;
/** Most entries of one `attachment.list` page. */
export const ATTACHMENT_LIST_LIMIT = 200;
/** Longest attachment name (bytes). */
export const MAX_ATTACHMENT_NAME_BYTES = 255;
/** Image MIME types the daemon accepts (sniffed from the bytes). */
export const ATTACHMENT_IMAGE_MIMES = [
  "image/png",
  "image/jpeg",
  "image/gif",
  "image/webp",
] as const;
