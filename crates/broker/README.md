# aidex-broker

The execution broker (issue #17; spec §4.4, §12.4-12.5, §13-15, §48, §50, §81, §92, §94): the security boundary between reasoning and action. Agents, the harness, skills, adapters, remote devices, and the local user submit structured tool invocations; the broker decides, asks, contains, and records. It owns the `aidex-tools` registry behind the daemon's `tool.*` methods, and it implements the reserved `approvals` domain. The native harness (`aidex-harness` `NativeEngine`, desktop agent runs) currently keeps its own registry, launcher, policy evaluation, and approvals (`approval.list|respond`); moving it onto this broker is follow-up work (#19), and until then the guarantees below cover broker-submitted calls only.

It depends on `aidex-core`, `aidex-drive`, `aidex-events`, `aidex-policy`, `aidex-providers`, `aidex-sandbox`, `aidex-tools`, `aidex-vault`, `aidex-workspace`, and embedded SQLite. No async runtime; `Broker` is `Send + Sync` and is shared as `Arc<Broker>`.

## Flow

```text
ToolInvocation {session, workspace, agent?, tool, input, origin, call_id?}
  │  emergency stop in effect? ──────────────────────────────► Err(Stopped) + tool.denied
  ▼
ToolRegistry::plan  (closed schema, ConfinedRoot paths, facts, diff preview; no side effects)
  │  rejected input ─────────────────────────────────────────► Failed + tool.requested/tool.failed
  ▼
intents = plan.intents() + broker facts
  │  protected paths (vault, identity, sessions, config, memory, index, runtime) → `protected`
  │  writes beneath policies/ → `policy.change`; remote origin → `remote.action`
  │  git.push: .git/config remote → classified destination; stored HTTPS credential → `secret.use`
  ▼
aidex_policy::evaluate (profile, privacy, host trust, network mode, policy documents, project rules)
  │  agent-side origin + policy.change/broker.disable/vault.modify/credential.export/secret.dump → Deny
  │  events: tool.requested, policy.decision × intents
  ├── Deny / rate limit reached ─────────────────────────────► Denied + tool.denied (nothing runs)
  ├── RequireApproval ── matching session grant? ── yes ───► execute (permit: Approval{grant})
  │        │ no; sandbox unavailable? ── yes ───────────────► Denied (fail closed, no prompt)
  │        ▼
  │   approval (card, fingerprint, expiry) ─────────────────► PendingApproval + approval.created
  │        approve_once / approve_session ── continue_after_approval(id)
  │        re-plan → same fingerprint? policy still allows? unused? → execute (permit: Approval{id})
  └── Allow / Sandbox{level} ────────────────────────────────► execute (permit: Policy{rule})

execute: sandbox negotiation for process tools at max(decision level, brokered) (failure → Denied)
         → secret handles (secret.granted) → launch permit registered for the plan fingerprint
         → ToolRegistry::execute(ExecutionPermit bound to the plan fingerprint)
         → SandboxLauncher (sandbox.negotiated, process.started/output/exited)
         → handles revoked (secret.revoked) → Completed | Failed
```

## API

```rust
let broker = Broker::builder(BrokerConfig::for_drive(&paths)?, workspaces /* Arc<dyn WorkspaceResolver> */, events /* Arc<dyn EventSink> */)
    .vault(vault)                 // optional; credentials come from here
    .sandbox(Sandbox::locate())   // default; helper next to the executable
    .registry(ToolRegistry::with_builtin_tools()) // default
    .clock(Arc::new(SystemClock)) // default; ManualClock for tests
    .open()?;                     // cancels approvals left pending by a previous run

broker.submit(ToolInvocation::new(session, workspace, ToolId::new("fs.write")?, input, Origin::Agent).with_agent(agent))
    -> Result<BrokerOutcome, BrokerError>;
broker.continue_after_approval(&approval_id) -> Result<BrokerOutcome, BrokerError>;
broker.explain(&invocation) -> Result<Explanation, BrokerError>;          // dry run, no events

broker.resolve_approval(&id, ApprovalResolution::{ApproveOnce, ApproveSession, Deny}, &Principal::LocalUser) -> Result<ApprovalRecord, _>;
broker.revoke_approval(&id, &principal, reason) / cancel_approval(&id, reason) / expire_due()
broker.approval(&id) / list_approvals(&ApprovalFilter) / approvals() -> &ApprovalStore (reads)

broker.change_policy(PolicyChange::{Profile, Privacy, HostTrust, NetworkMode, ReloadDocuments}, &Principal::LocalUser) -> Result<PolicyStatus, _>;
broker.policy_status() / settings()
broker.cancel_call(&call_id) / cancel_session(&session_id, &principal) -> SessionCancelReport
broker.emergency_stop(reason, &principal) -> Result<EmergencyReport, _>;  broker.resume_operations(&principal)
broker.shutdown_processes(drive_removed)                                   // daemon stop / drive removal
broker.tools() / tool_specs() / sandbox() / launcher()

ModelEgress::new(Arc<Broker>)?.with_offline_switch(offline) // impl aidex_providers::{EgressGate, CredentialResolver}
```

| Type | Purpose |
| --- | --- |
| `ToolInvocation` | `session_id`, `workspace_id`, `agent_id?`, `tool_id`, `input` (untrusted), `origin`, `call_id?` (harness tool-call id; generated when absent). `Debug` never prints the input. |
| `Origin` | `Agent`, `User`, `Remote { device_id }`, `Skill { skill_id }`, `Adapter { adapter_id }`. Only `User` may change policy. |
| `BrokerOutcome` | `Completed { call_id, output: ToolOutput, permit: PermitInfo { issuer, sandbox } }`, `PendingApproval { call_id, approval_id, card }`, `Denied { call_id, reason, rule }`, `Failed { call_id, failure: ToolFailure { reason, code, message } }`. Serializes with `outcome` = `completed`/`pending_approval`/`denied`/`failed`. |
| `ApprovalCard` | `approval_id`, `requester { origin, agent_id }`, `tool_id`, `action`, `summary`, `effect` (e.g. "Writes 3 files in workspace demo: +12 −4", "Uploads local commits on branch main to remote origin (https://github.com/acme/app.git)"), `workspace_id`, `workspace_name`, `session_id`, `risk`, `scope_options` (`once`, and `session` when policy allows), `created_at`, `expires_at`, `rule`, `reason`, `sandbox`, `targets`, `command` (redacted), `secrets` (name + consumer only), `diff_preview` (redacted, 32 KiB per file), `session_grant` (what approve-for-session covers), `warnings`. |
| `ApprovalRecord` | `approval_id`, `session_id`, `workspace_id`, `call_id`, `tool_id`, `state`, `card`, `intent_fingerprint` (hex; for `aidex_sessions::ApprovalRef`), `consumed_at`, `grant { expires_at, active }`. |
| `ApprovalState` | `pending`, `approved { scope, by, at }`, `denied { by, at }`, `expired { at }`, `revoked { by, at, reason }`, `cancelled { at, reason }` (`status` tag). |
| `Principal` | `LocalUser`, `RemoteDevice { device_id }`. Remote devices can resolve approvals (recorded) but cannot change policy or resume after a stop. |
| `Explanation` | plan summary, effect, risk, combined decision, per-intent decisions, the session grant that would apply, why the sandbox is unavailable, targets, diff, warnings. |
| `EmergencyReport` | `stopped_at`, `reason`, `cancelled_approvals`, `revoked_approvals`, `cancelled_calls`, `terminated_processes`, `failed_terminations`, `revoked_handles`, `vault_locked` (set by the daemon). |
| `PolicySettings` | `profile`, `privacy`, `host_trust`, `network_mode`; default Balanced / hybrid / known / normal; persisted in `private_config/policy.toml`. |
| `SandboxLauncher` | `aidex_tools::ProcessLauncher` over `aidex-sandbox`; refuses any launch whose plan fingerprint the broker did not register. |
| `BrokerError` | `code()` and stable `reason()`: `emergency_stop`, `unknown_tool`, workspace reasons, approval refusals (`approval_not_found`, `approval_pending`, `approval_denied`, `approval_expired`, `approval_revoked`, `approval_cancelled`, `already_resolved`, `already_consumed`, `scope_not_offered`, `intent_changed`, `invocation_lost`), `unauthorized`, `invalid_request`, `approval_store`, `policy_settings`, `event_not_recorded`, `internal`. |

## Guarantees

- **No unpermitted execution.** The broker owns its tool registry and never hands it out; every execution goes through one function that builds an `ExecutionPermit` from the evaluated plan, and the registry refuses a permit whose fingerprint differs, an altered plan, a different context, or a second execution. Processes launch only through `SandboxLauncher`, which refuses plans the broker did not register (tested with a self-made permit).
- **Facts, not claims.** Intents come from the plan's computed facts plus broker-verified facts; nothing an agent asserts about risk or destination is used. Deny wins, unmatched actions are denied by default, and policy-document or project-rule load failures deny every call (fail closed).
- **Bound, single-use approvals.** An approval binds a SHA-256 fingerprint over the plan without its random ids (tool, version, normalized input, resolved paths and their content hashes, diff preview, classified command, destination) plus every evaluated intent's fingerprint and the origin. `continue_after_approval` re-plans the stored invocation and refuses `intent_changed` if anything differs, re-evaluates policy, and consumes the approval with a compare-and-set so it runs at most once.
- **Atomic transitions.** The SQLite store (`approvals.sqlite3`, WAL, `synchronous=FULL`, symlinked files refused) changes state only with `UPDATE … WHERE status = …` inside immediate transactions; approve vs. deny vs. cancel vs. expire vs. revoke races have exactly one winner (tested with threads). Resolving after the deadline expires the request instead.
- **Session grants.** `approve_session` (only when offered) creates a grant keyed by session, workspace, tool, origin, the input without content fields (file content, patch edits, commit messages), and every intent's action and attributes (paths, command, destination, secret). Later identical intents in that session run without a prompt and are recorded as `policy.decision` with rule `session_grant:<id>`; grants never cross sessions or workspaces, never override a deny, apply only while policy still offers `session`, expire (8 h by default), and are revocable.
- **Secrets stay out of reach.** A plan's credential (an HTTPS git remote with a vault secret whose policy allows `git:<host[:port]>`) is evaluated as `secret.use`, shown on the card by name and consumer only, and issued as a handle only after the permit; the launcher resolves it right before spawning and passes it as `GIT_CONFIG_*` environment entries (an `http.<url>.extraHeader` scoped to that remote, credential helpers reset, system config ignored). The value never reaches argv, events, plans, cards, outcomes, or `Debug` (tested by grepping every surface for the token and its encoding). Handles are revoked when the call ends. Plain HTTP is eligible only for loopback.
- **Audit trail.** `tool.requested`, `policy.decision` (per intent), `tool.denied`, `approval.created|approved|denied|expired|revoked|cancelled`, `secret.granted|revoked`, `sandbox.negotiated`, `process.started|output|exited`, `policy.changed`, `emergency.stopped|resumed`, plus the registry's `tool.started|completed|failed`, `file.changed`, `diff.created`. An event that must precede an effect is fail-closed: if it cannot be recorded nothing runs. All text is redacted and bounded by the event sink; raw command lines and tool input are never put in events.
- **Emergency stop.** Freezes new submissions and continuations (`Stopped`), cancels running calls, terminates every managed process tree (registry shut down until resume), withdraws launch permits, revokes every vault handle, cancels pending approvals, and revokes unused approvals and session grants. Locking the vault is the daemon's option.
- **Model egress.** `ModelEgress` passes the offline switch, then the emergency stop, then a `model.invoke` intent (remote locality, classified destination); approval verdicts refuse (a provider call cannot wait for a human). Provider credentials are evaluated as `secret.use` for `provider:<id>`: a deny refuses; the user's vault binding of the secret to that provider is its standing approval. Each use is a 30 s handle with `secret.granted`/`secret.revoked`.

## Enforcement per OS

The broker enforces policy, approvals, permits, and secret scoping identically everywhere. Process containment is exactly what `aidex-sandbox` negotiates (see its README):

| | Windows | Linux | macOS |
| --- | --- | --- | --- |
| `brokered` (Balanced, Autonomous) | Job Object tree kill, cleared environment, watchdog limits; network not OS-restricted (policy governs destinations) | process group, rlimits; network per policy | process group; network per policy |
| `restricted` (Safe, Offline Secure, risky commands) | **fails closed**: process tools are denied | Landlock read/write confinement; tools that need no network also need network denial (unprivileged user namespaces), otherwise they fail closed | `sandbox-exec` write confinement and network denial; reads not confined |
| `ephemeral` | fails closed | fails closed | fails closed |

In-process tools (`fs.*`) run inside the broker on their fixed, confined plan; that satisfies levels up to `restricted` (the plan can touch only its resolved paths), never `ephemeral`. Validation: the full suite passes on Windows locally; Linux and macOS run in CI.

## Limitations

- **Same-user processes can reach the daemon.** A tool process at `brokered` level runs as the current OS user without filesystem confinement, so it can read the daemon's endpoint capability and call IPC methods as the local user (for example resolve its own approval). Profiles keep unknown commands at `restricted` or approval (Windows then denies them; Linux/macOS confinement keeps the drive's runtime directory out of reach), but Autonomous and explicit allow rules run more at `brokered`. Approvals confirmed through a channel tool processes cannot reach (an attested client, the desktop, a paired device) are follow-up work.
- Approval prompts are not pushed to the CLI; `aidex approvals`/`approve`/`deny` poll the daemon, and the desktop or harness can follow `approval.*` events. Remote approvals (spec §53, signed and replay-protected) and their authorization are #29: `Principal::RemoteDevice` is recorded but not authenticated here.
- Pending approvals hold their invocation in memory only (raw input is never persisted), so a daemon restart cancels them; approved-but-not-continued calls report `invocation_lost`.
- Only git HTTPS credentials are discovered automatically; `process:<tool>` consumers need the injection hooks extended. MCP servers (`mcp:<id>`) get their secrets from `aidex-mcp`, which launches them through this broker's sandbox (so emergency stop kills them) but outside tool plans. Credentials are read as tokens (Basic auth with user `x-access-token`).
- Destinations are classified before DNS resolution; no egress proxy enforces them for processes (the sandbox has no network allowlist backend). Remote URL rewrites (`insteadOf`, includes, several URLs) make a push destination unknown and suppress credential injection.
- Model-egress intents are not tied to a session (the provider API has no session), so session-layer rules and grants do not apply to them; provider ports are assumed to be 443.
- Host trust is a drive-wide setting until per-host trust records exist; the shell classifier remains a heuristic contained by the sandbox; secret values handed to the sandbox request are ordinary `String`s (not zeroized) for the moment of the launch.
- Rate limits are in memory and per daemon run.
