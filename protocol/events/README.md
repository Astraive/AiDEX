# Event contracts (envelope version 2)

One event stream powers the CLI, desktop, mobile, session history, and diagnostics
(spec §19). The Rust types in `crates/events` (`aidex-events`) are canonical; this
document describes their JSON form. Events are served by the daemon as
`events.replay`, `events.latest`, and the `events.subscribe` stream
(`protocol/commands/README.md`).

## Envelope

```json
{"version": 2, "cursor": 17,
 "scope": {"workspace_id": "ws_01J...", "session_id": "ses_01J..."},
 "occurred_at": "2026-10-10T07:51:01.042Z",
 "event": {"type": "tool.requested", "data": {"call_id": "call_1", "tool_id": "fs.read", "summary": "read src/main.rs", "risk": "low"}}}
```

- `version` is checked before anything else; any other value is rejected
  (`UnsupportedVersion`). Version 1 was PR #40's per-workspace `Event` record.
- `cursor` is the drive's single, strictly increasing position: durable events get
  consecutive cursors from 1, and a failed append assigns none. Acknowledge the
  last durable event you processed or an ordered consumed watermark, then resume
  strictly after it with the same filter.
- `scope` names the workspace and/or session; both are omitted for drive-wide
  events. A session scope always names its workspace.
- `occurred_at` is wall-clock time (RFC 3339 UTC). Order by `cursor`, not time.
- `event` is `{"type": "<dotted.name>", "data": {...}}`. The catalog is closed:
  unknown types and unknown `data` fields are rejected. `null` marks an absent
  optional field. Identifiers are opaque `EntityId`s (`[A-Za-z0-9._:-]`, at most
  128 bytes); `text` fields are free text, redacted and truncated on append.

**Ephemeral** events (`agent.delta`, `process.output`) are live-only: never stored
or replayed, they take no cursor and carry the latest durable cursor instead (they
follow that event). Never acknowledge or deduplicate by an ephemeral cursor.

## Filters

`{"workspace_id"?, "session_id"?, "kinds"?: ["prefix", ...]}`. Scope ids match
exactly (drive-wide events do not match a workspace filter). Kind prefixes match
whole dotted segments: `tool` matches `tool.requested`, `agent.subagent` matches
`agent.subagent.created`, an exact name matches itself. A prefix that matches no
catalog name is rejected. At most 32 prefixes.

Filtered subscriptions also send `events.watermark` notifications with
`data: {"cursor": N}`. Every durable event through `N` was either sent in a
preceding `events.event` notification or excluded by this subscription's filter.
Process all preceding events before acknowledging the watermark. It never
overtakes a queued matching event, including during history paging or overflow
catch-up; multiple filtered-out events may share one coalesced watermark. This
keeps a current filtered consumer's reconnect cursor current even when retention
removes old nonmatches. Watermarks are not envelopes, do not print as events, and
are only sent on durable progress, not as periodic heartbeats. Neither an
ephemeral envelope's cursor nor the subscribe result's `latest_cursor` alone
acknowledges durable progress.

## Retention and cursors

The daemon keeps the newest 100 000 events and nothing older than 90 days, and only
ever removes the oldest events. Resuming after a removed cursor is an explicit
`cursor_expired` error naming `oldest_available` (resume after `oldest_available - 1`
to acknowledge the gap); a cursor the log never assigned is `cursor_ahead`.

## Redaction

Every text field is passed through `aidex_events::redact_text` before it is stored
or delivered: provider tokens (`sk-…`, `sk-ant-…`, `ghp_…`, `github_pat_…`,
`glpat-…`, `AKIA…`, `xox?-…`, `AIza…`, `hf_…`), JWTs, `Bearer …` and
`Authorization: <scheme> …` credentials, PEM private-key blocks, URL passwords, and
values assigned to password/token/secret/key-like names become `[REDACTED]`. Text
longer than 16 KiB is cut with a `[TRUNCATED: N bytes omitted]` marker, and an
encoded event is at most 256 KiB. An identifier that looks like a credential
refuses the event. Secrets are referenced by handle and name only, never by value.

## Catalog

`id` = `EntityId`, `text` = redacted free text, `?` = may be `null`. All events are
durable unless marked *ephemeral*.

