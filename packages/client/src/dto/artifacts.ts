// Mirrors the `artifact.*` methods in crates/aidexd/src/methods/artifacts.rs and the
// `aidex_native_sessions` artifact types (crates/native-sessions/src/artifacts/model.rs),
// plus the results of the native `artifact_create`, `artifact_update`, and
// `plan_propose` tools (crates/harness/src/native_tools/exec.rs). Artifacts are
// encrypted at rest and unavailable while the vault is locked.

export type ArtifactKind =
  | "document"
  | "code"
  | "data"
  | "diagram"
  | "plan"
  | "web_page"
  | "other";

/** Lifecycle of a `plan` artifact; a session has at most one `active` plan. */
export type PlanStatus = "active" | "superseded" | "completed";

/** How `artifact.read` returns content: `utf8` text or `base64` bytes. */
export type ArtifactEncoding = "utf8" | "base64";

/** Result of `artifact.get` (and the entries of `artifact.list`). */
export type ArtifactMeta = {
  /** `art_<ULID>`. */
  id: string;
  session_id: string;
  run_id: string | null;
  name: string;
  kind: ArtifactKind;
  mime: string;
  language: string | null;
  /** Plaintext size in bytes. */
  size: number;
  /** SHA-256 of the plaintext (lowercase hex). */
  sha256: string;
  encoding: ArtifactEncoding;
  /** Set exactly for `plan` artifacts. */
  plan_status: PlanStatus | null;
  /** Unix milliseconds. */
  created_at: number;
  updated_at: number;
};

/** Params of `artifact.list` (`limit` 1-100, default 100). */
export type ArtifactListParams = {
  session_id: string;
  offset?: number;
  limit?: number;
};

/** Result of `artifact.list`, newest first. */
export type ArtifactList = {
  artifacts: ArtifactMeta[];
  total_artifacts: number;
  offset: number;
  next_offset: number | null;
  truncated: boolean;
};

/** Params of `artifact.read`: a page of at most 64 KiB (larger limits are clamped). */
export type ArtifactReadParams = { id: string; offset?: number; limit?: number };

/** Result of `artifact.read`. UTF-8 pages never split a character. */
export type ArtifactContent = {
  id: string;
  encoding: ArtifactEncoding;
  content: string;
  offset: number;
  /** Null on the last page. */
  next_offset: number | null;
  total_bytes: number;
  sha256: string;
};

/** Params of `artifact.plan.update` (conflict for non-plan artifacts). */
export type ArtifactPlanUpdateParams = { id: string; status: PlanStatus };

/** Content (JSON) of an `artifact_create` or `artifact_update` tool result message. */
export type ArtifactToolResult = { artifact: ArtifactMeta };

/** Content (JSON) of a `plan_propose` tool result message. The plan is the session's new active plan. */
export type PlanToolResult = { title: string; artifact: ArtifactMeta };

/** Largest page `artifact.read` returns (`ARTIFACT_PAGE_BYTES`). */
export const ARTIFACT_PAGE_BYTES = 64 * 1024;
