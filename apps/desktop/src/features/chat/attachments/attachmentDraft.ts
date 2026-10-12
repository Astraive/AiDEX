// The attachments waiting in one composer: intake (prepare and validate files, fold large pastes,
// add project files), sequential chunked uploads to the bound conversation with progress, remove
// (abort or delete the staged copy), retry, showing an existing conversation's staged copies again,
// and handing the ready ids to a send. Framework-free (React subscribes through
// `subscribe`/`getSnapshot`); RPC and file preparation are injectable.
import { call as rpcCall, type RpcMethod, type RpcParams, type RpcResult } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import {
  ATTACHMENT_LIST_LIMIT,
  MAX_ATTACHMENTS_PER_MESSAGE,
  MAX_TEXT_ATTACHMENT_BYTES,
  type Attachment,
  type AttachmentKind,
  type AttachmentList,
} from "@aidex/client/dto/attachments";
import {
  attachmentDisplayName,
  bytesToBase64,
  chunkRanges,
  containsPrivateKey,
  dedupeKey,
  fitsMessageTotal,
  intakeMessages,
  isFileReadError,
  nextPasteName,
  toHex,
  uploadFailureMessage,
} from "./attachmentFiles";
import { prepareFile } from "./attachmentIntake";
import { loadAttachmentImage, seedAttachmentImage } from "./attachmentImages";

export type DraftStatus = "processing" | "pending" | "uploading" | "ready" | "failed";
export type DraftKind = "image" | "pdf" | "text" | "file";

export type DraftAttachment = {
  /** Local key (the tray's item id); the daemon id is `attachmentId`. */
  readonly key: string;
  readonly name: string;
  /** From the bytes once prepared (`file` while unknown, e.g. a project file before it is read). */
  readonly kind: DraftKind;
  readonly size?: number;
  readonly mime?: string;
  /** `file`: picked, pasted, or dropped; `paste`: a folded text paste; `workspace`: a project file. */
  readonly source: "file" | "paste" | "workspace";
  /**
   * `processing`: being prepared; `pending`: waiting for the conversation or its turn to upload;
   * `ready`: staged in the daemon as `attachmentId`.
   */
  readonly status: DraftStatus;
  readonly progress?: number;
  readonly error?: string;
  /** A `data:image/...` URL. */
  readonly previewUrl?: string;
  /** The preview is the uploaded image itself rather than a smaller rendition. */
  readonly previewExact?: boolean;
  readonly attachmentId?: string;
  readonly dedupe?: string;
  /** The bytes to upload (kept until sent, so an expired staged copy can be uploaded again). */
  readonly blob?: Blob;
  readonly sha256?: string;
  readonly workspace?: { workspaceId: string; path: string; projectName?: string };
};

type Rpc = <M extends RpcMethod>(method: M, params: RpcParams<M>) => Promise<RpcResult<M>>;

export type DraftDeps = {
  call: Rpc;
  prepare: typeof prepareFile;
  digest: (blob: Blob) => Promise<string>;
  newKey: () => string;
  loadImage: (id: string, mime: string) => Promise<string | undefined>;
  seedImage: (id: string, src: string, mime: string, provisional: boolean) => void;
  /** Called after every change, e.g. to keep a registry of drafts by conversation. */
  onChange?: (draft: AttachmentDraft) => void;
};

async function sha256Hex(blob: Blob): Promise<string> {
  return toHex(await crypto.subtle.digest("SHA-256", await blob.arrayBuffer()));
}

export const browserDeps: DraftDeps = {
  call: rpcCall,
  prepare: prepareFile,
  digest: sha256Hex,
  newKey: () => `local-${crypto.randomUUID()}`,
  loadImage: loadAttachmentImage,
  seedImage: seedAttachmentImage,
};

const noop = () => undefined;

/** A kind to show while a file is prepared (the bytes decide it afterwards). */
function provisionalKind(type: string, name: string): DraftKind {
  const lower = name.toLowerCase();
  if (type.startsWith("image/")) return "image";
  if (type === "application/pdf" || lower.endsWith(".pdf")) return "pdf";
  if (type.startsWith("text/")) return "text";
  return "file";
}

function fallbackName(type: string): string {
  if (type === "image/png") return "pasted-image.png";
  if (type === "image/jpeg") return "pasted-image.jpg";
  return "attachment";
}

