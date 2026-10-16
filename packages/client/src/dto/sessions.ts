/** Bounded public projections from aidexd::methods::native_view. */
import type { AttachmentRef } from "./attachments";

export type ConversationMode = "chat" | "plan" | "agent";
export type PermissionMode =
  | "supervised"
  | "auto_accept_edits"
  | "auto"
  | "full_access";
export type RunStatus =
  | "pending"
  | "running"
  | "waiting_approval"
  | "completed"
  | "failed"
  | "cancelled"
  | "interrupted";
export type ToolCall = {
  id: string;
  name: string;
  arguments: string;
  arguments_bytes: number;
  arguments_truncated: boolean;
  provider_metadata_omitted: boolean;
};
export type Message = {
  original_index: number;
  role: "system" | "user" | "assistant" | "tool";
  content: string;
  content_bytes: number;
  content_truncated: boolean;
  tool_calls: ToolCall[];
  tool_calls_total: number;
  tool_calls_truncated: boolean;
  tool_call_id: string | null;
  /** Skill ids (`<scope>:<name>`) invoked with this user message; absent otherwise. */
  skills?: string[];
  /**
   * Attachments sent with this user message, in order; absent otherwise. A message sent with
   * attachments and no text stores `ATTACHMENT_ONLY_CONTENT` ("[Attached files]") as its
   * content: hide exactly that text when `attachments` is non-empty.
   */
  attachments?: AttachmentRef[];
};
/**
 * Params of `message.send` and `agent.start`. `content` is stored exactly as typed
 * (keep `$name` tokens); `skills` are injected for that run only, and `web_search`
 * turns on web tools for that run in Chat mode (Agent and Plan follow the web
 * settings). Neither option is stored in the session configuration. Unknown or
 * disabled skills fail with `data.reason = "skill_unavailable"`; `web_search` while
 * web search is off fails with `data.reason = "web_unavailable"`. `attachments` are
 * staged attachment ids sent with this message (`content` may then be blank); refusals
 * are `invalid_params` with `data.reason` `attachment_unavailable` (unknown, foreign,
 * repeated, already sent, more than 20, or with `parent_run_id`),
 * `attachments_unsupported` (the model cannot read images or PDFs, or the conversation
 * runs on an external runtime), `context_too_large_for_runtime`, or `too_large`. Nothing is
 * stored when a send is refused; a retry with the same `command_id` and arguments returns
 * the original run.
 */
export type SendParams = {
  session_id: string;
  content: string;
  command_id: string;
  parent_run_id?: string;
  /** At most 8 skill ids. */
  skills?: string[];
  web_search?: boolean;
  /** At most 20 staged attachment ids, in order. */
  attachments?: string[];
};
/**
 * Model-facing names of the native tools the desktop renders specially (the
 * dotted ids `web.search`, `web.fetch`, `skill.load`, `artifact.create`,
 * `artifact.update`, `plan.propose` with dots as underscores). MCP tools are
 * `mcp__<server>__<tool>`. Match tool results to calls by `tool_call_id`.
 */
