# External runtime adapters

`aidex-adapters` runs external agent CLIs (Codex, Claude Code, Cursor, Grok, OpenCode, Antigravity, Pi, Muse, OMP and a generic ACP command) as `RuntimeExecutor`s for the native engine. The vendor owns its tool loop; AiDEX owns the process tree, the credentials it hands over, approval mediation where the protocol has one, and the encrypted state the vendor leaves in its home.

The daemon owns one `Arc<RuntimeManager>` (`Services::runtime()`) and keeps it across RPCs, so login processes and cancellation refer to the same jobs.

## Configuration and state

- `RuntimeConfig` is literal: an executable, bounded credential-free argv, an optional managed profile id and a `RuntimeAuth` (`none`, `vault {secret_name, env_name}` or `codex_oauth {provider_id}`). Credentials never enter it. The id `native` is reserved for the built-in loop.
- Everything the crate stores lives in `<drive>/private/` and is sealed with XChaCha20-Poly1305 under a key held in the vault as `external-runtime-encryption-key` (`RUNTIME_STATE_KEY_SECRET`, a record AiDEX manages and clients must not replace or delete). A locked vault never reads as an empty configuration.
  - `runtime-config.sealed`: the configuration list (at most 64 runtimes, 64 KiB).
  - `profile-<digest(profile)>.sealed`: a managed account's credential.
  - `account-<digest(profile)>.sealed`: that account's revision.
  - `runtime-<digest(runtime)>-<digest(session:branch)>.sealed`: a native session snapshot.
- `has_saved_configuration()` reads file metadata only, so it works while the vault is locked and never creates the key.

## Process lifecycle

- Every launch gets an app-owned temporary home (`CODEX_HOME`, `CLAUDE_CONFIG_DIR`, `XDG_*` and the other vendor variables point there) inside a brokered sandbox with the workspace mounted read-write. Codex always runs with `cli_auth_credentials_store="file"`, so it never reaches the host keyring.
- The manager's network gate is the daemon's closure. It closes when the daemon goes offline, the vault locks or an emergency stop begins. A closed gate refuses launches and logins (`offline_refused`), and a watcher thread kills a running vendor tree within about 25 ms, also while a run waits for approval. The run's cancellation token does the same.
- `cancel_logins()` cancels every pending official login and leaves the manager usable, so `emergency.resume` and unlock need no restart. `has_pending_logins()` tells a lock watcher whether it has anything to stop.
- `shutdown()` is for daemon stop: it cancels logins, terminates the runtime process registry and closes it, so no runtime process starts afterwards.

## Managed accounts (Codex)

Only Codex's official account commands and portable `auth.json` format are verified. Its catalog entry lists the auth methods `native_cli` and `profile_import` (besides `codex_oauth` and `vault`); every other driver refuses account calls with `auth_unsupported`.

| Manager method | RPC | Result |
| --- | --- | --- |
| `account_status(id)` | `runtime.account.status` | `RuntimeAccountStatus` |
| `import_profile(id, source)` | `runtime.profile.import` | `RuntimeAccountStatus` |
| `login_start(id)` | `runtime.login.start` | `RuntimeLoginStatus` |
| `login_status(id, flow_id)` | `runtime.login.status` | `RuntimeLoginStatus` |
| `login_cancel(id, flow_id)` | `runtime.login.cancel` | `RuntimeLoginStatus` |
| `logout(id)` | `runtime.login.logout` | `RuntimeAccountStatus` |

`RuntimeAccountStatus = {id, driver, profile, authenticated, credential_present, evidence, status}`; `status` is `authenticated_cached`, `auth_required`, `auth_pending` or `signed_out`, and `evidence` a stable code (`native_credential_and_cli_exit_status`, `native_credential_missing`, `official_cli_login_pending`, `native_cli_logout_and_profile_removed`, `managed_profile_removed_cli_unconfirmed`). `RuntimeLoginStatus = {id, flow_id, state, profile, authenticated, authorization_url, message, error_code}` with `state` `pending`, `connected`, `cancelled` or `failed`. Neither ever carries credential bytes or vendor output.