function kindOf(kind: AttachmentKind): DraftKind {
  return kind === "unknown" ? "file" : kind;
}

export class AttachmentDraft {
  private items: readonly DraftAttachment[] = [];
  private session: string | undefined;
  private readonly listeners = new Set<() => void>();
  private pumping = false;
  private active: { key: string; cancelled: boolean } | null = null;
  private waiters: Array<() => void> = [];
  /**
   * `needed` from `bindExisting` until `restoreStaged` has run: nothing uploads and a send waits, so
   * an upload committed meanwhile cannot be mistaken for a staged copy no draft holds.
   */
  private restore: "none" | "needed" | "running" = "none";
  /**
   * The ids `uploadAll` handed to a send, until `markSent` or `release`. The send outlives the page
   * that started it, so the lock lives here: a remounted page must not remove what is being sent.
   */
  private sendingIds: ReadonlySet<string> | null = null;

  constructor(private readonly deps: DraftDeps = browserDeps) {}

  /** The conversation uploads go to, once bound. */
  get sessionId(): string | undefined {
    return this.session;
  }

  /** A send carries this draft's attachments: they cannot be removed or retried until it settles. */
  get sending(): boolean {
    return this.sendingIds !== null;
  }

  readonly subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  readonly getSnapshot = (): readonly DraftAttachment[] => this.items;

  private notify() {
    this.listeners.forEach((listener) => listener());
  }

  private setItems(next: readonly DraftAttachment[]) {
    this.items = next;
    this.notify();
    this.deps.onChange?.(this);
    this.settle();
  }

  private setSending(ids: ReadonlySet<string> | null) {
    this.sendingIds = ids;
    this.notify();
  }

  private find(key: string): DraftAttachment | undefined {
    return this.items.find((item) => item.key === key);
  }

  private patch(key: string, changes: Partial<DraftAttachment>) {
    if (!this.find(key)) return;
    this.setItems(this.items.map((item) => (item.key === key ? { ...item, ...changes } : item)));
  }

  private drop(key: string) {
    this.setItems(this.items.filter((item) => item.key !== key));
  }

  private sizes(except?: string): number[] {
    return this.items
      .filter((item) => item.key !== except && item.status !== "processing" && item.status !== "failed")
      .map((item) => item.size ?? 0);
  }

  /**
   * Adds picked, pasted, or dropped files. Duplicates (same name, size, and type) are skipped; files
   * over the count are refused. Each file is prepared in turn (images decoded, downscaled, or
   * converted). Returns the refusals to show.
   */
  async add(files: readonly File[]): Promise<string[]> {
    const problems: string[] = [];
    const queued: Array<{ key: string; file: File }> = [];
    const next = [...this.items];
    for (const file of files) {
      const dedupe = dedupeKey(file);
      if (next.some((item) => item.dedupe === dedupe)) continue;
      if (next.length >= MAX_ATTACHMENTS_PER_MESSAGE) {
        problems.push(intakeMessages.tooMany);
        break;
      }
      const key = this.deps.newKey();
      const name = attachmentDisplayName(file.name, fallbackName(file.type));
      next.push({
        key,
        name,
        kind: provisionalKind(file.type, name),
        size: file.size,
        source: "file",
        status: "processing",
        dedupe,
      });
      queued.push({ key, file });
    }
    if (queued.length) this.setItems(next);
    for (const { key, file } of queued) {
      const item = this.find(key);
      if (!item) continue;
      let result: Awaited<ReturnType<typeof prepareFile>>;
      try {
        result = await this.deps.prepare(file, item.name, file.type);
      } catch {
        result = { ok: false, problem: intakeMessages.unreadable(item.name) };
      }
      if (!this.find(key)) continue;
      if (!result.ok) {
        problems.push(result.problem);
        this.drop(key);
        continue;
      }
      const prepared = result.file;
      if (!fitsMessageTotal(this.sizes(key), prepared.blob.size)) {
        problems.push(intakeMessages.messageTotal(prepared.name));
        this.drop(key);
        continue;
      }
      this.patch(key, {
        status: "pending",
        kind: prepared.kind,
        name: prepared.name,
        blob: prepared.blob,
        mime: prepared.mime,
        size: prepared.blob.size,
        previewUrl: prepared.previewUrl,
        previewExact: prepared.previewExact,
      });
      void this.pump();
    }
    return problems;
  }

