import { useState } from "react";
import {
  ChatMessage,
  CodeBlock,
  InlineChip,
  Markdown,
  Notice,
  renderSkillTokens,
} from "@aidex/ui/components";
import { SparklesIcon } from "@aidex/ui/icons";
import { Button, Stack, Text } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import { ATTACHMENT_ONLY_CONTENT } from "@aidex/client/dto/attachments";
import type { ContentChunk, HistoryScope, Message } from "@aidex/client/dto/sessions";
import { SentAttachments } from "../attachments/SentAttachments";
import { skillNameOf } from "../composer/composerText";
import { useTranscript } from "./TranscriptContext";

/**
 * One chat message: user text with `$skill` chips and the attachments sent with it, assistant text as
 * Markdown, other tool output as text. Calls not listed in `hiddenCalls` show their arguments; long
 * content reads in pages.
 */
export function MessageContent({
  message,
  hiddenCalls,
}: {
  message: Message;
  hiddenCalls?: ReadonlySet<number>;
}) {
  const { scope, openLink } = useTranscript();
  const [chunk, setChunk] = useState<ContentChunk>();
  const [error, setError] = useState<string>();
  const read = (offset = 0) =>
    void call("history.read", {
      ...scope,
      index: message.original_index,
      offset,
    })
      .then(setChunk)
      .catch((caught) => setError(toAidexError(caught).message));
  const text = chunk?.content ?? message.content;
  const attachments = message.role === "user" ? (message.attachments ?? []) : [];
  // A message sent with attachments only stores a fixed marker as its text.
  const attachmentsOnly = attachments.length > 0 && message.content === ATTACHMENT_ONLY_CONTENT;
  return (
    <ChatMessage
      role={message.role === "user" ? "user" : "assistant"}
      meta={message.role === "tool" ? `Tool ${message.tool_call_id ?? "result"}` : undefined}
    >
      {message.role === "assistant" ? (
        text ? (
          <Markdown source={text} onOpenLink={openLink} />
        ) : null
      ) : message.role === "user" ? (
        <>
          {attachments.length > 0 && <SentAttachments attachments={attachments} />}
          {!attachmentsOnly && (
            <Text>
              <UserText text={text} skills={message.skills} />
            </Text>
          )}
        </>
      ) : (
        <Text>{text}</Text>
      )}
      {message.content_truncated && (
        <Stack direction="row" gap={2}>
          <Text tone="muted">Showing an excerpt of {message.content_bytes} bytes.</Text>
          <Button variant="ghost" onClick={() => read()}>
            Read full content in pages
          </Button>
          {chunk?.next_offset !== null && chunk?.next_offset !== undefined && (
            <Button variant="ghost" onClick={() => read(chunk.next_offset!)}>
              Next content page
            </Button>
          )}
        </Stack>
      )}
      {chunk && (
        <Text tone="muted">
          Bytes {chunk.offset}–{chunk.offset + new TextEncoder().encode(chunk.content).length} of{" "}
          {chunk.total_bytes}
        </Text>
      )}
      {message.tool_calls.map((tool, index) =>
        hiddenCalls?.has(index) ? null : (
          <Stack key={tool.id} gap={2}>
            <CodeBlock title={tool.name} language="json" code={tool.arguments} />
            {tool.arguments_truncated && (
              <ToolArguments scope={scope} index={message.original_index} callIndex={index} />
            )}
          </Stack>
        ),
      )}
      {message.tool_calls_truncated && (
        <Notice tone="warning">
          Additional tool calls are stored in history. This summary shows {message.tool_calls.length} of{" "}
          {message.tool_calls_total}.
        </Notice>
      )}
      {error && <Notice tone="danger">{error}</Notice>}
    </ChatMessage>
  );
}

/** User text with each invoked `$skill` mention as a chip; other `$words` stay text. */
function UserText({ text, skills }: { text: string; skills?: string[] }) {
  const { skill } = useTranscript();
  if (!skills?.length) return <>{text}</>;
  return (
    <>
      {renderSkillTokens(text, (name, raw) => {
        const id = skills.find((candidate) => skillNameOf(candidate) === name);
        if (!id) return null;
        const info = skill?.(id);
        return (
          <InlineChip
            label={info?.display_name ?? name}
            icon={<SparklesIcon />}
            kindLabel="Skill"
            rawText={raw}
            tooltip={info ? `${raw} — ${info.description}` : `${raw} (${id})`}
          />
        );
      })}
    </>
  );
}

/** Pages through a call's complete arguments when the view holds only an excerpt. */
export function ToolArguments({
  scope,
  index,
  callIndex,
}: {
  scope: HistoryScope;
  index: number;
  callIndex: number;
}) {
  const [chunk, setChunk] = useState<ContentChunk>();
  const [error, setError] = useState<string>();
  const read = (offset = 0) =>
    void call("history.read", {
      ...scope,
      index,
      field: "tool_arguments",
      call_index: callIndex,
      offset,
    })
      .then(setChunk)
      .catch((caught) => setError(toAidexError(caught).message));
  return (
    <Stack gap={2}>
      <Button variant="ghost" onClick={() => read()}>
        Read complete arguments in pages
      </Button>
      {chunk && <CodeBlock language="json" code={chunk.content} />}{" "}
      {chunk?.next_offset != null && (
        <Button variant="ghost" onClick={() => read(chunk.next_offset!)}>
          Next arguments page
        </Button>
      )}
      {error && <Notice tone="danger">{error}</Notice>}
    </Stack>
  );
}