- **Login.** `login_start` runs the configured CLI's `login` in an app-owned temporary `CODEX_HOME`; the CLI opens the browser and AiDEX supplies no OAuth client. A worker thread owns the process and its cleanup even if nobody polls. At most 64 KiB of output is kept, only to find an `https://auth.openai.com/oauth/authorize?` URL (never one carrying a code or token), and is cleared when the flow ends. The saved configuration is not touched while the login is pending, fails or is cancelled, so the runtime keeps its previous authentication. Only once the CLI exits successfully and its `login status` confirms the credential does the worker encrypt `auth.json`, start a new account revision and switch the runtime to its profile (`profile` set, `auth: none`). That confirmation runs without holding the flow, so status polls and cancellation never wait for it; a flow cancelled meanwhile saves nothing. A closed gate cancels a pending flow. A cancelled or failed flow always clears the runtime's pending state, also when its process tree could not be confirmed terminated (`process_shutdown_failed`, retried by the next cancel). One login per runtime and eight in total can be pending.
- **Import.** `import_profile` takes an explicit absolute `auth.json`, or its directory, of at most 1 MiB that is a regular file and not a link or reparse point. Only that file is read; nothing host-global is discovered. The official CLI must confirm it before it is encrypted and the runtime switches to the profile.
- **Status.** `authenticated` needs both a credential file and a successful official `login status` exit: cached-credential evidence, not proof of inference entitlement. The result is cached for 60 seconds per runtime, keyed by the CLI, the profile and its revision, so health checks do not start a process each time. `health` asks for it only when the binary is installed (`binary_missing` otherwise), and an account status that cannot be read leaves the runtime unverified instead of failing the check.
- **Logout.** Runs the CLI's `logout` on the restored credential, then removes the encrypted profile and every session snapshot of the runtime, so no session restores the old credential. The runtime stays in profile mode and is signed out.
- **Using the account.** In profile mode the profile record is the only credential and it outranks any credential inside a session snapshot (session snapshots of a profile runtime leave `auth.json` out). A signed-out profile fails `auth_required` before a process starts, and a pending login refuses turns with `auth_pending`. The restored credential's values are scrubbed from everything the vendor sends back. A credential the CLI refreshed is saved again (at an approval request, at completion, when a turn or launch fails, and after a model-list probe), unless the account revision changed or the account was signed out meanwhile.
- **Revisions.** A successful import or login starts a new account revision and removes the runtime's session snapshots. A token refresh keeps the revision.

## Native continuation

- **Checkpoint.** Before a turn can produce effects the adapter snapshots the vendor home and reports `RuntimeEvent::SessionStarted { runtime_ref }`: Codex, Muse and ACP once the native session or thread id is known and before the turn is sent, Claude Code and Cursor on the first root `session_id`. A crash mid-turn therefore still leaves a reference to resume. A callback that returns `Pause` refuses the turn. Pi and OMP have no native restore, so they take no snapshots and report no checkpoint; their references are never resumable.
- **Reference.** `runtime_ref` carries `runtime_id`, `session_id`, `run_id`, `native_session_id`, `native_turn_id`, `native_branch_id`, `snapshot_id`, `snapshot_digest` (of the snapshot plaintext), `account_revision`, `config_digest` (driver, profile and auth configuration) and `credential_binding`; an approval request's reference adds `native_request_id` and `approval_digest`. The snapshot is refreshed at every approval request and at completion.
- **Redaction-stable.** The harness keeps a reference only when `aidex_events::redact_text` leaves it unchanged, so every field is value-free metadata and no key with a string value names a credential (`secret`, `token`, `key`, `auth`, `credential`, `password`). Tests assert this for every driver and credential kind at each point a reference is emitted.
- **Branches.** The branch is the run that started the native session, so a root conversation and each child run have independent snapshots, and follow-ups of a branch keep its snapshot name.
- **Resume.** `RuntimeOperation::Resume` continues only a reference for which `is_resumable_reference` holds. Every evidence failure is refused with `resume_unavailable` before any process starts: no bound branch, any field that does not match this runtime, session, account revision, configuration digest or credential binding, and a snapshot that is missing, changed or cannot be restored. The harness then starts a new native session with the transcript for a follow-up. Pi and OMP have no native restore (`supports_resume` is false).
- **Recovery.** Resuming the run the reference was saved by sends an inspection-only continuation: the vendor is told to inspect its history and the workspace, the original request is quoted as evidence only, and it is never sent again. A follow-up run sends only its new message.
- **Credential binding.** The binding is value-free. In the reference it is `{"kind":"none"}`, `{"kind":"vault","record":<digest of the vault record id>,"revision":<n>}` for a vault credential's `SecretVersion`, or `{"kind":"codex_oauth","account":<hash>}` for the shared OAuth account's `AuthService::account_binding` hash of verified issuer, subject and client. It is captured at launch, the vault credential is resolved only at that revision, and the binding is checked again right before the process starts and before an approval decision is sent. Token refresh keeps the OAuth binding; signing in to another account or replacing the vault secret changes it.

