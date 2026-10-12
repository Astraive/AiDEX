import { useEffect, useState } from "react";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import type { ContentChunk, HistoryScope, Message } from "@aidex/client/dto/sessions";

/** Most history text loaded for one tool result (the harness caps MCP output at 256 KiB). */
const MAX_CONTENT_BYTES = 512 * 1024;

export type FullContent = {
  /** The whole content, or as much as was loaded. */
  content: string | undefined;
  /** False when the content stopped at the size cap. */
  complete: boolean;
  loading: boolean;
  error?: string;
};

/**
 * A message's full content. Views carry a bounded excerpt; when `message.content_truncated`, the rest
 * is read from history in pages while `enabled` (e.g. once a work-log row is expanded).
 */
export function useFullContent(message: Message | undefined, scope: HistoryScope, enabled = true): FullContent {
  const truncated = !!message?.content_truncated;
  const [state, setState] = useState<{ key: string; content?: string; complete: boolean; error?: string }>({
    key: "",
    complete: false,
  });
  const key = message ? `${scope.session_id ?? ""}|${scope.run_id ?? ""}|${message.original_index}` : "";

  useEffect(() => {
    const settled = state.key === key && (state.content !== undefined || !!state.error);
    if (!message || !truncated || !enabled || settled) return;
    let current = true;
    setState({ key, complete: false });
    (async () => {
      let content = "";
      let offset: number | null = 0;
      while (offset !== null && content.length < MAX_CONTENT_BYTES) {
        const chunk: ContentChunk = await call("history.read", { ...scope, index: message.original_index, offset });
        content += chunk.content;
        offset = chunk.next_offset;
      }
      if (current) setState({ key, content, complete: offset === null });
    })().catch((caught) => {
      if (current) setState({ key, complete: false, error: toAidexError(caught).message });
    });
    return () => {
      current = false;
    };
    // `scope` is compared through `key`.
  }, [key, truncated, enabled]);

  if (!message) return { content: undefined, complete: false, loading: false };
  if (!truncated) return { content: message.content, complete: true, loading: false };
  const fresh = state.key === key;
  return {
    content: fresh ? state.content : undefined,
    complete: fresh && state.complete,
    loading: enabled && (!fresh || (state.content === undefined && !state.error)),
    error: fresh ? state.error : undefined,
  };
}
