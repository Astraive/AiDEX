# aidex-tools

Typed, workspace-scoped tools with a two-phase **plan → execute** contract (issue #16; spec §12.2-12.4, §13, §56, §78-79, §91). Every tool validates its input against a closed schema, resolves every path through `aidex_workspace::ConfinedRoot`, reports facts and risk for policy evaluation, previews writes as reviewable diffs, and executes only with a permit bound to the exact plan. Processes (git, shell) run only through a `ProcessLauncher` that the execution broker implements over `aidex-sandbox`.

Depends on `aidex-core`, `aidex-events` (`EventSink`, `redact_text`), `aidex-policy` (`Action`, `ShellCommand`/`classify_shell`, `Intent`), `aidex-sandbox` (launch/exit/output types), and `aidex-workspace` (`ConfinedRoot`). The execution broker (`aidex-broker`) exposes tools through the daemon (`tool.*`); the native harness (`aidex-harness`) currently holds its own registry.

## Flow

```rust
let registry = ToolRegistry::with_builtin_tools();               // held by the broker only
let ctx = ToolContext::new(session_id, workspace_id, call_id, Arc::new(root), events)
    .with_launcher(broker_launcher);                              // Arc<dyn ProcessLauncher>
let plan = registry.plan(&ctx, &ToolId::new("fs.write")?, input)?; // validate, resolve, preview; no side effects
for intent in plan.intents() { /* aidex_policy::evaluate(...), approvals */ }
let permit = ExecutionPermit::for_plan(&plan, PermitIssuer::Approval { approval_id });
let output = registry.execute(&ctx, &plan, permit)?;             // re-verify, lock, apply, emit events
```

## API

```rust
pub trait Tool: Send + Sync {
    fn descriptor(&self) -> &ToolDescriptor;
    fn plan(&self, ctx: &ToolContext, input: serde_json::Value) -> Result<ToolPlan, ToolError>;
    fn execute(&self, ctx: &ToolContext, plan: &ToolPlan, permit: &ExecutionPermit) -> Result<ToolOutput, ToolError>;
    fn write_locks(&self, plan: &ToolPlan) -> Vec<String>; // default: plan.facts().write_paths()
}

impl ToolRegistry {
    pub fn new() -> Self;
    pub fn with_builtin_tools() -> Self;
    pub fn register(&mut self, tool: Box<dyn Tool>) -> Result<(), ToolError>;
    pub fn descriptor(&self, id: &ToolId) -> Option<&ToolDescriptor>;
    pub fn descriptors(&self) -> Vec<&ToolDescriptor>;           // ordered by id
    pub fn specs(&self) -> Vec<ToolSpec>;                         // model-facing {name, description, parameters}
    pub fn resolve_model_name(&self, name: &str) -> Option<&ToolId>; // "fs_read" -> fs.read
    pub fn plan(&self, ctx: &ToolContext, tool: &ToolId, input: Value) -> Result<ToolPlan, ToolError>;
    pub fn execute(&self, ctx: &ToolContext, plan: &ToolPlan, permit: ExecutionPermit) -> Result<ToolOutput, ToolError>;
}

pub trait ProcessLauncher: Send + Sync {
    fn launch(&self, request: &LaunchRequest, cancel: &CancelToken) -> Result<LaunchOutcome, LaunchError>;
}
```

| Type | Purpose |
| --- | --- |
| `ToolId` | Dotted lowercase id (`fs.read`), also a valid `EntityId`; `model_name()` maps `.` to `_`. |
| `ToolDescriptor` | `id`, `version`, `description`, `input_schema`/`output_schema` (draft-7 JSON Schema generated with `schemars` from closed `deny_unknown_fields` structs), `risk: RiskProfile`, `capabilities`. `spec()` gives the `ToolSpec`. |
| `RiskProfile` | `action: aidex_policy::Action`, `base_risk: RiskLevel`, `reads_workspace`, `writes_workspace`, `spawns_process`, `network`, `destructive`, `remote_write`; `is_read_only()`. |
| `ToolContext` | `session_id`, `workspace_id`, `call_id`, `root: Arc<ConfinedRoot>`, `events: Arc<dyn EventSink>`, `cancel: CancelToken` (re-exported `aidex_core::CancelToken`, shared with providers and the harness; cancellation or an expired deadline stops the call), `limits: ToolLimits`, `processes: Option<Arc<dyn ProcessLauncher>>`; `new`, `with_limits`, `with_cancel`, `with_launcher`, `for_call`, `scope`. |
| `ToolLimits` | `max_read_bytes` (512 KiB), `max_write_bytes` (4 MiB), `max_search_results` (200), `max_output_bytes` (256 KiB), `max_search_file_bytes` (2 MiB), `max_walk_entries` (20 000), `max_diff_bytes` (128 KiB), `process_timeout` (120 s). |
| `ToolPlan` | Immutable, single-use: `plan_id`, tool id/version, action, session/workspace/call ids, root, normalized `input`, `facts: PlanFacts`, `risk`, redacted `summary`, `preview: Option<DiffPreview>`, tool `state`, `fingerprint: PlanFingerprint` (SHA-256 over canonical JSON of all of these). `intent()`/`intents()` build `aidex_policy::Intent`s; `verify_integrity()`, `check_context(ctx)`. `ToolPlan::new(ctx, descriptor, PlanParts)` for third-party tools. |
| `PlanFacts` | `paths: Vec<PlanPath { path, display, access: PathAccess, inside_workspace, exists, kind, size, sha256, sensitive }>`, `command: Option<ShellCommand>`, `network: Option<NetworkFacts { targets, unresolved }>`, `destructive`, `remote_write`, `implied_actions`; `intent_attributes()` (sensitive hint → `PathClass::Sensitive`). |
| `ExecutionPermit` | `new(PlanFingerprint, PermitIssuer)` / `for_plan(&plan, issuer)`; `authorize(&plan, &descriptor)`. Issuers: `Policy { rule }`, `Approval { approval_id }`, `ReadOnly` (refused for non-read-only tools), `Development`. Not `Clone`; consumed by `ToolRegistry::execute`. |
| `ToolOutput` | `summary` (redacted, ≤ 512 B), `content` (typed output as JSON, round-trip validated), `diff`, `changes: Vec<PathChange>`, `warnings`. |
| `DiffPreview` / `FileDiff` | Per file: `path`, `kind` (created/modified/deleted), `additions`, `deletions`, `unified` (bounded, cut on line boundaries), `binary`, `truncated`, before/after SHA-256. |
| `LaunchRequest` | Built only from a plan plus its permit (`LaunchRequest::new(ctx, plan, permit, program, args)`): fixed argv, cwd, workspace root and `MountAccess`, fixed non-secret env, stdin, network flag, timeout, output bound, plan fingerprint, issuer, redacted display. |
| `LaunchOutcome` | `process_id`, `sandbox_level`, `status: aidex_sandbox::ExitStatusInfo`, `output: aidex_sandbox::CapturedOutput`. |
| `DirectLauncher` | **Tests and development only** (feature `direct-launcher`): runs processes unsandboxed as the current user. |
| `ToolError` | `code() -> ServiceErrorCode` and stable `reason()`: `invalid_input` (with `field` like `edits[1].old` and `InputError`: `not_an_object`, `unknown_field`, `missing_field`, `invalid_type`, `invalid_value`, `invalid_path`, `invalid_pattern`, `out_of_range`, `conflicting_fields`), `escapes_root`, `symlink_leaf`, `not_found`, `wrong_kind`, `forbidden_path`, `too_large`, `conflict`, `patch_failed`, `plan_mismatch`, `permit_rejected`, `already_executed`, `cancelled`, `launcher_unavailable`, `launch_failed`, `process_failed`, `timed_out`, `not_a_repository`, `events`, `invalid_output`, `io`, plus workspace reasons. |

## Built-in tools

| Id | Action | Base risk | Effects | Input (closed schema) |
| --- | --- | --- | --- | --- |
| `fs.read` | `fs.read` | low | read | `path`, `start_line?`, `max_lines?`, `max_bytes?` → content, size, sha256, `binary`, `lossy_utf8`, `next_line` |
| `fs.list` | `fs.read` | low | read | `path?`, `recursive`, `max_depth?`, `include_ignored`, `max_entries?` |
| `fs.glob` | `fs.search` | low | read | `pattern` (`*` within a component, `**` across), `path?`, `include_ignored`, `case_insensitive`, `max_results?` |
| `fs.search` | `fs.search` | low | read | `pattern`, `regex`, `case_insensitive`, `path?`, `glob?`, `context_lines` (≤ 10), `max_results?`, `include_ignored` |
| `fs.write` | `fs.write` | medium | write | `path`, `content`, `create_only`, `expected_sha256?` |
| `fs.patch` | `fs.write` | medium | write | `path`, exactly one of `edits: [{old, new, expected_occurrences?}]` or `unified_diff`, `expected_sha256?` |
| `fs.delete` | `fs.delete` | medium (high when recursive) | write, destructive | `path`, `recursive` |
| `git.status` | `git.read` | low | read, process | `paths`, `untracked` |
| `git.diff` | `git.read` | low | read, process | `mode` (`worktree`/`staged`/`rev`), `rev?`, `paths`, `context_lines` |
| `git.log` | `git.read` | low | read, process | `rev?`, `max_count?` (≤ 200), `skip`, `paths` |
| `git.show` | `git.read` | low | read, process | `rev`, `paths`, `context_lines` |
| `git.branch` | `git.read` | low | read, process (list only) | `all` |
| `git.commit` | `git.commit` | medium | repository write, process | `message`, `paths` (stages and commits exactly these) |
| `git.push` | `git.push` | high | network, remote write, process | `remote` (name, default `origin`), `branch`, `set_upstream` |
| `shell.exec` | `shell.exec` | medium (classified; `shell: true` ≥ high) | anything, process | `program` + `args`, or `command` with `shell: true`; `cwd?`, `timeout_ms?`, `stdin?` |

## Guarantees

- **Two-phase, single-use execution.** `plan` has no side effects (no writes, no processes). `execute` refuses a permit whose fingerprint differs, a plan whose content no longer hashes to its fingerprint, a plan for another tool version, workspace, session, call, or root, a `ReadOnly` permit for a non-read-only tool, and a plan already executed. Execution uses only the plan's normalized input, so the approved plan is what runs. Process launches require the plan and its permit, and carry the fingerprint so the broker's launcher can refuse launches it did not permit. Built-in tools are never handed out by the registry. The harness never holds a registry; it holds a broker handle.
- **Confinement.** Every path (including git pathspecs and the shell `cwd`) is parsed with `parse_workspace_path` and resolved with `ConfinedRoot` right before use: traversal, absolute/drive/UNC paths, and links or junctions leaving the root are refused; write targets that are links are refused (`symlink_leaf`); `fs.delete` on a link removes only the link. Directory walks never follow links and never enter `.git`. File tools refuse to modify anything inside `.git` (hooks there would later run code); writes beneath `.aidex/` add an implied `policy.change` intent.
- **Reviewable, conflict-checked writes.** `fs.write`/`fs.patch`/`fs.delete` precompute the diff (`similar`, bounded, binary marked) and record the target's SHA-256 (directories: a tree digest). Execute re-reads the target under a per-path write lock and fails with `conflict` if anything changed; patches are re-applied and must reproduce the reviewed result hash. Files are written to an exclusive temporary sibling, fsynced, and renamed (Unix mode bits preserved); parents are created one at a time with confinement re-checked.
- **Concurrency.** Writers whose lock paths overlap (equal, or one contains the other) serialize; the loser of a race sees `conflict`. Reads take no locks. Git mutations serialize on the repository.
- **Bounded reads.** `fs.read` streams: binary files (NUL in the first 8 KiB) return size and SHA-256 only; text returns at most `max_read_bytes`, cut on a UTF-8 boundary, with `next_line` for continuation. `fs.search` skips binary files, files above `max_search_file_bytes`, links, and credential-like names (counted in `skipped`).
- **Git hardening.** Fixed argument vectors (no shell), `--no-pager`, `-c core.fsmonitor=false`, hooks disabled (`core.hooksPath` set to a device), no colour, no external diff or textconv drivers, `--end-of-options` before revisions, `:(literal)` pathspecs, `GIT_TERMINAL_PROMPT=0`, `GIT_OPTIONAL_LOCKS=0` for reads, `GIT_CEILING_DIRECTORIES` so only a repository rooted at the workspace is used, porcelain/NUL-delimited parsing. `git.commit` also passes `--no-verify` and commits only the named paths (message on stdin). `git.push` accepts only configured remote names and plain branch names; force pushes are not supported.
- **Events.** `ToolRegistry::execute` emits `tool.started` (if it cannot be recorded, nothing runs), then `file.changed` per changed path and `diff.created` for applied diffs, then `tool.completed { duration_ms, summary }` or `tool.failed { code: reason(), message }`. Summaries and messages are redacted with `aidex_events::redact_text` and bounded; event failures after the effect are reported in `ToolOutput::warnings`. `tool.requested`/`tool.denied` belong to the broker; `process.*` events to the launcher.
- **Untrusted output.** Tool output (file content, search hits, process output) is data, never instructions. Process stdout/stderr are redacted and bounded.

## Limitations

- Policy evaluation, approvals, `protected` path classes, secret handles, and the sandboxed `ProcessLauncher` live in `aidex-broker`. `DirectLauncher` is unsandboxed and test-only.
- Confinement is a point-in-time check (TOCTOU): a concurrent process could swap a checked directory for a link between resolution and use. OS-enforced confinement belongs to the sandbox. A concurrent external writer can also race the final hash check and rename.
- `fs.read` does not redact file content; credential-like files are governed by policy (sensitive path class), and `fs.search` skips them by name only. The hint is not a secret scanner.
- Ignore rules: `.gitignore` files inside the workspace and `.git/info/exclude`; files above the workspace root and the global excludes file are not read.
- `fs.write` writes UTF-8 text only (no binary content). `fs.patch` matches exact text; unified diffs need exact context (no fuzz) and apply to one file. CRLF files keep their endings for unified diffs; search/replace edits are byte-exact.
- Git plans cannot preview what a commit or push will contain (planning never spawns processes). `git.push` destinations are reported as unresolved remote names. In a sandbox whose `HOME` is a scratch directory, git has no user identity until the broker provides one. Worktree `.git` files that point outside the workspace are followed by git.
- `shell.exec` with `shell: true` runs `sh -c` on Unix and Windows PowerShell (`-NoProfile -NonInteractive -Command`) on Windows; classification covers both cmd and PowerShell syntax but remains a heuristic.
- Output schemas are produced from typed structs and round-trip checked; there is no general JSON Schema validator for third-party tool results yet.