## Snapshots

- Session snapshots use the `base64_v1` archive (`{"format":"base64_v1","files":{path: base64}}`): at most 4 MiB of raw files, 4096 files and 32 directory levels, inside the 8 MiB encrypted-record cap. Links and Windows reparse points (junctions included) anywhere in the home are refused.
- SQLite databases are copied with the backup API, so a database in WAL mode is captured coherently with its committed transactions; its `-wal`, `-shm` and `-journal` sidecars are rebuilt rather than copied. A database that stays locked for 3 seconds fails the capture (`state_busy`) and nothing is published.
- Codex's top-level `.tmp` (regenerated argv0 tool copies, including Windows byte-range locked files) is left out. Other drivers have no implicit exclusions.
- A snapshot that cannot be saved (`state_limit`, `state_busy`, `state_unavailable`, `state_corrupt`, `vault_locked`) never fails a turn whose vendor work already ran. The turn completes or pauses with a reference that is not resumable (`snapshot_digest` and `native_branch_id` null, `snapshot_error` set to that code) and the stale snapshot is removed; the next follow-up then starts with the transcript. At the first checkpoint the turn continues without one.
- Account profile records keep the legacy archive (a JSON map of path to byte array) and hold only `auth.json`.

## Protocols

- Pi and OMP model ids are provider-qualified (`provider/model`), the form `begin` selects, so discovered models can be chosen.
- A Claude Code or Cursor subagent's result, failed or not, never ends or fails the root turn, and a root `session_id` that changes within a launch is a `protocol_error`.
- Vendor frames are scrubbed of every credential handed to the process, in string values, object keys and numeric or boolean reflections. Approval arguments are redacted again before they reach callers.
- A process that ends early reports `process_closed` with its exit code, stderr byte count, a truncation flag and a closed stderr category (`assertion_failed`, `dependency_missing`, `access_denied`, `syntax_error`, `sandbox_helper_failed`, `vendor_output_withheld`, `none`), never vendor output.
- JSON-RPC `-32602` maps to `protocol_parameters`; messages mentioning auth or login to `auth_required`.

## Installer

`install(driver, source)` places only a component the drive's `runtime-components.json` declares. Every path component of the source and of `runtimes/<driver>/` is checked for links and reparse points, so a planted junction cannot redirect the write outside the drive. The artifact (at most 256 MiB) is hashed while it is copied into a temporary file in the destination directory, and only a matching digest is synced and published with a no-clobber rename.

## Rollback

- A daemon from before `base64_v1` cannot decode the new session snapshots, and it does not know the branch-scoped names. After a rollback its resume of those sessions fails closed; their conversations continue with a transcript. The snapshot an older daemon kept under `runtime-<digest(id:session)>.sealed` is removed after the first new snapshot of that session; nothing resumes from it any more.
- Profile records stay in the legacy encoding, so an older daemon still restores a managed account. It ignores the account revision record.
- References an older daemon saved have no `native_branch_id` or `snapshot_digest`, so they are never resumed; those conversations continue with a transcript.

## Tests

`tests/process_protocols.rs` drives real processes: `tests/peer.py` speaks every protocol (scenarios select approvals, crashes, oversized native history, credential reflections and more, and every start is recorded in `launches.txt`), and `tests/account_peer.py` behaves like the official account commands without network or real credentials. Set `AIDEX_TEST_PYTHON` to the Python interpreter. The sandbox helper `aidex-sandbox-exec` must be built in the same target directory. The probes against an installed Codex are `#[ignore]`d and need `AIDEX_TEST_CODEX`; they never log in or start a model turn.