export const NATIVE_TOOL_NAMES = {
  webSearch: "web_search",
  webFetch: "web_fetch",
  skillLoad: "skill_load",
  artifactCreate: "artifact_create",
  artifactUpdate: "artifact_update",
  planPropose: "plan_propose",
} as const;
/** Prefix of MCP tool names: `mcp__<server>__<tool>`. */
export const MCP_TOOL_PREFIX = "mcp__";
export type SessionConfig = {
  title: string;
  workspace_id: string | null;
  mode: ConversationMode;
  provider_id: string;
  model_id: string;
  runtime_id?: string | null;
  permission_mode: PermissionMode;
  max_steps?: number;
  max_tokens?: number;
  instructions?: string;
  fallback_model?: string | null;
};
export type Run = {
  id: string;
  session_id: string;
  parent_run_id: string | null;
  status: RunStatus;
  steps: number;
  error: string | null;
  error_truncated: boolean;
  created_at: number;
  updated_at: number;
  effective_max_tokens: number | null;
  estimated_input_tokens: number | null;
  model_used: string | null;
  /**
   * Whether the run saved an external runtime reference `run.resume` can continue (bound to a
   * native branch and snapshot); never the reference itself.
   */
  has_runtime_ref: boolean;
  transcript_omitted: boolean;
  pending_call_count: number;
  /** Index of the run's user message in the conversation history (root runs). */
  message_index?: number;
  /**
   * Conversation mode, permission mode and project when the run began. Absent for runs
   * recorded before these fields existed; show no mode divider next to such runs.
   */
  mode?: ConversationMode;
  permission_mode?: PermissionMode;
  workspace_id?: string;
  /** Inference attempts repeated after a retryable provider failure (each used a step). */
  inference_retries?: number;
  /**
   * The largest number of older messages one request of this run left out to fit the model's
   * context window; the stored history is unchanged.
   */
  context_trimmed_messages?: number;
};
export type ApprovalStatus = "pending" | "approved" | "denied" | "expired";
export type Approval = {
  id: string;
  run_id: string;
  tool_name: string;
  arguments: unknown;
  arguments_bytes: number;
  arguments_truncated: boolean;
  fingerprint: string;
  status: ApprovalStatus;
  preview: unknown;
  preview_truncated: boolean;
  runtime_id: string | null;
  runtime_request_id: string | null;
  runtime_ref_omitted: boolean;
};
export type Checkpoint = {
  id: string;
  run_id: string;
  path: string;
  before_present: boolean;
  before_bytes: number | null;
  after_digest: string;
  content_omitted: boolean;
};
export type SessionDetail = SessionConfig & {
  id: string;
  pinned: boolean;
  created_at: number;
  messages: Message[];
  messages_offset: number;
  total_messages: number;
  messages_truncated: boolean;
  next_message_offset: number | null;
  runs: Run[];
  total_runs: number;
  runs_truncated: boolean;
  approvals: Approval[];
  total_approvals: number;
  approvals_truncated: boolean;
  checkpoints: Checkpoint[];
  total_checkpoints: number;
  checkpoints_truncated: boolean;
  /** The conversation saved an external runtime reference, which no view returns. */
  runtime_ref_omitted: boolean;
  /** Always `true`: command ids (`message.send` idempotency keys) are never returned. */
  commands_omitted: boolean;
  /** Set on a conversation created by `conversation.fork`: its first `message_count` messages
   * are a copy of `session_id`'s history. */
  forked_from?: ForkOrigin;
};
export type ForkOrigin = { session_id: string; message_count: number };
/**
 * Params of `conversation.fork`: a new conversation with `configuration` (validated like
 * `conversation.create`; `title` replaces its title) whose history and sent attachments are
 * copies of `source_id`'s. Runs, approvals, checkpoints, artifacts and staged attachments are
 * not copied. Refused (`conflict`) while the source has an active run; `command_id` makes a
 * retry return the conversation the first call created.
 */
export type ConversationForkParams = {
  source_id: string;
  configuration: SessionConfig;
  title?: string;
  /** 1-128 bytes. */
  command_id?: string;
};
export type SessionSummary = {
  id: string;
  status: string;
  locked: boolean;
  title?: string;
  pinned?: boolean;
  workspace_id?: string | null;
  mode?: ConversationMode;
  provider_id?: string;
  model_id?: string;
  created_at?: number;
};
export type ContentChunk = {
  content: string;
  exists: boolean;
  offset: number;
  next_offset: number | null;
  total_bytes: number;
  truncated: boolean;
  digest: string | null;
};
export type HistoryPage = {
  messages: Message[];
  offset: number;
  total_messages: number;
  next_offset: number | null;
  truncated: boolean;
};
export type HistoryScope = { session_id?: string; run_id?: string };
/** Offset paging (`conversation.list`, `approval.list`, `checkpoint.list`); `limit` is 1-100. */
export type PageParams = { offset?: number; limit?: number };
/**
 * Paging facts of a list result: `next_offset` is the next page's offset, `null` on the last page.
 * When the wire budget cuts items from a page, `truncated` and an `<items>_truncated` flag are set.
 */
export type PageInfo = { offset: number; next_offset: number | null; truncated: boolean };
/** Result of `conversation.list`: from `offset` 0, at most 100 per page by default. */
export type ConversationList = PageInfo & {
  sessions: SessionSummary[];
  total_sessions: number;
  sessions_truncated?: true;
};
/** Result of `checkpoint.list`: in the order they were taken, from `offset` 0 by default. */
export type CheckpointList = PageInfo & {
  checkpoints: Checkpoint[];
  total_checkpoints: number;
  checkpoints_truncated?: true;
};
/** Params of `approval.list`: one run's approvals or all of them, optionally of one status. */
export type ApprovalListParams = PageParams & { run_id?: string; status?: ApprovalStatus };
/**
 * Result of `approval.list`: approvals oldest first by creation time, across conversations (ties
 * keep conversation and request order). Without `offset` the last page, holding the newest
 * approvals, is returned, and `truncated` is also true when items exist before `offset`.
 */
export type ApprovalList = PageInfo & {
  approvals: Approval[];
  total_approvals: number;
  approvals_truncated?: true;
};