  /** Attaches a large paste as `pasted-text.txt` (`-2`, …). Returns its name, or a refusal. */
  addPaste(text: string): { name: string } | { problem: string } {
    if (this.items.length >= MAX_ATTACHMENTS_PER_MESSAGE) return { problem: intakeMessages.tooMany };
    const bytes = new TextEncoder().encode(text);
    if (bytes.length === 0) return { problem: intakeMessages.unreadable("pasted text") };
    if (bytes.length > MAX_TEXT_ATTACHMENT_BYTES) return { problem: intakeMessages.pasteTooLarge };
    const name = nextPasteName(this.items.map((item) => item.name));
    if (containsPrivateKey(text)) return { problem: intakeMessages.privateKey(name) };
    if (!fitsMessageTotal(this.sizes(), bytes.length)) return { problem: intakeMessages.messageTotal(name) };
    this.setItems([
      ...this.items,
      {
        key: this.deps.newKey(),
        name,
        kind: "text",
        size: bytes.length,
        mime: "text/plain",
        source: "paste",
        status: "pending",
        blob: new Blob([bytes], { type: "text/plain" }),
      },
    ]);
    void this.pump();
    return { name };
  }

  /** Attaches a file of a registered project; the daemon reads it when the conversation exists. */
  addProjectFile(workspaceId: string, path: string, projectName?: string): string | undefined {
    if (this.items.length >= MAX_ATTACHMENTS_PER_MESSAGE) return intakeMessages.tooMany;
    const dedupe = `project\u0000${workspaceId}\u0000${path}`;
    if (this.items.some((item) => item.dedupe === dedupe)) return undefined;
    const name = attachmentDisplayName(path, "file");
    this.setItems([
      ...this.items,
      {
        key: this.deps.newKey(),
        name,
        kind: provisionalKind("", name),
        source: "workspace",
        status: "pending",
        dedupe,
        workspace: { workspaceId, path, projectName },
      },
    ]);
    void this.pump();
    return undefined;
  }

  /**
   * Removes an attachment: an upload in flight is aborted, a staged copy deleted. One a send carries
   * stays (deleting it would race the send, which could still deliver it).
   */
  remove(key: string): void {
    const item = this.find(key);
    if (!item || (item.attachmentId && this.sendingIds?.has(item.attachmentId))) return;
    this.drop(key);
    if (this.active?.key === key) {
      this.active.cancelled = true;
      return;
    }
    if (item.status === "ready" && item.attachmentId)
      void this.deps.call("attachment.delete", { id: item.attachmentId }).catch(noop);
  }

  /** Uploads a failed attachment again, from the start (not while a send is in flight). */
  retry(key: string): void {
    const item = this.find(key);
    if (item?.status !== "failed" || this.sendingIds) return;
    this.patch(key, { status: "pending", error: undefined, progress: undefined, attachmentId: undefined });
    void this.pump();
  }

  /** Uploads go to `sessionId` from now on; anything pending starts uploading. */
  bind(sessionId: string): void {
    if (this.session === sessionId) return;
    this.session = sessionId;
    this.deps.onChange?.(this);
    void this.pump();
  }

  /**
   * Binds an existing conversation that has no draft in this window (it was opened, the window
   * reloaded, or the app restarted). Uploads wait for `restoreStaged`, which must follow.
   */
  bindExisting(sessionId: string): void {
    this.restore = "needed";
    this.bind(sessionId);
  }

  /**
   * After `bindExisting`: the conversation's staged copies that no draft holds (left by a reload, a
   * restart, or a delete that failed) still count against the daemon's caps, so they come back to
   * the tray as ready, newest first while they fit one message (20 attachments, 32 MB); the rest are
   * deleted. Runs once; a failure (e.g. a locked vault) leaves them to expire.
   */
  async restoreStaged(): Promise<void> {
    const session = this.session;
    if (this.restore !== "needed" || !session) return;
    this.restore = "running";
    try {
      const staged: Attachment[] = [];
      let offset: number | null = 0;
      while (offset !== null) {
        const page: AttachmentList = await this.deps.call("attachment.list", {
          session_id: session,
          offset,
          limit: ATTACHMENT_LIST_LIMIT,
        });
        staged.push(...page.attachments.filter((attachment) => attachment.state === "staged"));
        offset = page.next_offset !== null && page.next_offset > offset ? page.next_offset : null;
      }
      if (this.session === session) this.adoptStaged(staged);
    } catch {
      // They expire on their own.
    } finally {
      this.restore = "none";
      void this.pump();
    }
  }

