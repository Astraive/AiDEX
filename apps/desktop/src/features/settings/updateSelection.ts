// Pure checks behind the Updates panel: which manual sources it sends to `update.check`, when a checked
// package may be applied, whether a re-check still describes the package the user reviewed, when the
// panel re-reads the update status, and its confirmation and error state.
import type { BundleTarget, CheckReport, JournalOperation, JournalSummary } from "@aidex/client/dto/updates";

/**
 * Whether `source` is an absolute local path on a host of `target`: a drive-letter path on Windows, a
 * `/` path elsewhere. URLs, relative paths, and control characters are refused, and so are Windows
 * network shares and device paths (`\\server\share`, `\\?\`, `\\.\`): a package on a share is copied to
 * this computer, or the share mapped to a drive letter, first. There is no online release catalog.
 */
export function absoluteUpdateSource(source: string, target: BundleTarget): boolean {
  if (!source || /[\x00-\x1f]/.test(source) || /^[a-z][a-z0-9+.-]*:\/\//i.test(source)) return false;
  return target.startsWith("windows-") ? /^[A-Za-z]:[\\/]/.test(source) : source.startsWith("/");
}

/** Why a checked package cannot be applied, or `undefined` when it can. */
export function applySelectionReason(
  check: CheckReport | undefined,
  target: BundleTarget | undefined,
  pending: boolean,
): string | undefined {
  if (pending) return "An update is unfinished. AiDEX Core must recover it before another package is applied.";
  if (!check || !target) return "Check a signed package first.";
  if (check.candidate.signature.status !== "verified") return "A trusted publisher signature is required.";
  if (check.candidate.target !== target) return "This package is built for a different computer.";
  if (check.action === "up_to_date") return "This signed package is already the selected version.";
  return undefined;
}

/** Whether a re-check describes the same signed package (path, target, version, manifest, and signer). */
export function sameCandidate(before: CheckReport, after: CheckReport): boolean {
  return (
    before.package === after.package &&
    before.candidate.target === after.candidate.target &&
    before.candidate.version === after.candidate.version &&
    before.candidate.manifest_sha256 === after.candidate.manifest_sha256 &&
    before.candidate.signature.status === "verified" &&
    after.candidate.signature.status === "verified" &&
    before.candidate.signature.key_id === after.candidate.signature.key_id
  );
}

/** While an operation runs or an update is unfinished, the journal is re-read this often... */
export const PROGRESS_POLL_MS = 2000;
/** ...and this often once it has not changed for `UNCHANGED_READS_BEFORE_SLOWING` reads. */
export const SLOW_PROGRESS_POLL_MS = 10_000;
export const UNCHANGED_READS_BEFORE_SLOWING = 3;

/**
 * How often the Updates panel re-reads `update.status`, or `undefined` for not at all. Every read
 * hashes the selected bundle's critical files, so it polls only while the journal can still move: an
 * operation of this page is running (`busy`), or an update is unfinished (`pending`) and AiDEX Core is
 * not stuck on it (`recoveryError`: startup recovery failed, and nothing changes until a restart).
 * A journal that stopped changing (`unchangedReads`) is read less often; Verify again stays a manual
 * re-read.
 */
export function progressPollInterval(input: {
  busy: boolean;
  pending: boolean;
  recoveryError: boolean;
  unchangedReads: number;
}): number | undefined {
  if (!input.busy && (!input.pending || input.recoveryError)) return undefined;
  return input.unchangedReads >= UNCHANGED_READS_BEFORE_SLOWING ? SLOW_PROGRESS_POLL_MS : PROGRESS_POLL_MS;
}

/** What identifies the journal's progress: its operation, step, and last write. */
export function journalProgressKey(journal: JournalSummary | null | undefined): string {
  return journal ? `${journal.id}|${journal.state}|${journal.updated_at}` : "none";
}

/** Counts consecutive reads that found the journal at the same step. */
export function countUnchangedReads(
  previous: { key: string; count: number } | undefined,
  key: string,
): { key: string; count: number } {
  return previous?.key === key ? { key, count: previous.count + 1 } : { key, count: 0 };
}

export type Review = "apply" | "rollback";

/** The open confirmation and the error shown with it (or on the page while none is open). */
export type ReviewState = { review?: Review; error?: string };

export type ReviewAction =
  | { type: "open"; review: Review }
  | { type: "close" }
  | { type: "error"; error: string | undefined }
  | { type: "reset" };

/**
 * Confirmation and error state of the Updates panel. Opening a confirmation clears the error: one from
 * an earlier operation (a refused check, a changed package) belongs to that operation, not to the new
 * dialog. An error raised while the dialog is open stays in it; closing keeps it for the page.
 */
export function reviewReducer(state: ReviewState, action: ReviewAction): ReviewState {
  switch (action.type) {
    case "open":
      return { review: action.review, error: undefined };
    case "close":
      return { review: undefined, error: state.error };
    case "error":
      return { ...state, error: action.error };
    case "reset":
      return {};
  }
}

/**
 * The outcome of an apply or rollback this page stopped waiting for (its call timed out or lost the
 * connection while AiDEX Core kept working), read from the journal once it is no longer pending.
 * Only a journal of that operation started at or after `since` (milliseconds) describes it.
 */
export function followedOutcome(
  operation: JournalOperation,
  since: number,
  journal: JournalSummary | null | undefined,
): { tone: "success" | "danger"; text: string } {
  const action = operation === "apply" ? "applying the package" : "rolling back";
  const started = journal ? Date.parse(journal.started_at) : Number.NaN;
  if (!journal || journal.operation !== operation || !(started >= since))
    return {
      tone: "danger",
      text: `AiDEX Core has no record of ${action} after this page stopped waiting. Check the version above, then try again.`,
    };
  if (journal.state === "committed")
    return {
      tone: "success",
      text: `AiDEX Core finished ${action}${journal.to ? `: ${journal.to.version} is selected for the next launch` : ""}.`,
    };
  return {
    tone: "danger",
    text: `AiDEX Core did not finish ${action}${journal.outcome ? ` (${journal.outcome.replace(/_/g, " ")})` : ""}. The selected version is unchanged.`,
  };
}
