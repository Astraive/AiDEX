# Managed runtime accounts

The daemon owns one `Arc<RuntimeManager>` through `Services::runtime()`. It must retain
this manager across RPCs so login processes and cancellation refer to the same job.

| RPC                      | Parameters                      | Result                 |
| ------------------------ | ------------------------------- | ---------------------- |
| `runtime.profile.import` | `{id: string, source: string}`  | `RuntimeAccountStatus` |
| `runtime.login.start`    | `{id: string}`                  | `RuntimeLoginStatus`   |
| `runtime.login.status`   | `{id: string, flow_id: string}` | `RuntimeLoginStatus`   |
| `runtime.login.cancel`   | `{id: string, flow_id: string}` | `RuntimeLoginStatus`   |
| `runtime.login.logout`   | `{id: string}`                  | `RuntimeAccountStatus` |
| `runtime.account.status` | `{id: string}`                  | `RuntimeAccountStatus` |

`RuntimeAccountStatus = {id, driver, profile: string|null, authenticated: boolean,
credential_present: boolean, evidence: string, status: string}`.

`RuntimeLoginStatus = {id, flow_id, state: "pending"|"connected"|"cancelled"|"failed",
profile: string, authenticated: boolean, authorization_url: string|null,
message: string, error_code: string|null}`.

Codex uses the configured installed CLI's official `login` command with an isolated
`CODEX_HOME` and `cli_auth_credentials_store="file"`. The CLI opens the user's browser.
AiDEX does not supply or replace an OAuth client. `source` is an explicit absolute
`auth.json` file or its containing directory; only that credential file is imported.
No host-global credentials are discovered or imported automatically.

Credential data lives only in an application-owned temporary home during a command.
Portable profiles are encrypted with the vault-backed runtime key on the drive.
Cancel and manager shutdown terminate the owned process tree before deleting that
temporary home. Logout removes the encrypted account profile and every encrypted
session snapshot belonging to that runtime so sessions cannot restore old credentials.
Official `login status` process success plus native credential-file evidence are both
required to report authentication. This is cached-credential evidence, not a claim
that the account has inference entitlement. Other drivers return `auth_unsupported`
until their official account commands and portable credential format are verified.

Explicit import and successful browser login rotate an encrypted account revision and
invalidate that runtime's old snapshots. Resume references bind this revision and the
driver/profile/auth configuration; a session belonging to a previous account is refused.
Native credential refresh preserves the account revision. An in-flight login refuses
native model turns until it completes or is cancelled.
Same-run interrupted recovery sends an explicit inspection/continuation request, with
the original user task quoted as intent evidence. It never blindly reissues that task
to reconstruct native context. A new follow-up run sends the user's new prompt.
Ambiguous prior effects stay explicit and are not automatically retried.

Vault API-key recovery also binds the vault record identity and revision; a changed
credential refuses recovery before any vendor process launches. Shared Codex OAuth
recovery binds `AuthService::account_binding`, a backend-only hash of the verified
encrypted issuer, subject and client identity. Token refresh preserves that identity;
signing into a different account invalidates the old reference. Missing verified
identity fails closed. Neither token values nor the bridge credential are resume fields.

`RuntimeManager::cancel_logins()` cancels browser login jobs before vault lock.
`RuntimeManager::shutdown()` also closes and terminates the native process registry.
`open_with_private(drive_root, private_root, vault, auth, sandbox, network_gate)` lets
daemon composition use the manifest's config directory. The old drive `private/`
location is migrated once by copying only bounded named runtime ciphertext; unrelated
vault files are excluded and original ciphertext is retained.

Process errors expose exit codes, bounded output counts and closed diagnostic
categories. Raw credential-bearing stdout/stderr is never returned or logged.

## Scoped MCP bridge

`RuntimeRequest.bridge` is an optional backend-only capability, skipped by Serde and
redacted in Debug. Only Codex and Claude Code currently implement native MCP bridge
injection. `RuntimeExecutor::supports_mcp_bridge()` is authoritative; `DriverInfo`
also exposes `supports_mcp_bridge` for the frontend. Other drivers reject a supplied
bridge with `bridge_unsupported`, and their catalog explains this limitation.

Codex receives the bridge URL and `env_http_headers.Authorization` environment-variable
reference through app-server CLI configuration, with `required=true`. Its shell
environment excludes the bridge credential and provider keys. Claude receives a
temporary `--mcp-config` JSON file containing an environment-variable header reference,
plus `--strict-mcp-config` and `timeout: 180000`; the file is outside the native home collected for recovery.
The [official Claude MCP documentation](https://code.claude.com/docs/en/mcp), retrieved
2026-10-11 at lines 499–504, specifies the per-server `timeout` field in milliseconds
and its precedence over `MCP_TOOL_TIMEOUT`. Codex uses `tool_timeout_sec=180`.
Both use a fresh run-scoped authorization supplied only through the owned process
environment. Tokens are absent from command arguments, frontend DTOs and diagnostics.
Vendor output scrubbing includes known-secret object keys and non-string reflections.

Snapshots exclude our temporary bridge configuration. If the vendor copies the bridge
credential into its native recovery files, snapshot publication fails with
`bridge_state_unsupported` rather than saving or restoring an ephemeral capability.
Public routing URLs may remain as vendor history metadata; startup/resume configuration
always overrides the bridge URL and authorization with the current run's capability.
Codex snapshots explicitly omit its top-level `.tmp` regenerated argv0/tool artifacts,
which include large executable copies and Windows byte-range lock files. Session
rollouts and database content, including committed WAL transactions, remain included; other drivers receive no
implicit directory exclusions. The installed Codex configuration probe verifies
authenticated bridge startup and an actual encrypted native-home snapshot without
starting a model turn.
SQLite data uses a bounded coherent backup, including committed WAL content; its
transient WAL/shared-memory sidecars are rebuilt on restore rather than copied.
Archives use compact base64 encoding within the existing 8 MiB encrypted-record cap;
legacy byte-array archives still restore. Busy backups time out without publishing
partial recovery data. Limits are 4 MiB of aggregate raw native data, 4096 files,
depth 32 and a three-second SQLite backup deadline; the encrypted record cap is 8 MiB.
The harness owns bridge lease revocation on completion, cancellation and shutdown;
adapter temporary homes are cleaned after their owned process ends. No OAuth,
bidirectional sampling, or generic ACP/Pi bridge support is implied by this integration.

Verified configuration references:

- [Codex configuration reference](https://developers.openai.com/codex/config-reference): HTTP MCP URL, environment headers and required startup.
- [Claude MCP configuration](https://code.claude.com/docs/en/mcp): HTTP transport and environment expansion in headers.
- [Claude CLI reference](https://code.claude.com/docs/en/cli-reference): `--mcp-config` and `--strict-mcp-config`.

The design was compared with T3 Code revision `98beed1a226c42b85c52ff5ee9d5dbb67d776a77`.
No T3 source was copied; its MIT notice must accompany any later source reuse.

Official command references:

- https://developers.openai.com/codex/cli/reference/
- https://developers.openai.com/codex/auth/
