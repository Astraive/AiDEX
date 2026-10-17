import { useEffect, useState } from "react";
import { ArtifactCard, Notice, PlanCard, WorkLogRow } from "@aidex/ui/components";
import { ClipboardCheckIcon, CopyIcon, DownloadIcon, FileTextIcon } from "@aidex/ui/icons";
import { Button, IconButton } from "@aidex/ui/primitives";
import { toAidexError } from "@aidex/client/errors";
import type { ArtifactMeta, ArtifactToolResult, PlanToolResult } from "@aidex/client/dto/artifacts";
import { useToast } from "../../../app/providers/toast";
import { artifactFileName, downloadText, readArtifact } from "../artifacts/artifactContent";
import { FailureDetail } from "./ToolRows";
import { useTranscript } from "./TranscriptContext";
import { jsonStringField, parseJson, toolOutcome, type ToolEntry } from "./transcriptModel";
import { useFullContent } from "./useFullContent";

/**
 * A pending or failed producer call, before there is an artifact to show. A failure's text is the
 * row's detail, so a truncated one is read from history only once the row is expanded.
 */
function PendingRow({ entry, running, failed }: { entry: ToolEntry; running: string; failed: string }) {
  const outcome = toolOutcome(entry);
  return (
    <WorkLogRow
      icon={entry.kind === "plan_propose" ? <ClipboardCheckIcon /> : <FileTextIcon />}
      label={outcome.status === "running" ? running : failed}
      status={outcome.status}
      statusText={outcome.statusText}
    >
      {outcome.status === "failed" && entry.result ? <FailureDetail entry={entry} /> : undefined}
    </WorkLogRow>
  );
}

/** `artifact_create` / `artifact_update`: the artifact as a card with Open and Download. */
export function ArtifactResultRow({ entry }: { entry: ToolEntry }) {
  const outcome = toolOutcome(entry);
  const name = jsonStringField(entry.call.arguments, "name") ?? "artifact";
  if (outcome.status !== "completed" || !entry.result)
    return entry.kind === "artifact_update" ? (
      <PendingRow entry={entry} running="Updating an artifact" failed="Update an artifact" />
    ) : (
      <PendingRow entry={entry} running={`Creating ${name}`} failed={`Create ${name}`} />
    );
  return <ArtifactResultCard entry={entry} />;
}

function ArtifactResultCard({ entry }: { entry: ToolEntry }) {
  const { scope } = useTranscript();
  const full = useFullContent(entry.result, scope);
  const parsed = parseJson<ArtifactToolResult>(full.content);
  if (!parsed?.artifact) {
    if (full.loading || full.error) return full.error ? <Notice tone="danger">{full.error}</Notice> : null;
    return <WorkLogRow icon={<FileTextIcon />} label="Saved an artifact" />;
  }
  return <ArtifactResultCardView meta={parsed.artifact} updated={entry.kind === "artifact_update"} />;
}

function ArtifactResultCardView({ meta, updated }: { meta: ArtifactMeta; updated: boolean }) {
  const { openArtifact, downloadArtifact } = useTranscript();
  const open = () => openArtifact({ id: meta.id, name: meta.name, meta });
  return (
    <ArtifactCard
      name={meta.name}
      kind={meta.kind}
      size={meta.size}
      createdAt={updated ? meta.updated_at : meta.created_at}
      detail={updated ? "Updated" : (meta.language ?? undefined)}
      onOpen={open}
      actions={
        <>
          <Button variant="ghost" onClick={open}>
            Open
          </Button>
          <Button variant="ghost" icon={<DownloadIcon />} onClick={() => downloadArtifact(meta.id)}>
            Download
          </Button>
        </>
      }
    />
  );
}

/** The plan text the model proposed, as the harness stored it (a leading `# title` is added when missing). */
function planFromArguments(entry: ToolEntry): string | null {
  if (entry.call.arguments_truncated) return null;
  const args = parseJson<{ title?: string; markdown?: string }>(entry.call.arguments);
  if (!args || typeof args.markdown !== "string") return null;
  if (args.markdown.trimStart().startsWith("#")) return args.markdown;
  return `# ${(args.title ?? "").trim()}\n\n${args.markdown}`;
}

/** `plan_propose`: a plan card with Copy, Download, and (in a live conversation) Implement actions. */
export function PlanProposalRow({ entry }: { entry: ToolEntry }) {
  const outcome = toolOutcome(entry);
  if (outcome.status !== "completed" || !entry.result)
    return <PendingRow entry={entry} running="Proposing a plan" failed="Propose a plan" />;
  return <PlanProposalCard entry={entry} />;
}

function PlanProposalCard({ entry }: { entry: ToolEntry }) {
  const { scope, planStatus, planActions, openLink } = useTranscript();
  const notify = useToast();
  const full = useFullContent(entry.result, scope);
  const parsed = parseJson<PlanToolResult>(full.content);
  const meta = parsed?.artifact;
  const fromArguments = planFromArguments(entry);
  const [loaded, setLoaded] = useState<{ markdown?: string; error?: string }>({});

  // Plans longer than the call's argument excerpt are read from their artifact.
  useEffect(() => {
    if (fromArguments !== null || !meta) return;
    let current = true;
    readArtifact(meta.id, 512 * 1024).then(
      (data) => current && setLoaded({ markdown: data.encoding === "utf8" ? data.text : "" }),
      (caught) => current && setLoaded({ error: toAidexError(caught).message }),
    );
    return () => {
      current = false;
    };
  }, [fromArguments, meta?.id]);

  if (!meta) {
    if (full.error) return <Notice tone="danger">{full.error}</Notice>;
    return full.loading ? null : <WorkLogRow icon={<ClipboardCheckIcon />} label="Proposed a plan" />;
  }
  const markdown = fromArguments ?? loaded.markdown;
  // The stored result holds the status at creation (always active), so with a live lookup only its
  // answer may offer Implement: a plan it has not confirmed may have been superseded.
  const live = planStatus?.(meta.id);
  const status = live ?? meta.plan_status ?? "active";
  const implementable = planStatus ? live === "active" : status === "active";
  const copy = () => {
    if (!markdown) return;
    navigator.clipboard.writeText(markdown).then(
      () => notify("Plan copied.", "success"),
      () => notify("The plan could not be copied.", "danger"),
    );
  };
  return (
    <>
      <PlanCard
        markdown={markdown ?? `# ${parsed.title}`}
        status={status}
        onOpenLink={openLink}
        headerActions={
          <>
            <IconButton size="sm" label="Copy plan" icon={<CopyIcon />} disabled={!markdown} onClick={copy} />
            <IconButton
              size="sm"
              label="Download plan"
              icon={<DownloadIcon />}
              disabled={!markdown}
              onClick={() => markdown && downloadText(markdown, artifactFileName(meta))}
            />
          </>
        }
        actions={
          planActions && implementable ? (
            <>
              <Button
                variant="primary"
                disabled={planActions.disabled || !markdown}
                onClick={() => markdown && planActions.implement(markdown)}
              >
                Implement
              </Button>
              <Button
                variant="outline"
                disabled={!markdown}
                onClick={() => markdown && planActions.implementInNewChat(markdown)}
              >
                Implement in a new chat
              </Button>
            </>
          ) : undefined
        }
      />
      {loaded.error && <Notice tone="danger">{loaded.error}</Notice>}
    </>
  );
}
