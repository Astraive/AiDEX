# aidex-policy

Pure, deterministic policy evaluation for brokered agent actions (part of issue #17).

The execution broker turns an agent or tool request into a normalized `Intent` — facts it computed itself (resolved paths, a classified command line, a classified destination), never claims copied from model output — and asks `evaluate` for a `Decision`. This crate **decides; it does not enforce**. Enforcement, approvals, audit events, and CLI commands belong to the broker work that consumes it.

The crate has no I/O during evaluation, no clock, no randomness, no async runtime, and no dependency on `aidex-events` or `aidex-sandbox`. It depends on `aidex-core`, `serde`, `sha2`, `toml`, and `globset`.

## API

```rust
pub fn evaluate(policies: &PolicySet, context: &EvaluationContext, intent: &Intent) -> Decision;

pub fn classify_shell(command: &str, platform: ShellPlatform) -> ShellClassification;
pub fn classify_network(host: &str, port: u16) -> Result<NetworkTarget, PolicyError>;
pub fn redact_command(line: &str) -> String;

impl Intent { pub fn fingerprint(&self) -> [u8; 32]; pub fn fingerprint_hex(&self) -> String; }
impl PolicySet { pub fn load(dir: impl AsRef<Path>) -> Result<PolicySet, PolicyError>; /* … */ }
impl ProjectRules { pub fn load(path: impl AsRef<Path>) -> Result<ProjectRules, PolicyError>; /* … */ }
```

| Type | Purpose |
| --- | --- |
| `Action` | Closed catalogue of 28 dotted actions (`fs.write`, `git.push`, `disk.format`, …). Unknown ids do not parse. |
| `ActionPattern` | `fs.write`, `git.*`, or `*` in rules. |
| `Intent`, `IntentAttributes` | Action, session/workspace/agent/tool/skill ids, broker summary, and facts: `paths: Vec<PathScope>`, `command: Option<ShellCommand>`, `network: Option<NetworkTarget>`, `secret: Option<SecretUse>`, `model_locality`, `remote_write`, `destructive`. |
| `PathScope` | Resolved `path`, `workspace_relative` (`Some` = inside the workspace), `class` (`ordinary`, `sensitive`, `protected`). |
| `ShellCommand` | A command line plus its classification. Construction and deserialization always re-classify; `Debug` redacts credentials. |
| `NetworkTarget` | Host, port, and `NetworkKind` (`loopback`, `local_network`, `public_ip`, `domain`), always computed by `classify_network`; deserialization re-classifies. |
| `Decision` | `verdict`, `reason`, `rule: RuleRef`, `effect_summary`, `sandbox` (minimum isolation even after approval), `rate_limit`. |
| `Verdict` | `Allow`, `Deny`, `Sandbox { level }`, `RequireApproval { scope_options, expires_in }`, `RateLimit { max, per }`. |
| `EvaluationContext` | `profile`, `privacy`, `host_trust`, `network_mode`, optional `host_id` (selects machine-layer rules). |
| `PolicySet` | Layer rules by `(Layer, subject)`, global `NetworkSettings`, and per-workspace `ProjectRules`. |
| `HARD_RULES` | The documented hard safety rules. |

`Rewrite` (spec §13) is intentionally **not** a verdict: silently changing what an agent asked for would make the executed action differ from the one shown on the approval card, fingerprinted, and audited.

## Precedence algorithm

Each intent is split into **units**: its own action once per path, plus every action its classified command implies (`npm install` → `package.install`, `git push` → `git.push`, `mkfs` → `disk.format`, `curl` → `network.request`) once per extracted destination. Units are evaluated independently and combined by strictness, so allowing `shell.exec` never allows the package install, push, or network request the command performs.

| Step | Source | Effect |
| --- | --- | --- |
| 1 | Hard deny rules | `Deny`. No layer, profile, or approval overrides them. |
| 2 | Fact completeness | `Deny` when required facts are missing (no path for `fs.*`, no classified command, no destination for `network.request`, unknown model locality, no secret, invalid relative path). |
| 3a | Layer deny | Any matching `deny` in any applicable layer wins. |
| 3b | Most specific layer | `session` > `tool` > `skill` > `agent` > `workspace` > `machine` > `global`; within that layer the strictest matching rule. Layers may loosen or tighten profile defaults. |
| 3c | Profile default | The strictest matching rule of the effective profile. |
| 3d | Deny by default | No rule matched. |
| 4 | Floors | Hard approval rules, project rules, privacy mode, network mode, host trust. Combined by strictness: they only tighten. |
| 5 | Finish | Strictest unit wins; the profile's baseline sandbox applies; `Allow` with a sandbox of `restricted` or stronger is reported as `Sandbox`. |

Strictness: `Allow` < `RateLimit` < `Sandbox` < `RequireApproval` < `Deny`. Ties keep the deeper sandbox, the intersection of approval scopes (falling back to `once`), the shorter expiry, and the slower rate. Sandbox and rate-limit constraints are carried on the `Decision` even when the verdict is an approval.

Rule conditions about facts a unit lacks (a path glob on an intent without paths) do not match. `only_programs` requires every program a command runs to be listed (safe for allow rules); `any_programs` matches if one is (for deny rules). Path globs are workspace-relative, case-insensitive, `*` stays within a component, and a pattern also covers everything beneath it.

## Hard rules (spec §50)

| Id | Verdict |
| --- | --- |
| `hard.disk-format`, `hard.drive-raw-write`, `hard.credential-export`, `hard.secret-dump`, `hard.directory-delete-arbitrary`, `hard.vault-modify`, `hard.broker-disable` | `Deny` for the action, including when a command implies it (`mkfs`, `dd of=/dev/sda`, `rm -rf /`, `diskpart`, reading `~/.ssh/id_rsa` into `curl`). |
| `hard.protected-path` | `Deny` for any `fs.*` intent on a `protected` path (AiDEX vault, identity, broker state). |
| `hard.policy-change-authorization` | `policy.change` needs one-time approval at least. |
| `hard.destructive-command` | Commands classified `destructive` need one-time approval at least, even in Autonomous. |
| `hard.destructive-effect` | Intents the broker marks `destructive` need one-time approval at least. |

## Profiles (spec §49, §14)

`ask` = approval once or for the session; `once` = one-time approval only; `—` = no rule, so deny by default.

| Case | Safe | Balanced | Autonomous | Offline Secure |
| --- | --- | --- | --- | --- |
| `fs.read`/`fs.search`/`fs.write` in workspace | allow | allow | allow | allow |
| `fs.read`/`fs.search` outside | ask | ask | ask | ask |
| `fs.write` outside | once | once | ask | once |
| `fs.delete` in workspace / outside | ask / once | allow / once | allow / once | ask / once |
| `sensitive` path, any `fs.*` | once | once | once | once |
| shell/process `low` | allow | allow | allow | allow |
| shell/process `medium` | ask | sandbox (restricted) | allow | ask |
| shell/process `high` | once | ask | sandbox (restricted) | once |
| shell/process `destructive` | once | once | once | once |
| `network.request` | allow | allow | allow | loopback/LAN only |
| `package.install` | ask | ask | sandbox (restricted) | — |
| `git.read`, `git.commit`, `git.branch` | allow | allow | allow | allow |
| `git.pull` / `git.push` | ask / once | allow / ask | allow / ask | ask / once |
| `secret.use` | once | ask | ask | once |
| `mcp.call` / `agent.spawn` | ask | allow | allow | ask |
| `model.invoke` local / remote | allow / ask | allow | allow | allow / — |
| `artifact.create` | allow | allow | allow | allow |
| `host.mount` / `remote.action` | — | once / ask | ask | — |
| `policy.change` | once | once | once | once |
| any `remote_write` intent | once | ask | ask | once |
| Baseline sandbox | restricted | brokered | brokered | restricted |
| Network cap / privacy floor | allowlist / — | — | — | offline / local only |

High and destructive commands and package installs run at least `restricted` even after approval. Every profile is at least as strict as the ones above it for every intent (tested).

## Floors

| Input | Rule |
| --- | --- |
| Privacy `air_gapped` | Any network egress, including loopback, is denied. |
| Privacy `local_only` | External egress (public domains/IPs, remote models, unknown destinations) is denied. |
| Network `offline` | External egress denied; local network per `local_network`. |
| Network `allowlist` | Domains must match `allow_domains` (`name` or `*.name`, subdomains only); public IPs per `arbitrary_ip`; unknown destinations denied. |
| Network `normal` | Public IPs per `arbitrary_ip`, local network per `local_network`, unknown destinations ask. |
| Host `new` | `secret.use` and remote `model.invoke` need one-time approval. |
| Host `restricted` | As `new`, plus `host.mount`/`remote.action` denied, `package.install` once; profile at least Balanced. |
| Host `untrusted` | As `restricted`, plus `secret.use` denied; profile at least Safe. |
| Project rules | `never_modify` denies `fs.write`/`fs.delete` beneath the paths; `require_approval` asks for the listed actions. |

`local_network` and `arbitrary_ip` are `allow`, `ask` (default), or `deny`. The Offline Secure profile implies `local_only` privacy and `offline` network; Safe caps the network at `allowlist`.

## Policy documents

`PolicySet::load(DrivePaths::policies())` reads every `*.toml` file directly in the directory, in file-name order; other files are ignored. A missing directory, symlinked directory or file, file over 256 KiB, malformed TOML, unknown field or value, unsupported `schema_version`, or inconsistent rule fails the whole load.

```toml
schema_version = 1
scope = "workspace"          # global | machine | workspace | agent | skill | tool | session
subject = "ws_01J..."        # required for every scope except global

[[rules]]
id = "review-pushes"         # a-z 0-9 . _ -, unique per layer and subject
action = "git.*"             # action id, category.*, or *
verdict = "require_approval" # allow | deny | sandbox | require_approval | rate_limit
approval_scopes = ["once"]   # require_approval only (default once + session)
expires_in_secs = 300        # require_approval only (default 600, max 86400)
# sandbox = "restricted"     # sandbox only: brokered | restricted | ephemeral
# rate_max = 10              # rate_limit only, with rate_per_secs
reason = "pushes are reviewed"
[rules.when]                 # all optional; all must hold
remote_write = true
# location, paths, path_class, min_risk, max_risk, only_programs, any_programs,
# hosts, network_kinds, model_locality, destructive, secret_names, consumers
```

Only one `global` document may carry `[network]` (`allow_domains`, `local_network`, `arbitrary_ip`).

## Project rules (spec §55)

`ProjectRules::load(<workspace>/.aidex/rules.toml)` (see `PROJECT_RULES_FILE`) reads `schema_version = 1`, an informational `[project]` table (`package_manager`, `tests`, `lint`, `build`), and `[agent]` with `never_modify` globs and `require_approval` action patterns. Project files are repository content, which is untrusted, so they can only tighten: they apply as floors no layer can loosen. Attach them with `PolicySet::set_project_rules(workspace_id, rules)`.

## Classifiers

`classify_shell` lexes POSIX, cmd.exe, and PowerShell syntax (Windows lines are classified with both cmd and PowerShell rules, keeping the stricter result), unpacks nested shells (`sh -c`, `cmd /c`, `powershell -Command`), wrappers (`sudo`, `env`, `xargs`, `find -exec`), substitutions, and pipelines, and returns risk (`low` < `medium` < `high` < `destructive`), reasons, network/write effects, implied actions, and destinations. Unknown programs are at least `medium`; obfuscation (decoded payloads piped to an interpreter, `eval`/`iex`, encoded PowerShell, runtime-computed program names, unbalanced quoting) raises risk. `classify_network` rejects non-canonical numeric hosts (`2130706433`, `0x7f.1`) rather than guessing.

`redact_command` removes values after secret options, `Authorization`/API-key headers, secret-named assignments and query parameters, URL user-info, and common token formats. It is a display/audit aid, not a guarantee.

## Fingerprints

`Intent::fingerprint` is SHA-256 over a domain-separated, length-prefixed encoding of every field and attribute (including the raw command line and classification). Any change yields a different fingerprint, so the broker can bind an approval to the exact intent and reject altered intents.

## Not implemented here

- **Enforcement.** Nothing in this crate stops an action. The broker must route every agent-originated action through `evaluate`, enforce the verdict and `sandbox` level, and fail closed when the sandbox cannot provide that level.
- **Approval lifecycle.** Pending requests, expiry clocks, cancellation races, revocation, session-scope reuse, and fingerprint checks belong to the broker/approval PR; this crate only supplies verdicts, scopes, expiry durations, and fingerprints.
- **Audit events.** Decisions are return values; the broker emits redacted audit events.
- **Fact computation.** Path confinement, symlink resolution, and `sensitive`/`protected` classification come from the workspace service and broker. Destinations are classified before DNS resolution; the broker must re-check resolved addresses (DNS rebinding).
- **Heuristic classification.** The shell classifier is not a shell parser and will miss some obfuscation; risk is conservative, and the sandbox must contain what it misses. Project `never_modify` rules see only `fs.*` intents, not paths a shell command writes.
- **Policy authoring.** Writing policy files must itself be gated as `policy.change` by whatever edits them; loading does not authenticate files.
- Wiring: `aidex-broker` enforces decisions, owns approvals and audit events, and serves `policy.*` through the daemon (`aidex policy show|profile|reload|explain`).
