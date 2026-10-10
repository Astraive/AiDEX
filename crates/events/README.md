# `aidex-events`

The AiDEX event contract: a closed, typed catalog of product events, a versioned
envelope, a durable SQLite log with one global cursor per drive, filtered replay,
bounded push subscriptions, the `EventSink` emit interface for domain crates, and
the credential redactor every text field goes through. The wire catalog is
documented in [`protocol/events/README.md`](../../protocol/events/README.md).

## API

```rust
use aidex_events::*;

// Open (or create, or migrate a PR #40 database in place). The directory must exist.
let log = EventLog::open(dir.join("events.sqlite3"), RetentionPolicy::default())?;

// Emit. Domain crates take `&dyn EventSink` / `Arc<dyn EventSink>`, never the log itself.
let cursor: Cursor = log.emit(
    EventScope::session(workspace_id, session_id),
    EventKind::ToolRequested { call_id, tool_id, summary: "read src/main.rs".into(), risk: RiskLevel::Low },
)?;
let envelope: EventEnvelope = log.append(EventScope::global(), EventKind::DriveSafeToRemove {})?;

// Replay strictly after an acknowledged cursor (1..=256 per page).
let page: ReplayPage = log.replay(&EventFilter::for_workspace(ws).with_kind("tool"), Cursor::ZERO, 256)?;
// page.events, page.next_cursor (resume after this), page.has_more

// Subscribe: history first (paged from disk), then live events, in cursor order.
let mut subscription = log.subscribe(EventFilter::all(), last_acknowledged)?;
while let Some(envelope) = subscription.next_timeout(Duration::from_secs(1))? { /* ... */ }

log.latest_cursor()?; log.oldest_cursor()?; log.prune()?;
log.import_legacy(&old_pr40_file)?; // read-only, once, only into an empty log

// Tests in other crates
let sink = MemoryEventSink::new(); /* ... */ sink.names(); sink.events(); sink.take();
let discard = NullEventSink;

// Redaction (also for logs)
let clean: Cow<str> = redact_text("Authorization: Bearer abc.def.ghi"); // "Authorization: Bearer [REDACTED]"
```

Key types: `EventKind` (catalog; `name()`, `domain()`, `is_durable()`), `EventEnvelope
{ version, cursor, scope, occurred_at, event }` (`decode()` rejects unknown versions
first), `Cursor(u64)` (`Cursor::ZERO`), `EventScope { workspace_id, session_id }`
(`global()`, `workspace(..)`, `session(ws, ses)`), `EventFilter { workspace_id,
session_id, kinds }`, `RetentionPolicy { max_events, max_age }`, `ReplayPage`,
`Subscription`, `EventSink`, `EventError` (with `code() -> ServiceErrorCode`),
`LegacyImport`. Small payload enums: `Outcome`, `MessageRole`, `RiskLevel`,
`ApprovalScope`, `Verdict`, `ProcessStream`, `FileChange`, `SandboxLevel`,
`WorkspaceKind`, `DaemonStopReason`, `ExtensionKind`, `LegacyRecord`,
`LegacyApprovalDecision`.

## Guarantees

- **One strictly increasing cursor.** Durable appends run in an immediate SQLite
  transaction (WAL, `synchronous=FULL`) under one lock; they get consecutive
  cursors starting at 1, are committed before `append` returns, and a failed append
  assigns nothing. Cursors never restart, even after retention removed everything.
- **No silent gaps.** Retention removes only a prefix of the log. Replaying or
  subscribing after a cursor whose successors were removed fails with
  `CursorExpired { requested, oldest_available }`; a cursor the log never assigned
  fails with `CursorAhead`.
- **Replay exactness.** `replay(filter, after, limit)` returns matching events with
  cursor `> after` in cursor order. `next_cursor` is where to resume; without
  `has_more` it is the latest cursor, so filtered readers skip ahead.
- **Subscriptions.** A subscription is registered at a live-from cursor under the
  log lock: durable history up to it is paged from disk, everything after it is
  offered to a bounded live queue (default 1024 events, at most 4 MiB). Delivery is
  in cursor order without duplicates or gaps. Publishers never block: a full queue
  marks the subscriber lagged and drops its queue; the subscriber then catches up
  from disk after the last cursor it delivered and rejoins. Dropping a subscription
  unregisters it.
- **Ephemeral events** (`agent.delta`, `process.output`) are never stored and take
  no cursor; their envelope carries the latest durable cursor. They reach live
  subscribers only, and are lost while a subscriber lags.
- **Redaction and bounds.** Before storage or delivery every string in the payload
  goes through `redact_text` and is truncated to 16 KiB with a
  `[TRUNCATED: N bytes omitted]` marker. An identifier (payload or scope) that looks
  like a credential refuses the event (`SensitiveIdentifier`), since rewriting it
  would change what it names. Encoded events are limited to 256 KiB. `Debug` of an
  `EventKind` shows only redacted data.
- **Closed, versioned wire format.** Unknown event types, unknown payload fields,
  and unknown envelope versions are rejected. Envelope version 2 (PR #40's `Event`
  record was version 1).
- **Schema safety.** A `schema_version` table records schema 2; newer or unknown
  schemas and non-event databases are refused untouched, before enabling WAL.
  PR #40's
  `events(workspace_id, sequence, session_id, body)` table is migrated in place in
  one transaction (rows imported in their original write order, then the old table
  is dropped); a corrupt row fails the migration and leaves the database unchanged.
  `import_legacy` imports a separate PR #40 file read-only, once (recorded in
  `event_migrations`), and only into an empty log. Symlinked database paths
  (including `-wal`/`-shm`/`-journal` sidecars) are refused.

## Migration of PR #40 records

Records with a faithful equivalent map onto it: `task_started` → `task.started`,
`task_finished` (succeeded/cancelled) → `task.completed`/`task.cancelled`,
`session_started` → `session.started`, `agent_started` → `agent.started` (no model),
`agent_finished` → `agent.finished`, `approval_resolved` (denied/expired) →
`approval.denied`/`approval.expired`, `process_finished` → `process.exited`,
`cleanup_started` → `cleanup.started`. Everything else becomes `legacy.v1`, which
preserves the original record (kind, subject id, outcome, decision, exit code)
instead of inventing fields v1 never captured. Imported events carry the import
time as `occurred_at` (v1 had no timestamps).

## Limitations

- Live delivery covers appends made through this process's `EventLog` handles
  (the daemon is the only writer); another process appending to the same file is
  seen by replay and catch-up, not live.
- Redaction is pattern based (provider token prefixes, JWTs, bearer/authorization
  credentials, PEM private keys, URL passwords, and values assigned to names such as
  `password`, `token`, `secret`, `key`). It errs toward redacting but cannot
  recognise arbitrary secrets; producers must still keep secret values out of
  events and reference secrets by handle.
- The log is not encrypted at rest (spec §94 asks for encrypted audit history; that
  needs the vault and drive encryption).
- `occurred_at` is wall-clock time; ordering is defined by cursor only.
- The catalog's payload fields are a first contract; domain crates may need
  additional variants, which are added as new names (never by reusing one).
