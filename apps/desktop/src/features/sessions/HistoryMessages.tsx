import { useEffect, useState } from "react";
import { ChatMessage, CodeBlock, Notice } from "@aidex/ui/components";
import { Button, Stack, Text } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import type {
  ContentChunk,
  HistoryScope,
  Message,
} from "@aidex/client/dto/sessions";
import { useRpc } from "../../hooks/useRpc";

export function MessageContent({
  message,
  scope,
}: {
  message: Message;
  scope: HistoryScope;
}) {
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
  return (
    <ChatMessage
      role={message.role === "user" ? "user" : "assistant"}
      meta={
        message.role === "tool"
          ? `Tool ${message.tool_call_id ?? "result"}`
          : undefined
      }
    >
      {message.role === "tool" ? (
        <CodeBlock
          title="Saved tool output"
          code={chunk?.content ?? message.content}
        />
      ) : (
        <Text>{chunk?.content ?? message.content}</Text>
      )}
      {message.content_truncated && (
        <Stack direction="row" gap={2}>
          <Text tone="muted">
            Showing an excerpt of {message.content_bytes} bytes.
          </Text>
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
          Bytes {chunk.offset}–
          {chunk.offset + new TextEncoder().encode(chunk.content).length} of{" "}
          {chunk.total_bytes}
        </Text>
      )}
      {message.tool_calls.map((tool, index) => (
        <Stack key={tool.id} gap={2}>
          <CodeBlock title={tool.name} language="json" code={tool.arguments} />
          {tool.arguments_truncated && (
            <ToolArguments
              scope={scope}
              index={message.original_index}
              callIndex={index}
            />
          )}
        </Stack>
      ))}
      {message.tool_calls_truncated && (
        <Notice tone="warning">
          Additional tool calls are stored in history. This summary shows{" "}
          {message.tool_calls.length} of {message.tool_calls_total}.
        </Notice>
      )}
      {error && <Notice tone="danger">{error}</Notice>}
    </ChatMessage>
  );
}
function ToolArguments({
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
      field: "arguments",
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
export function HistoryMessages({
  scope,
  live = false,
}: {
  scope: HistoryScope;
  live?: boolean;
}) {
  const [offset, setOffset] = useState(0);
  const history = useRpc("history.list", { ...scope, offset, limit: 20 });
  useEffect(() => {
    history.refetch();
    if (!live) return;
    const timer = setInterval(history.refetch, 1000);
    return () => clearInterval(timer);
  }, [live, history.refetch]);
  return (
    <Stack gap={4}>
      {history.error && <Notice tone="danger">{history.error.message}</Notice>}
      {history.data?.messages
        .filter((message) => message.role !== "system")
        .map((message) => (
          <MessageContent
            key={message.original_index}
            message={message}
            scope={scope}
          />
        ))}
      <Stack direction="row" gap={2}>
        <Button
          variant="outline"
          disabled={offset === 0}
          onClick={() => setOffset(Math.max(0, offset - 20))}
        >
          Previous messages
        </Button>
        <Text>
          {offset + (history.data?.messages.length ? 1 : 0)}–
          {offset + (history.data?.messages.length ?? 0)} of{" "}
          {history.data?.total_messages ?? 0}
        </Text>
        <Button
          variant="outline"
          disabled={history.data?.next_offset == null}
          onClick={() => setOffset(history.data!.next_offset!)}
        >
          Next messages
        </Button>
      </Stack>
    </Stack>
  );
}
