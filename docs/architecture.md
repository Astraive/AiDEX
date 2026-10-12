# AiDEX architecture

## Product boundary

The CLI is the first active client surface. Its `aidex` Rust binary reaches backend services through the `aidexd` daemon's JSON-RPC API (`protocol/commands/README.md`), uses the host crate for diagnostics, and, until the native agent lands, delegates interactive agent sessions to the pinned Oh My Pi CLI and headless sessions to Oh My Pi's restricted Bun SDK. The desktop and installer Tauri shells remain in place, but the product UI does not own domain behavior.

## Repository layers

```text
crates/aidex-cli (`aidex`)
   ├── aidexd::Client ──JSON-RPC 2.0, IPC contract v2, authenticated loopback──┐
   ├── aidex-host (host diagnostics)                                          │
   └── pinned Oh My Pi runtime (CLI TUI + restricted Bun SDK)                 │
                                                                              ▼
crates/aidexd (`aidexd`)
   ├── transport: handshake, Router (`<domain>.<verb>`), bounded streams
   └── Services ── crates/core, drive, events, sessions, storage, workspace, vault
        ├── events: typed catalog, one cursor per drive, replay + push stream
        ├── sessions: durable sessions, checkpoints, startup recovery, resume plans
        ├── context: local index + search, project rules, memory, budgeted context packs
        └── broker: `tool.*` execution, approvals, policy settings, model egress, emergency stop

crates/broker (`aidex-broker`)
   ├── crates/tools: plan (no side effects) → permit-bound execute
   ├── crates/policy: pure evaluation of intents built from plan facts
   ├── approvals: bound cards, SQLite compare-and-set store, session grants
   ├── crates/sandbox: every tool process (fail closed)
   └── crates/vault: short-lived secret handles for the child or provider request

crates/launcher (`AiDEX.exe` / `./aidex` on the drive)
   └── crates/bundle: verify app/current.json → bundle.json → binaries, then run `aidex`

crates/packager (`aidex-packager`, developer/CI only)
   └── crates/bundle: build, archive, install, smoke-test portable bundles

apps/desktop, apps/installer
   └── future clients of the same daemon API
```

Every important state transition is a typed `aidex-events` event (`protocol/events/README.md`). Domain crates emit through the `EventSink` trait; the daemon owns the drive's durable log (`private_sessions/events.sqlite3`) and serves it to every client as `events.replay` and the `events.subscribe` push stream, which resumes after a client's last acknowledged cursor without duplicates or silent gaps.

Sessions are durable drive state (`aidex-sessions`, `private_sessions/sessions.sqlite3` plus content-addressed blobs). Logical state (messages, tool calls, approvals, artifacts, and hash-verified checkpoints) is portable across restarts, drive moves, and hosts; host-bound state (processes, approval prompts, model memory) is never serialized. At startup the daemon interrupts sessions that were active, marks running tool calls lost, and records event gaps explicitly; `session.resume_plan` reports what a resume restores and what must be recreated. Continuing a session is the native harness's job (#19).

Repository context is local drive state (`aidex-context`): a rebuildable per-workspace index in `private_index/<workspace>/`, memory in `private_memory/`, and project rules read from the workspace's `.aidex/rules.toml`. The harness asks for a context pack per task; packs are bounded by a token budget and select relevant chunks rather than whole repositories, and every item is delimited and labelled with its trust level so repository text is never mistaken for instructions.

The desktop and installer are independent Tauri v2 projects (each `src-tauri` is its own Cargo workspace with path dependencies on the crates). Their Rust shells own desktop integration only; domain services belong in the Rust workspace. The desktop bridge resolves the session drive (session choice, `AIDEX_DRIVE_ROOT`, or the drive holding the executable), starts `aidexd` from beside the app when asked (and stops it on exit only if it started it), proxies any daemon method through `rpc_call`, and forwards `<domain>.subscribe` streams as Tauri events. The installer calls `aidex_drive::list_volumes` and `aidex_drive::init` directly. Both UIs use `@aidex/client` (`packages/client`), whose DTOs mirror the Rust types. `crates/aidex-cli` owns the first Rust command surface and treats Oh My Pi as an external agent runtime, not as AiDEX core. `protocol/` is the intended source of frontend/backend contracts. The daemon IPC transport (framing, handshake, error codes, method naming) is defined by `crates/aidexd`'s Rust types and specified in `protocol/commands/README.md`; frontends use `aidexd::Client` or follow that document. Until generation and compatibility checks exist, no other handwritten duplicate contract should be treated as canonical.

## Rust workspace boundaries

The Rust workspace remains the primary implementation focus. Its first layer is intentionally small: shared core types, portable-drive paths, host capability discovery, events, durable storage/session foundations, the workspace registry and path confinement (`aidex-workspace`: explicitly selected host, drive, and reviewable ephemeral workspaces), the `aidex-sandbox` process isolation library, typed two-phase workspace tools (`aidex-tools`), the encrypted vault (`aidex-vault`), the execution broker and approvals (`aidex-broker`: the daemon's `tool.*` execution path; policy, approvals, sandboxed launches, secret handles, emergency stop), model providers and routing (`aidex-providers`, `aidex-router`: provider registry, OpenAI-compatible streaming provider, offline egress gate, privacy-aware selection with explicit fallback), repository context (`aidex-context`: offline index and search, project rules, memory, budgeted context packs), portable bundles, their signatures, and signed atomic updates (`aidex-bundle`, `aidex-launcher`, `aidex-packager`, `aidex-updater`), the `aidexd` process boundary, and the `aidex` CLI. Depend on lower-level shared contracts rather than creating cycles. Future domains listed in the specification—harness, inference, agent adapters, MCP/skills/plugins, identity/remote, cleanup—must be introduced when there is a real API and consumer, not as empty service crates.

## Portable-drive data is not repository source

`portable/` describes an installable drive distribution. Application binaries reach a drive only as verified bundles installed by `aidex-bundle` into `app/versions/`, selected by `app/current.json`, and started through the drive-root launcher (`portable/packaging/README.md`). It is distinct from this source repository and must never contain user secrets, model weights, or generated binaries in source control. Per-drive state is runtime data (including vault, identity, sessions, and memory); bundled public assets are versioned or verified through manifests. The installer reports exactly what it did: it initializes the drive layout and manifest, and lists bundle copy, models, and agents as not available until their components exist; it never claims a full installation.

## Security invariants

- Agent output and project content are untrusted input, not policy.
- No agent gains unrestricted shell or filesystem access by default.
- Secret access is mediated and scoped; raw secrets must not enter model context or logs.
- Remote control uses authenticated, replay-resistant, end-to-end protected messages; a relay is transport only.
- AiDEX can report and clean its own managed residue, but cannot promise removal of host-OS telemetry.
- A new host uses conservative behavior until trust and capabilities are established.

These are design constraints, not claims that the corresponding runtime enforcement is already implemented.
