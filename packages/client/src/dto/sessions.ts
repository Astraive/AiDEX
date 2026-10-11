/** Bounded public projections from aidexd::methods::native_view. */
export type ConversationMode = "chat" | "plan" | "agent";
export type PermissionMode =
  "supervised" | "auto_accept_edits" | "auto" | "full_access";
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
};
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
  skill_ids?: string[];
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
  has_runtime_ref: boolean;
  transcript_omitted: boolean;
  pending_call_count: number;
};
export type Approval = {
  id: string;
  run_id: string;
  tool_name: string;
  arguments: unknown;
  arguments_bytes: number;
  arguments_truncated: boolean;
  fingerprint: string;
  status: "pending" | "approved" | "denied" | "expired";
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
