# AiDEX CLI

`aidex` is the first active AiDEX client. Its Rust command surface lives in `crates/aidex-cli`; the desktop and installer Tauri clients remain in place while the Rust foundation stays the primary implementation focus.

## Agent runtime

The interactive command delegates to Oh My Pi's documented `omp` CLI so its terminal UI and approval dialogs stay upstream-owned. The headless `aidex run` command uses the pinned TypeScript SDK in `src/run.ts` and its explicit restricted-session API. AiDEX does not fork or import Oh My Pi's internal Rust crates: upstream documents those crates as implementation details behind its package.

Oh My Pi was selected over the original [Pi monorepo](https://github.com/badlogic/pi-mono) because its documented CLI and SDK support Bun 1.3.14+, matching this workspace. The Pi CLI package currently declares Node.js `>=22.19.0`; both projects are MIT-licensed. AiDEX consumes Oh My Pi through its supported runtime/package interfaces rather than importing its private Rust crates.

The dependency is pinned to `@oh-my-pi/pi-coding-agent` `18.8.7`, installed by Bun. Upstream references:

- [Oh My Pi repository](https://github.com/can1357/oh-my-pi)
- [CLI reference](https://github.com/can1357/oh-my-pi/blob/main/docs/cli-reference.md)
- [Approval modes and safety](https://github.com/can1357/oh-my-pi/blob/main/docs/approval-mode.md)
- [Native crate boundary](https://github.com/can1357/oh-my-pi/blob/main/docs/native-crates.md)
- [SDK integration](https://github.com/can1357/oh-my-pi/blob/main/docs/sdk.md)
- [Package metadata](https://github.com/can1357/oh-my-pi/blob/main/packages/coding-agent/package.json)

## Commands

```sh
bun install
bun run dev:cli -- --help
bun run dev:cli -- init --drive /path/to/removable-drive
bun run dev:cli -- status --drive /path/to/removable-drive
bun run dev:cli -- doctor
bun run dev:cli -- agent list
bun run dev:cli -- daemon start --drive /path/to/drive
bun run dev:cli -- daemon status --drive /path/to/drive --json
bun run dev:cli -- events tail --drive /path/to/drive --follow
bun run dev:cli -- events show --drive /path/to/drive --after 0 --limit 20
bun run dev:cli -- vault init --recovery-key --drive /path/to/drive
bun run dev:cli -- vault set openai --consumer provider:openai --kind api_key --drive /path/to/drive
bun run dev:cli -- vault list --drive /path/to/drive
bun run dev:cli -- vault lock --drive /path/to/drive
bun run dev:cli -- daemon stop --drive /path/to/drive
bun run dev:cli -- workspace add ~/src/project --name project --drive /path/to/drive
bun run dev:cli -- workspace ephemeral create project --drive /path/to/drive
bun run dev:cli -- approvals --pending --drive /path/to/drive
bun run dev:cli -- approve apr_01J... --drive /path/to/drive
bun run dev:cli -- policy explain --tool fs.delete --input '{"path":"build"}' --drive /path/to/drive
bun run dev:cli -- stop --emergency --lock-vault --drive /path/to/drive
bun run dev:cli -- index build --workspace project --drive /path/to/drive
bun run dev:cli -- search --symbol parse_config --drive /path/to/drive
bun run dev:cli -- context pack "fix the login timeout" --budget 8000 --drive /path/to/drive
bun run dev:cli -- memory add --tag build "build with bun run build" --drive /path/to/drive
bun run dev:cli -- rules show --drive /path/to/drive
bun run dev:cli -- chat
bun run dev:cli -- run "Explain this repository"
bun run login:omp
```

`aidex init --drive PATH` requires an existing removable-drive root, verifies writability and at least 10 MiB free space, then idempotently creates only the directories listed by `aidex-drive.json`. It never formats a drive or replaces existing files. The root-level `aidex-drive.json` is the canonical schema-v2 JSON manifest. A valid schema-v1 manifest is migrated explicitly, with the original kept as `aidex-drive.v1.json`; malformed and unsupported manifests are reported and left untouched.

`aidex status [--drive PATH]` reads drive health without creating directories or repairing the manifest. Without `--drive`, status and doctor use `AIDEX_DRIVE_ROOT`, which a portable launcher can set. Status reports the resolved root, manifest schema version, writability, and available space; a missing or invalid manifest is an error. Doctor retains its host and Oh My Pi checks, adds actionable active-drive health output, and reports whether a daemon is running on the active drive (informational; it does not affect doctor's exit code).

`aidex daemon start|status|stop [--drive PATH]` manage the `aidexd` daemon for a drive (default `AIDEX_DRIVE_ROOT`). `start` launches the `aidexd` executable next to `aidex` (falling back to `aidexd` on `PATH`), recovering a stale endpoint first, and waits until the daemon answers; `stop` waits until the daemon has closed its connections and removed its endpoint. The daemon requires an initialized schema-v2 drive (`aidex init`); it refuses other folders without changing them. Every daemon-backed command connects through IPC contract v2 (`protocol/commands/README.md`); when no daemon is running it exits 3 with a hint to run `aidex daemon start --drive <path>`.

`aidex events tail [--after N] [--workspace ID] [--session ID] [--kind PREFIX]... [--follow] [--drive PATH]` prints the drive's events strictly after cursor `N` (default 0) and exits; `--kind` takes dotted name prefixes such as `tool` or `approval.created` (see `protocol/events/README.md`). With `--follow` it streams `events.subscribe`; after a daemon restart, a dropped connection, or a slow-consumer disconnect it reconnects with backoff (0.25 s doubling to 5 s) and resumes strictly after the last acknowledged durable event or consumed watermark, so no matching durable event is printed twice or skipped. A matching durable event is acknowledged only after its line was flushed; ordered `events.watermark` notifications acknowledge filtered-out durable progress without printing, and ephemeral events never advance the resume cursor. `aidex events show [--after N] [--limit M] [filters]` prints one replay page (`events.replay`, `M` 1-256, default 50) and the cursor to continue from. An expired cursor is a permanent error (exit 8, with a hint naming the oldest retained cursor) rather than a silent gap; an unknown kind prefix or invalid id is exit 2. Human output is one line per event (`cursor  time  name  scope  data`; live-only events show `~cursor`).
Human `events show` fails with exit 1 if an event line, continuation cursor, or
stdout flush cannot be written; a truncated output page is not reported as success.

`aidex vault init [--recovery-key]`, `unlock [--recovery-key]`, `lock`, `status`, `set <NAME> --consumer <SCOPE>... [--kind KIND]`, `list`, `delete <NAME>`, and `passphrase change` (all with `[--drive PATH]`) manage the drive's encrypted vault through the daemon (`vault.*` methods). Passphrases, recovery keys, and secret values are read from the terminal without echo, or from stdin only with `--passphrase-stdin` (`init`, `unlock`, `passphrase change`: one line each; `passphrase change` reads the old then the new passphrase, or only the new one after a recovery-key unlock) or `--from-stdin` (`set`: all of stdin, one trailing newline removed). They are never accepted as arguments or environment variables; without a terminal and without the flag the command exits 2 before connecting. `init` prints the recovery key once (store it offline); `list` shows names, kinds, consumers, and update times, never values. `--consumer` takes `provider:<id>`, `mcp:<server-id>`, `process:<tool-id>`, or `git:<host>`; model, prompt, agent, session, log, env, and shell scopes are refused (exit 6). A wrong passphrase or a locked vault exits 5; a missing vault or secret exits 8. Secret values are only handed to consumers by the broker through scoped handles; the CLI has no command that prints a stored value.

`aidex provider add <ID> --base-url URL [--secret NAME] [--remote] [--no-discover] [--model MODEL_ID... --context-window N [--max-output N] [--tools native|structured|none] [--json-schema] [--role ROLE...]]`, `provider list`, `provider remove <ID>`, and `provider health <ID>` configure OpenAI-compatible providers (`--kind openai-compatible` is the default and only configurable kind). `--secret` names a vault secret; store the key with `aidex vault set <NAME> --consumer provider:<ID>` — keys are never accepted as arguments. Loopback endpoints are local; `--remote` marks a loopback tunnel as remote, and every other host is remote. `health` exits 7 unless the provider answered. `aidex model list` shows every model with locality, context window, tool support (`native`/`structured`/`none`), JSON-schema output, roles, and whether its limits are configured, discovered, or assumed, marks the default with `*`, and warns about providers whose listing failed. `aidex model route --task TASK [--privacy local-only|hybrid|cloud-preferred|air-gapped] [--context N] [--tools] [--json-schema] [--secret-heavy] [--model PROVIDER/MODEL]` explains the routing decision: primary model and its limits, ordered fallbacks, reasons, and every rejected candidate; when nothing qualifies it prints the reasons and rejections and exits 7. `aidex model use PROVIDER/MODEL` persists the default (exit 8 if the provider does not list it); `aidex model default` shows it. `aidex offline on|off|status` toggles the persisted offline switch: remote providers are refused without any connection, loopback models keep working. All accept `--drive PATH` and `--json`.

`aidex inference` manages local GGUF models through the daemon's `inference.*` methods: `import <FILE> [--name NAME]` copies and validates a model (id `m-<sha256>`), `catalog` lists models, the installed runtime, and saved settings, `install-runtime [DIRECTORY]` installs a verified llama.cpp distribution (`runtime-manifest.json`; see `inference/llama-cpp/README.md`), `start <MODEL_ID> [--backend auto|cpu|gpu] [--context N] [--threads N]` launches the loopback server (`auto` settings are planned from RAM, physical cores, and the runtime), `status` shows the state, settings, and the plan's reasons (including why the CPU backend was used), `stop`, `cancel`, `remove <MODEL_ID> [--yes]` (confirmation required; without a terminal pass `--yes`, else exit 2), and `host [--probe-gpu]` shows the memory, core, and GPU facts plans use (the GPU probe is opt-in). A missing runtime or model exits 8; long operations wait up to four hours and are cancelled when the CLI is interrupted.

`aidex session` (alias `aidex sessions`) inspects durable sessions through the daemon's `session.*` methods; every subcommand accepts `--drive PATH` and `--json`:

- `create --workspace ID [--agent ID] <TITLE>...` records a session (state `created`; nothing runs). `list [--workspace ID] [--state STATE] [--limit N]` lists sessions newest first; `show <ID>` prints state, host, record counts, the latest checkpoint, and any event gaps; `messages <ID> [--after N] [--limit M]` pages messages; `checkpoints <ID>` lists checkpoints and marks corrupt ones.
- `resume <ID>` prints the resume plan (checkpoint, messages since it, host changes, and what cannot be recovered: lost processes, interrupted tool calls, approvals, model state) and adopts an interrupted session on this host, leaving it `paused`. It says explicitly that continuing the work is not available yet: the native agent harness (#19) is not wired, and nothing is started. A session that is not interrupted or paused exits 1.
- `cancel <ID>` cancels a session; `branch <ID> --checkpoint <C>` creates a child session from a checkpoint. An unknown session exits 8; an invalid state name exits 2.

`aidex update` manages signed updates of the AiDEX bundle on the drive (`--drive PATH`, default `AIDEX_DRIVE_ROOT`; `--json`): `check <SOURCE>` verifies a package (a `.zip`/`.tar.gz`, an unpacked bundle, or a folder of packages, where the newest for this host is used) without changing anything; `apply <SOURCE> [--allow-downgrade] [--allow-unsigned]` stages, verifies (signature against the keys built into this AiDEX and the drive's revocation list, every file, target, drive schema, version rules), self-tests, and switches the drive to it — the new version takes effect the next time AiDEX starts, and the command says so; `rollback [--allow-unsigned]` switches back to the previous version after re-verifying it; `status` shows the active and previous versions, integrity (signer key id, SBOM), and the last or interrupted operation; `verify [--allow-unsigned]` re-verifies the installed version locally (signature, every file, SBOM). `check`, `apply`, `rollback`, and `status` go through the daemon (`update.*`), which also recovers interrupted updates when it starts. Integrity and signature refusals exit 11; downgrades, wrong platforms, and incompatible drive schemas exit 10; nothing is changed by a refusal. `--allow-unsigned` and `AIDEX_ALLOW_UNSIGNED=1` are development-only opt-ins for packages without a signature; a bad or untrusted signature is never accepted. `aidex doctor` also reports the installed bundle's version, signer key id, verification, and SBOM status, and fails (exit 1) when an installed bundle does not verify.

`aidex workspace` (alias `aidex project`) manages workspaces through the daemon's `workspace.*` methods; every subcommand accepts `--drive PATH` (default `AIDEX_DRIVE_ROOT`) and `--json`:

- `add <PATH> [--name N]` registers an existing host directory exactly as selected (relative paths resolve against the current directory). The drive root, a directory containing the drive, and AiDEX-managed directories are refused; an already registered root is a conflict (exit 1).
- `create <NAME>` creates an empty drive workspace under the drive's `workspaces/`; `list` shows every workspace with its availability (a missing host root is reported, never recreated); `show [ID|NAME]` defaults to the selected workspace; `select <ID|NAME>` persists the selection on the drive, so it survives daemon restarts.
- `remove <ID>` only unregisters; host files are never deleted. `--delete-data` also deletes an AiDEX-created (drive or ephemeral) workspace directory after verifying its ownership marker.
  Choose `--delete-data` in the original remove command: after unregistering, that
  id is gone and any retained directory must be removed manually.
- `discover <PATH>` reports the enclosing Git repository and nested repositories (bounded scan, links not followed).
- `ephemeral create <SOURCE>` copies a workspace (honouring `.gitignore`, skipping `.git/` and `.aidex/`, never following links); `ephemeral review <ID>` lists added/modified/deleted files with conflicts and unified diffs; `ephemeral apply <ID> [--path P ...]` re-reviews, shows the changes, asks for confirmation, and applies only the selected non-conflicting files, refusing if the workspace changed since that review (exit 1 when any selected change was skipped or failed); `ephemeral discard <ID>` deletes only the ephemeral directory.
  A selective `--path` must appear in the returned review, even when that review
  is truncated; omitted paths are refused with exit 2 before confirmation or apply.
- `remove --delete-data`, `ephemeral apply`, and `ephemeral discard` need `--yes` or an interactive confirmation; without a terminal (or with `--json`) and without `--yes` they refuse with exit 2 and change nothing. Declining the prompt exits 9.

The execution broker's commands (all with `--drive PATH` and `--json`):

- `aidex approvals [list] [--pending | --state STATE] [--session ID] [--limit N]` lists approval requests newest first (state, tool, summary, expiry, grant status). `aidex approvals show <ID>` renders the card: who asks, the request, its effect (for example "Writes 3 files in workspace demo: +12 −4" or "Uploads local commits on branch main to remote origin (...)"), workspace, session, risk and minimum sandbox, the deciding policy rule, the redacted command, targets, secret names (never values), scope options (`once`, `session` when offered, with what a session grant covers), expiry, and the redacted diff. `aidex approvals revoke <ID> [--reason TEXT]` revokes a pending request, an approved call that has not run, or a session grant.
- `aidex approve <ID> [--session] [--yes]` shows the card and asks `Approve once? [y/N]`; any answer but `y`/`yes` (including Enter) denies the request. Without a terminal, or with `--json`, approving needs `--yes` and otherwise exits 2 without changing anything. `--session` is accepted only when the card offers it. `aidex deny <ID>` denies without a prompt. Approving does not run anything by itself: the requester (the harness, or `tool.continue` for local invocations) continues the call, which the broker re-plans and refuses if the intent changed.
- `aidex policy show` prints the profile, privacy mode, host trust, and network mode (requested and effective), loaded policy documents (or why they failed: every call is then denied), hard rules, and whether an emergency stop is in effect. `aidex policy profile <safe|balanced|autonomous|offline-secure>` switches the profile (recorded as `policy.changed`); `aidex policy reload` re-reads `policies/*.toml`. `aidex policy explain --tool T [--input JSON] [--workspace W] [--session S] [--as-user]` plans the call and prints the decision per intent without running it, creating an approval, or recording events.
- `aidex tools list` prints every registered tool with its action, base risk, and effects.
- `aidex plugins list|show <NAME>|install <PATH> [--replace]|enable <NAME> [--trust SHA256]|disable <NAME>|uninstall <NAME>` manages plugin bundles of skills and MCP servers (`plugin.*`). `install` copies a folder or `.zip` (made absolute here) into the drive's `plugins/` directory, disabled and untrusted, and prints the review: manifest, declared permissions, skills, MCP server commands (environment variable names only, never values), and the tree hash. Links, traversal, undeclared executables, and plugins over 32 MiB are refused (exit 2); an installed name without `--replace` is exit 1. The first `enable` needs `--trust` with the exact tree hash from `show` (otherwise exit 6); if the files change afterwards the plugin is disabled and must be reviewed and trusted again. `disable` keeps the trust.
- `aidex stop --emergency [--lock-vault] [--reason TEXT]` freezes tool execution, cancels pending approvals, revokes unused approvals and session grants, terminates every managed process, and revokes secret handles (optionally locking the vault); it exits 1 if a process could not be confirmed terminated. `aidex resume` lifts the stop; cancelled approvals must be requested again. While stopped, tool requests fail with exit 6 (`policy_denied`).

`aidex mcp` inspects and controls the drive's MCP servers through the daemon's `mcp.*` methods (all with `--drive PATH` and `--json`): `list [--workspace ID|NAME]` prints each server's id, status (`stopped`, `starting`, `running`, `failed`, `needs_auth`), enabled state, transport, tool count, and redacted endpoint, plus server files that failed to load; `start <ID>` launches or connects to an enabled server and lists its tools; `stop <ID>` stops it; `tools <ID>` lists its tools (starting it when needed); and `logs <ID> [--limit N]` prints the newest lines of its scrubbed stderr and lifecycle log. Servers are configured, imported from `.mcp.json`, and enabled in the desktop app (or with `mcp.add`/`mcp.import`/`mcp.set_enabled`); their secrets are vault secrets for consumer `mcp:<id>` (`aidex vault set NAME --consumer mcp:<id>`). An unknown server exits 8; a disabled one exits 1.

Repository context commands use the daemon's `context.*`, `memory.*`, and `rules.show` methods. Each accepts `--workspace ID|NAME` (default: the selected workspace; none selected is exit 2), `--drive PATH`, and `--json`:

- `aidex index build [--no-wait]` starts (or joins) the workspace's background index job and, unless `--no-wait`, prints progress every 2 s and the final status; a completed build exits 0, a cancelled one 9, a failed one 1. Interrupting the CLI does not stop the job; `aidex index cancel` does (committed batches are kept and the next build resumes). `aidex index status` prints counters, state (`empty`, `building`, `partial`, `complete`), repository branch/HEAD/remote names, any corruption recovery, and the latest job report (added/updated/unchanged/removed/redacted and skipped files by reason).
- `aidex search <QUERY>... [--symbol | --file] [--path PREFIX] [--limit N]` searches the index offline: full text (every word must match), file names, or definitions. Secret files are never indexed and snippets are redacted.
- `aidex context pack <TASK>... --budget N [--session ID] [--path P]... [--no-rules] [--no-memory] [--no-search] [--text]` shows the context pack the native agent would get: each item's tokens, trust, score, and source, the dropped candidates with reasons, and with `--text` the delimiter preamble and the delimited item texts. The total never exceeds the budget (1-100000).
- `aidex memory list|add <TEXT>... [--tag T]...|edit <ID> [--text T] [--tag T]... [--clear-tags]|remove <ID>` with `--workspace ID|NAME` (default), `--user`, or `--session ID` inspects and edits memory. Secret-like text is refused (exit 2); memories added here are user-written.
- `aidex rules show` prints the workspace's `.aidex/rules.toml` commands and guardrails (which can only tighten policy) and the instruction files found (`AGENTS.md`, `CLAUDE.md`, `.aidex/instructions.md`). An invalid rules file is exit 2.

Skill commands use the daemon's `skill.*` methods (all with `--drive PATH` and `--json`). `--workspace ID|NAME` adds that workspace's project and repo skills; without it only drive and plugin skills are visible:

- `aidex skills list` prints each skill's id (`<scope>:<name>`), state (`enabled`, `disabled`, or `error`), and description or error, then any roots that were skipped (links, unreadable directories, duplicate names).
- `aidex skills show <ID>` prints the skill's details, front matter, and redacted instructions; control characters in skill text are replaced before printing.
- `aidex skills enable <ID>` / `aidex skills disable <ID>` record the decision in `private_config/skills.toml`. Project and repo skills start disabled and need `--workspace`; a skill with an error cannot be enabled (exit 2).

`aidex` and `aidex chat` open Oh My Pi's interactive terminal UI. AiDEX passes `--approval-mode always-ask`, disables OMP extensions, MCP, LSP, skills, session persistence, and automatic title generation, and requests a limited tool set. OMP still owns its CLI tool lifecycle and approval implementation; this is not an AiDEX sandbox or policy boundary.

`aidex run` is headless, in-memory, and strictly restricts OMP SDK tools to `read`, `grep`, and `glob` (`restrictToolNames: true`). It disables MCP, LSP, extensions, custom tools, and skills. Writes and shell execution are not exposed in this mode.

Interactive chat uses the user's existing OMP provider credentials. Configure them with `bun run login:omp`; model access still requires a provider account or API key.

The source-tree `aidex run` adapter is `apps/cli/src/run.ts`. Packaged binaries must ship that script and set `AIDEX_RUNNER`; `AIDEX_BUN` can select a non-default Bun executable.
## Machine-readable output

The global `--json` flag makes `init`, `status`, `daemon start|status|stop`, `events show`, `vault ...`, `workspace ...`, `update ...`, `doctor`, and `agent list` print exactly one compact JSON document on stdout. Failures never write to stdout: with `--json` the error is a single JSON line on stderr,

```json
{"error":{"code":"daemon_unavailable","exit_code":3,"message":"daemon not running on /media/aidex: ...","hint":"start it with `aidex daemon start --drive /media/aidex`"}}
```

and without it a `aidex <command>: <message>` line plus an optional `hint:` line. `aidex events tail` prints one JSON envelope per line with `--json` (`events show --json` prints the page as one document); `chat` and `run` are interactive/delegated and ignore `--json`. Clap usage errors keep clap's text format.

## Exit codes

Exit codes are stable (`crates/aidex-cli/src/exit.rs`, `Exit`); `error.code` in JSON output is the name column.

| Code | Name | Meaning |
| --- | --- | --- |
| 0 | `success` | The command succeeded. |
| 1 | `failure` | General failure (including an already-running daemon on `daemon start`, or doctor finding a problem). |
| 2 | `usage` | Invalid command line, or arguments the daemon rejected (`invalid_params`). |
| 3 | `daemon_unavailable` | No daemon, unreachable daemon, daemon stopping, or connection dropped. |
| 4 | `drive_error` | Drive missing, not removable, unwritable, or its manifest is missing/invalid. |
| 5 | `auth_failed` | Vault locked or authentication failed (including a rejected daemon capability). |
| 6 | `policy_denied` | Denied by policy or permissions. |
| 7 | `model_unavailable` | No usable model or provider. |
| 8 | `not_found` | The requested entity or history does not exist (e.g. an expired event cursor). |
| 9 | `cancelled` | Cancelled or interrupted. |
| 10 | `incompatible_version` | CLI and daemon speak different IPC contract versions, or the daemon lacks the method; or an update is a downgrade, for another platform, or needs a newer drive schema. |
| 11 | `integrity_failed` | An installed bundle or update package failed integrity or signature verification (same meaning as the launcher's exit 11). |

`chat` and `run` map the delegated Oh My Pi process status onto this table: 0 stays 0, an interrupt (status 130 or a signal) is 9, any other status is 1. Daemon JSON-RPC error codes map to exit codes as listed in `protocol/commands/README.md`.
