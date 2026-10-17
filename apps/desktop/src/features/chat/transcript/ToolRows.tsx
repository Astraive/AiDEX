import {
  ArtifactCard,
  CodeBlock,
  Markdown,
  Notice,
  WebResultList,
  WorkLogGroup,
  WorkLogRow,
  urlHost,
  webSearchSummary,
} from "@aidex/ui/components";
import { DownloadIcon, GlobeIcon, PlugIcon, SparklesIcon } from "@aidex/ui/icons";
import { Button, Stack, Text } from "@aidex/ui/primitives";
import type { WebFetchToolResult, WebSearchToolResult } from "@aidex/client/dto/web";
import { ToolArguments } from "./MessageContent";
import { useTranscript } from "./TranscriptContext";
import {
  groupOutcome,
  jsonStringField,
  parseJson,
  parseMcpToolName,
  toolOutcome,
  type ToolEntry,
} from "./transcriptModel";
import { useFullContent } from "./useFullContent";

/** Shown in place of a detail that is still loading or could not be read. */
function DetailState({ loading, error, failed }: { loading: boolean; error?: string; failed?: string }) {
  if (error) return <Notice tone="danger">{error}</Notice>;
  if (loading) return <Text tone="muted">Loading…</Text>;
  return <Text tone="muted">{failed ?? "This result could not be read."}</Text>;
}

/** The failure text a tool stored as its result; as a `WorkLogRow`'s detail, a long one is read once the row expands. */
export function FailureDetail({ entry }: { entry: ToolEntry }) {
  const { scope } = useTranscript();
  const full = useFullContent(entry.result, scope);
  if (!entry.result) return null;
  if (full.content === undefined) return <DetailState loading={full.loading} error={full.error} />;
  return <Text tone="muted">{full.content}</Text>;
}

/** "Searched the web" with the query; expands to the results. */
export function WebSearchRow({ entry }: { entry: ToolEntry }) {
  const outcome = toolOutcome(entry);
  const query = jsonStringField(entry.call.arguments, "query");
  return (
    <WorkLogRow
      icon={<GlobeIcon />}
      label={outcome.status === "running" ? "Searching the web" : webSearchSummary(1)}
      secondary={query ?? undefined}
      status={outcome.status}
      statusText={outcome.statusText}
    >
      {entry.result ? (
        outcome.status === "failed" ? <FailureDetail entry={entry} /> : <WebSearchResults entry={entry} />
      ) : undefined}
    </WorkLogRow>
  );
}

function WebSearchResults({ entry }: { entry: ToolEntry }) {
  const { scope, openLink } = useTranscript();
  const full = useFullContent(entry.result, scope);
  const parsed = parseJson<WebSearchToolResult>(full.content);
  if (!parsed || !Array.isArray(parsed.results))
    return <DetailState loading={full.loading} error={full.error} />;
  return (
    <WebResultList
      results={parsed.results}
      onOpenLink={openLink}
      emptyText={`No results for “${parsed.query}”.`}
    />
  );
}

/** Consecutive searches: "Searched the web N times", expanding to each search. */
export function WebSearchGroupRow({ entries }: { entries: readonly ToolEntry[] }) {
  const outcome = groupOutcome(entries);
  return (
    <WorkLogGroup
      icon={<GlobeIcon />}
      label={webSearchSummary(entries.length)}
      status={outcome.status}
      statusText={outcome.statusText}
    >
      {entries.map((entry) => (
        <WebSearchRow key={entry.key} entry={entry} />
      ))}
    </WorkLogGroup>
  );
}

/** The path and query of a URL, as the muted detail next to its host. */
function urlPath(url: string | null): string | undefined {
  if (!url) return undefined;
  try {
    const parsed = new URL(url);
    const path = `${parsed.pathname}${parsed.search}`;
    return path === "/" ? undefined : path;
  } catch {
    return undefined;
  }
}

/** "Read <host>"; expands to the page title, link, and the saved artifact for long pages. */
export function WebFetchRow({ entry }: { entry: ToolEntry }) {
  const outcome = toolOutcome(entry);
  const url = jsonStringField(entry.call.arguments, "url");
  const host = urlHost(url) ?? url ?? "a web page";
  return (
    <WorkLogRow
      icon={<GlobeIcon />}
      label={outcome.status === "running" ? `Reading ${host}` : `Read ${host}`}
      secondary={urlPath(url)}
      status={outcome.status}
      statusText={outcome.statusText}
    >
      {entry.result ? (
        outcome.status === "failed" ? <FailureDetail entry={entry} /> : <WebFetchDetail entry={entry} />
      ) : undefined}
    </WorkLogRow>
  );
}