  /** Adds staged copies (newest first) that fit this message; deletes the others. */
  private adoptStaged(staged: readonly Attachment[]) {
    const held = new Set(this.items.flatMap((item) => (item.attachmentId ? [item.attachmentId] : [])));
    const sizes = this.sizes();
    let count = this.items.length;
    const restored: DraftAttachment[] = [];
    for (const meta of staged) {
      if (held.has(meta.id)) continue;
      held.add(meta.id);
      if (count >= MAX_ATTACHMENTS_PER_MESSAGE || !fitsMessageTotal(sizes, meta.size)) {
        void this.deps.call("attachment.delete", { id: meta.id }).catch(noop);
        continue;
      }
      count += 1;
      sizes.push(meta.size);
      restored.push({
        key: this.deps.newKey(),
        name: meta.name,
        kind: kindOf(meta.kind),
        size: meta.size,
        mime: meta.mime,
        source: meta.source === "paste" ? "paste" : "file",
        status: "ready",
        progress: 1,
        attachmentId: meta.id,
      });
    }
    if (!restored.length) return;
    // Oldest first, ahead of anything added since: the tray's order is the order they are sent in.
    this.setItems([...restored.reverse(), ...this.items]);
    for (const { key, kind, attachmentId, mime } of restored)
      if (kind === "image" && attachmentId && mime) this.loadPreview(key, attachmentId, mime);
  }

  /** Shows a staged image's stored copy as its tray preview. */
  private loadPreview(key: string, id: string, mime: string) {
    void this.deps.loadImage(id, mime).then((src) => {
      if (src && this.find(key)?.attachmentId === id) this.patch(key, { previewUrl: src, previewExact: true });
    });
  }

  private settledNow(): boolean {
    if (this.pumping || this.restore !== "none") return false;
    return !this.items.some(
      (item) =>
        item.status === "processing" || item.status === "uploading" || (item.status === "pending" && !!this.session),
    );
  }

  private settle() {
    if (!this.settledNow() || this.waiters.length === 0) return;
    const waiters = this.waiters;
    this.waiters = [];
    waiters.forEach((resolve) => resolve());
  }

  /** Resolves once nothing is being prepared or uploaded. */
  settled(): Promise<void> {
    if (this.settledNow()) return Promise.resolve();
    return new Promise((resolve) => this.waiters.push(resolve));
  }

  /**
   * Binds the conversation, uploads everything not yet uploaded, and returns the staged ids in tray
   * order. Throws the first failure's message when an attachment could not be uploaded. The returned
   * ids stay locked (see `sending`) until `markSent`, or `release` when the send fails.
   */
  async uploadAll(sessionId: string): Promise<string[]> {
    this.bind(sessionId);
    await this.settled();
    const failed = this.items.find((item) => item.status === "failed");
    if (failed) throw new Error(failed.error ?? "Retry or remove the failed attachment.");
    const ids = this.items.flatMap((item) => (item.status === "ready" && item.attachmentId ? [item.attachmentId] : []));
    this.setSending(new Set(ids));
    return ids;
  }

  /** After a send failed: its attachments can be removed and retried again. */
  release(): void {
    if (this.sendingIds) this.setSending(null);
  }

  private async pump(): Promise<void> {
    if (this.pumping) return;
    this.pumping = true;
    try {
      for (;;) {
        const session = this.session;
        const next =
          session && this.restore === "none" ? this.items.find((item) => item.status === "pending") : undefined;
        if (!session || !next) break;
        await this.upload(next, session);
      }
    } finally {
      this.pumping = false;
      this.settle();
    }
  }

