import { useCallback, useEffect, useState, useSyncExternalStore } from "react";
import type { AttachmentTrayItem } from "@aidex/ui/components";
import { AttachmentDraft, browserDeps, type DraftAttachment } from "./attachmentDraft";

// Drafts of existing conversations, kept while they hold attachments. Window-local (never stored),
// they outlive the page remount after a new chat's first send and a Chat → Agent route change, and
// uploads keep running while another screen is open.
const drafts = new Map<string, AttachmentDraft>();

function track(draft: AttachmentDraft) {
  const id = draft.sessionId;
  if (!id) return;
  if (draft.getSnapshot().length > 0) drafts.set(id, draft);
  else if (drafts.get(id) === draft) drafts.delete(id);
}

const trackedDeps = { ...browserDeps, onChange: track };

/**
 * The attachments of a conversation's composer. A new chat (no id) starts with `arrival` (handed over
 * from Home) and uploads nothing until `adopt` binds the conversation its first send created; an
 * existing conversation uploads as soon as files are added. An existing conversation without a draft
 * in this window first gets back its staged copies (`restoreStaged`), which would otherwise be
 * invisible yet still count against the daemon's caps.
 */
export function useConversationAttachments(
  conversationId: string | undefined,
  arrival?: readonly DraftAttachment[] | null,
) {
  const [draft] = useState(() => {
    const existing = conversationId ? drafts.get(conversationId) : undefined;
    if (existing) return existing;
    const created = new AttachmentDraft(trackedDeps);
    if (conversationId) created.bindExisting(conversationId);
    else if (arrival?.length) created.adoptItems(arrival);
    return created;
  });
  // Outside the initializer, which may run twice; the draft restores only once.
  useEffect(() => {
    void draft.restoreStaged();
  }, [draft]);
  const items = useSyncExternalStore(draft.subscribe, draft.getSnapshot, draft.getSnapshot);
  // A send in flight, possibly started by an earlier mount of this page: the tray and Send wait for it.
  const sendingSnapshot = useCallback(() => draft.sending, [draft]);
  const sending = useSyncExternalStore(draft.subscribe, sendingSnapshot, sendingSnapshot);
  /** Binds the conversation a new chat's first send created; pending attachments start uploading. */
  const adopt = useCallback((createdId: string) => draft.bind(createdId), [draft]);
  return { draft, items, sending, adopt };
}

/** Attachments of a composer that never uploads (Home): prepared here, handed to the new chat. */
export function useDraftAttachments() {
  const [draft] = useState(() => new AttachmentDraft(browserDeps));
  const items = useSyncExternalStore(draft.subscribe, draft.getSnapshot, draft.getSnapshot);
  return { draft, items };
}

/** How the composer tray shows a draft attachment. */
export function trayItemOf(item: DraftAttachment): AttachmentTrayItem {
  const waitingForProject = item.source === "workspace" && item.status === "pending";
  return {
    id: item.key,
    name: item.name,
    kind: item.kind,
    size: item.size,
    previewUrl: item.previewUrl,
    status: item.status === "processing" ? "pending" : item.status,
    progress: item.progress,
    error: item.error,
    statusText:
      item.status === "processing"
        ? "Preparing…"
        : waitingForProject
          ? `From ${item.workspace?.projectName ?? "project"}`
          : undefined,
  };
}