| Event | Data fields |
| --- | --- |
| `session.created` | `agent_id: id?`, `title: text?` |
| `session.started` / `paused` / `resumed` / `completed` / `cancelled` | — |
| `session.failed` | `reason: text` |
| `session.interrupted` | `reason: text`, `recoverable: bool` |
| `session.checkpointed` | `checkpoint_id: id`, `label: text?` |
| `task.created` | `task_id: id`, `parent_task_id: id?`, `role: text` |
| `task.started` / `completed` / `cancelled` | `task_id: id` |
| `task.failed` | `task_id: id`, `reason: text` |
| `task.verification` | `task_id: id`, `passed: bool`, `summary: text` |
| `agent.started` | `agent_id: id`, `model: id?` |
| `agent.message` | `role: user\|assistant\|system\|tool`, `content: text` |
| `agent.delta` (*ephemeral*) | `text: text` |
| `agent.subagent.created` | `child_session_id: id`, `role: text` |
| `agent.error` | `code: text`, `message: text` |
| `agent.retry` | `attempt: u32`, `reason: text` |
| `agent.finished` | `agent_id: id`, `outcome: succeeded\|failed\|cancelled` |
| `tool.requested` | `call_id: id`, `tool_id: id`, `summary: text`, `risk: low\|medium\|high\|critical` |
| `tool.started` | `call_id: id` |
| `tool.completed` | `call_id: id`, `duration_ms: u64`, `summary: text` |
| `tool.failed` | `call_id: id`, `code: text`, `message: text` |
| `tool.denied` | `call_id: id`, `reason: text` |
| `approval.created` | `approval_id: id`, `intent_id: id`, `summary: text`, `scope: once\|session\|workspace\|always`, `expires_at: timestamp?` |
| `approval.approved` | `approval_id: id`, `scope` |
| `approval.denied` / `revoked` | `approval_id: id`, `reason: text?` |
| `approval.expired` / `cancelled` | `approval_id: id` |
| `policy.decision` | `intent_id: id`, `decision: allow\|deny\|sandbox\|require_approval\|rate_limit`, `rule: text` |
| `policy.changed` | `profile: id?`, `summary: text` |
| `process.started` | `process_id: id`, `command_summary: text`, `sandbox_level: direct\|brokered\|restricted\|ephemeral` |
| `process.output` (*ephemeral*) | `process_id: id`, `stream: stdout\|stderr`, `chunk: text` |
| `process.exited` | `process_id: id`, `exit_code: i32?`, `signal: i32?` |
| `file.changed` | `path: workspace path`, `change: created\|modified\|deleted\|renamed` |
| `diff.created` | `diff_id: id`, `files: [workspace path]`, `additions: u64`, `deletions: u64` |
| `model.selected` | `provider_id: id`, `model_id: id`, `reason: text` |
| `model.loaded` / `unloaded` | `provider_id: id`, `model_id: id` |
| `model.fallback` | `from: id`, `to: id`, `reason: text` |
| `model.error` | `provider_id: id`, `model_id: id`, `code: text`, `message: text` |
| `workspace.added` | `workspace_id: id`, `kind: host\|drive\|mirror\|ephemeral`, `name: text` |
| `workspace.removed` / `selected` / `ephemeral_discarded` | `workspace_id: id` |
| `workspace.ephemeral_created` | `workspace_id: id`, `source_workspace_id: id` |
| `workspace.ephemeral_applied` | `workspace_id: id`, `files_changed: u64` |
| `vault.unlocked` / `locked` | `vault_id: id` |
| `secret.granted` | `handle_id: id`, `secret_name: text`, `consumer: id` (never the value) |
| `secret.revoked` | `handle_id: id`, `reason: text?` |
| `sandbox.negotiated` | `level: sandbox level`, `enforced: bool` |
| `sandbox.violation` | `process_id: id?`, `detail: text` |
| `cleanup.started` | `cleanup_id: id` |
| `cleanup.completed` | `cleanup_id: id`, `cleaned: u64`, `failed: u64`, `unmanaged: u64` |
| `drive.safe_to_remove` | — |
| `drive.removed` | `unexpected: bool` |
| `remote.paired` | `device_id: id`, `name: text` |
| `remote.revoked` / `connected` | `device_id: id` |
| `remote.disconnected` | `device_id: id`, `reason: text?` |
| `daemon.started` | `pid: u32`, `version: text` |
| `daemon.stopping` | `reason: requested\|drive_removed` |
| `emergency.stopped` | `reason: text`, `cancelled_approvals: u64`, `revoked_approvals: u64`, `terminated_processes: u64`, `revoked_handles: u64` |
| `emergency.resumed` | — |
| `extension.enabled` / `disabled` | `extension_id: id`, `kind: skill\|mcp\|plugin\|agent` |
| `update.staged` | `update_id: id`, `version: text`, `target: text` |
| `update.verified` | `update_id: id`, `version: text`, `signer_key_id: text?` (`null` only for an unsigned bundle accepted by the development opt-in), `unsigned_allowed: bool` |
| `update.switched` | `update_id: id`, `operation: apply\|rollback`, `from_version: text?`, `to_version: text` (takes effect on next launch) |
| `update.rolled_back` | `update_id: id`, `from_version: text?`, `to_version: text?`, `reason: text` (explicit rollback, or recovery of an interrupted operation) |
| `update.failed` | `update_id: id?`, `stage: text`, `code: text` (stable reason, e.g. `bad_signature`), `message: text` |
| `legacy.v1` | `record` (a PR #40 kind, e.g. `tool_finished`), `subject_id: id?`, `outcome?`, `decision: approved\|denied\|expired?`, `exit_code: i32?` — imported history only |

Domains are the first name segment (`agent` for `agent.subagent.created`). New
events are added as new names; a name's fields are never repurposed.