  private async upload(item: DraftAttachment, session: string): Promise<void> {
    const { call } = this.deps;
    const token = { key: item.key, cancelled: false };
    this.active = token;
    this.patch(item.key, { status: "uploading", progress: 0, error: undefined });
    let uploadId: string | undefined;
    let committed: Attachment | undefined;
    try {
      if (item.workspace) {
        committed = await call("attachment.from_workspace", {
          session_id: session,
          workspace_id: item.workspace.workspaceId,
          path: item.workspace.path,
        });
      } else {
        const blob = item.blob;
        if (!blob) throw new Error("The file is no longer available. Remove it and attach it again.");
        const sha256 = item.sha256 ?? (await this.deps.digest(blob));
        if (!item.sha256) this.patch(item.key, { sha256 });
        const ticket = await call("attachment.upload.begin", {
          session_id: session,
          name: item.name,
          size: blob.size,
          sha256,
          ...(item.mime ? { mime: item.mime } : {}),
          source: item.source === "paste" ? "paste" : "file",
        });
        uploadId = ticket.upload_id;
        for (const [start, end] of chunkRanges(blob.size, ticket.chunk_bytes)) {
          if (token.cancelled) break;
          const bytes = new Uint8Array(await blob.slice(start, end).arrayBuffer());
          await call("attachment.upload.chunk", { upload_id: uploadId, offset: start, data: bytesToBase64(bytes) });
          this.patch(item.key, { progress: end / blob.size });
        }
        if (!token.cancelled) committed = await call("attachment.upload.commit", { upload_id: uploadId });
      }
      if (token.cancelled) {
        if (committed) void call("attachment.delete", { id: committed.id }).catch(noop);
        else if (uploadId) void call("attachment.upload.abort", { upload_id: uploadId }).catch(noop);
        return;
      }
      const meta = committed!;
      this.patch(item.key, {
        status: "ready",
        progress: 1,
        attachmentId: meta.id,
        kind: kindOf(meta.kind),
        name: meta.name,
        size: meta.size,
        mime: meta.mime,
      });
      if (meta.kind === "image" && !item.previewUrl) this.loadPreview(item.key, meta.id, meta.mime);
    } catch (caught) {
      if (uploadId && !committed) void call("attachment.upload.abort", { upload_id: uploadId }).catch(noop);
      if (token.cancelled) return;
      this.patch(item.key, {
        status: "failed",
        progress: undefined,
        // Retrying cannot help a file that changed on disk; intake copies new files into memory.
        error: isFileReadError(caught)
          ? intakeMessages.changedOnDisk(item.name)
          : uploadFailureMessage(item.name, item.kind, toAidexError(caught)),
      });
    } finally {
      if (this.active === token) this.active = null;
    }
  }

  /**
   * After a send was refused as `attachment_unavailable`: staged copies that are gone (for example
   * swept after a day) go back to `pending` and upload again. Returns whether any did.
   */
  async revalidate(): Promise<boolean> {
    let changed = false;
    for (const item of this.items) {
      if (item.status !== "ready" || !item.attachmentId) continue;
      try {
        const meta = await this.deps.call("attachment.get", { id: item.attachmentId });
        if (meta.state === "staged") continue;
      } catch (caught) {
        if (toAidexError(caught).kind !== "not_found") continue;
      }
      this.patch(item.key, { status: "pending", attachmentId: undefined, progress: undefined });
      changed = true;
    }
    if (changed) void this.pump();
    return changed;
  }

  /** Clears the attachments a send carried; their previews show in the transcript until it loads them. */
  markSent(ids: readonly string[]): void {
    const sent = new Set(ids);
    const leaving = this.items.filter((item) => item.attachmentId && sent.has(item.attachmentId));
    for (const item of leaving)
      if (item.kind === "image" && item.previewUrl && item.attachmentId)
        this.deps.seedImage(item.attachmentId, item.previewUrl, item.mime ?? "image/png", !item.previewExact);
    const wasSending = this.sendingIds !== null;
    this.sendingIds = null;
    if (leaving.length) this.setItems(this.items.filter((item) => !leaving.includes(item)));
    else if (wasSending) this.notify();
  }

  /** Hands every attachment over (Home → the new chat) and empties this draft. */
  takeItems(): DraftAttachment[] {
    const items = this.items.filter((item) => item.status !== "processing");
    this.setItems([]);
    return items;
  }

  /** Takes over attachments prepared elsewhere (they upload once a conversation is bound). */
  adoptItems(items: readonly DraftAttachment[]): void {
    const taken = new Set(this.items.map((item) => item.dedupe).filter(Boolean));
    const fresh = items
      .filter((item) => !item.dedupe || !taken.has(item.dedupe))
      .map((item) => ({ ...item, status: "pending" as const, attachmentId: undefined, progress: undefined }));
    if (!fresh.length) return;
    this.setItems([...this.items, ...fresh].slice(0, MAX_ATTACHMENTS_PER_MESSAGE));
    void this.pump();
  }
}