function excerpt(text: string | undefined, max = 280): string | undefined {
  if (!text) return undefined;
  const flat = text.replace(/\s+/g, " ").trim();
  return flat.length > max ? `${flat.slice(0, max - 1)}…` : flat;
}

function WebFetchDetail({ entry }: { entry: ToolEntry }) {
  const { scope, openLink, openArtifact, downloadArtifact } = useTranscript();
  const full = useFullContent(entry.result, scope);
  const parsed = parseJson<WebFetchToolResult>(full.content);
  const finalUrl = parsed?.final_url ?? jsonStringField(full.content, "final_url");
  if (!finalUrl) return <DetailState loading={full.loading} error={full.error} />;
  const title = parsed?.title ?? jsonStringField(full.content, "title");
  const facts = [
    parsed ? `HTTP ${parsed.status}` : null,
    parsed?.content_type || null,
    parsed?.truncated ? "cut at the size limit" : null,
  ].filter(Boolean);
  return (
    <Stack gap={3}>
      <WebResultList
        results={[{ title: title ?? undefined, url: finalUrl, snippet: excerpt(parsed?.text) }]}
        onOpenLink={openLink}
      />
      {facts.length > 0 && <Text tone="muted">{facts.join(" · ")}</Text>}
      {parsed?.artifact && (
        <ArtifactCard
          name={parsed.artifact.name}
          kind="web_page"
          size={parsed.artifact.size}
          detail="Full page"
          titleLevel={4}
          onOpen={() => openArtifact({ id: parsed.artifact!.id, name: parsed.artifact!.name })}
          actions={
            <Button
              variant="ghost"
              icon={<DownloadIcon />}
              onClick={() => downloadArtifact(parsed.artifact!.id)}
            >
              Download
            </Button>
          }
        />
      )}
      {parsed?.artifact_error && (
        <Notice tone="warning">The full page could not be saved: {parsed.artifact_error}</Notice>
      )}
    </Stack>
  );
}

/** "<server> · <tool>"; expands to the arguments and the server's output. */
export function McpRow({ entry }: { entry: ToolEntry }) {
  const { scope } = useTranscript();
  const outcome = toolOutcome(entry);
  const name = parseMcpToolName(entry.call.name);
  return (
    <WorkLogRow
      icon={<PlugIcon />}
      label={name ? `${name.server} · ${name.tool}` : entry.call.name}
      status={outcome.status}
      statusText={outcome.statusText}
    >
      <Stack gap={3}>
        <CodeBlock title="Arguments" language="json" code={prettyJson(entry.call.arguments)} />
        {entry.call.arguments_truncated && (
          <ToolArguments scope={scope} index={entry.owner.original_index} callIndex={entry.callIndex} />
        )}
        {entry.result && <McpOutput entry={entry} />}
      </Stack>
    </WorkLogRow>
  );
}

function prettyJson(text: string): string {
  const parsed = parseJson<unknown>(text);
  return parsed === null ? text : JSON.stringify(parsed, null, 2);
}

function McpOutput({ entry }: { entry: ToolEntry }) {
  const { scope } = useTranscript();
  const full = useFullContent(entry.result, scope);
  if (full.content === undefined) return <DetailState loading={full.loading} error={full.error} />;
  return (
    <>
      <CodeBlock title="Output" code={full.content} />
      {!full.complete && <Notice tone="warning">Only the start of a very long output is shown.</Notice>}
    </>
  );
}

/** "Loaded skill <name>"; expands to the instructions the model read. */
export function SkillLoadRow({ entry }: { entry: ToolEntry }) {
  const outcome = toolOutcome(entry);
  const name = jsonStringField(entry.call.arguments, "name") ?? "skill";
  const label =
    outcome.status === "running"
      ? `Loading skill ${name}`
      : outcome.status === "failed"
        ? `Load skill ${name}`
        : `Loaded skill ${name}`;
  return (
    <WorkLogRow icon={<SparklesIcon />} label={label} status={outcome.status} statusText={outcome.statusText}>
      {entry.result ? (
        outcome.status === "failed" ? <FailureDetail entry={entry} /> : <SkillBody entry={entry} />
      ) : undefined}
    </WorkLogRow>
  );
}

function SkillBody({ entry }: { entry: ToolEntry }) {
  const { scope, openLink } = useTranscript();
  const full = useFullContent(entry.result, scope);
  if (full.content === undefined) return <DetailState loading={full.loading} error={full.error} />;
  return <Markdown source={full.content} onOpenLink={openLink} breaks />;
}
