import { Fragment, useMemo, type ReactNode } from "react";
import { Stack } from "@aidex/ui/primitives";
import type { Message } from "@aidex/client/dto/sessions";
import { ArtifactResultRow, PlanProposalRow } from "./ArtifactRows";
import { MessageContent } from "./MessageContent";
import { McpRow, SkillLoadRow, WebFetchRow, WebSearchGroupRow, WebSearchRow } from "./ToolRows";
import {
  buildTranscript,
  type ToolEntry,
  type TranscriptItem,
  type TranscriptOptions,
} from "./transcriptModel";

function ToolEntryView({ entry }: { entry: ToolEntry }) {
  switch (entry.kind) {
    case "web_search":
      return <WebSearchRow entry={entry} />;
    case "web_fetch":
      return <WebFetchRow entry={entry} />;
    case "mcp":
      return <McpRow entry={entry} />;
    case "skill_load":
      return <SkillLoadRow entry={entry} />;
    case "artifact_create":
    case "artifact_update":
      return <ArtifactResultRow entry={entry} />;
    case "plan_propose":
      return <PlanProposalRow entry={entry} />;
  }
}

/** Work-log rows sit close together; messages and cards keep the transcript's spacing. */
function isWorkLogRow(item: TranscriptItem): boolean {
  if (item.type === "web-searches") return true;
  return item.type === "tool" && ["web_search", "web_fetch", "mcp", "skill_load"].includes(item.entry.kind);
}

function renderItem(item: TranscriptItem): ReactNode {
  if (item.type === "message")
    return <MessageContent key={item.key} message={item.message} hiddenCalls={item.hiddenCalls} />;
  if (item.type === "tool") return <ToolEntryView key={item.key} entry={item.entry} />;
  return <WebSearchGroupRow key={item.key} entries={item.entries} />;
}

/** History index of the message an item shows (a tool row's calling message). */
function itemIndex(item: TranscriptItem): number {
  if (item.type === "message") return item.message.original_index;
  if (item.type === "tool") return item.entry.owner.original_index;
  return item.entries[0].owner.original_index;
}

/** Something shown between messages, such as a divider where the conversation changed mode. */
export type TranscriptMarker = {
  key: string;
  /**
   * History index (`Message.original_index`) the marker precedes. A marker before the first shown
   * message is left out (it belongs to earlier history); one past the last shows at the end.
   */
  index: number;
  node: ReactNode;
};

/**
 * Messages as a transcript: chat bubbles and Markdown replies, with web, MCP, skill, artifact, and
 * plan tool calls as work-log rows and cards. Other tools keep their arguments and output inline.
 * `markers` sit between messages by history index. Needs a `TranscriptProvider`.
 */
export function Transcript({
  messages,
  options,
  markers = [],
}: {
  messages: readonly Message[];
  options: TranscriptOptions;
  markers?: readonly TranscriptMarker[];
}) {
  const items = useMemo(
    () => buildTranscript(messages, options),
    [messages, options.running, options.complete],
  );
  const blocks: ReactNode[] = [];
  let rows: TranscriptItem[] = [];
  const flush = () => {
    if (rows.length)
      blocks.push(
        <Stack key={`rows-${rows[0].key}`} gap={1}>
          {rows.map(renderItem)}
        </Stack>,
      );
    rows = [];
  };
  const first = messages[0]?.original_index ?? 0;
  const pending = [...markers].filter((marker) => marker.index >= first).sort((a, b) => a.index - b.index);
  const emitMarkers = (upTo: number) => {
    if (!pending.length || pending[0].index > upTo) return;
    flush();
    while (pending.length && pending[0].index <= upTo) {
      const marker = pending.shift()!;
      blocks.push(<Fragment key={`marker-${marker.key}`}>{marker.node}</Fragment>);
    }
  };
  for (const item of items) {
    emitMarkers(itemIndex(item));
    if (isWorkLogRow(item)) rows.push(item);
    else {
      flush();
      blocks.push(renderItem(item));
    }
  }
  emitMarkers(Number.POSITIVE_INFINITY);
  flush();
  return <>{blocks}</>;
}
