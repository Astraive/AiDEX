# Native ChatGPT OAuth

`aidex-auth` owns supported Sign in with ChatGPT for AiDEX. A host without a supported identity source can start Core normally; sign-in returns unavailable before network discovery rather than preventing daemon startup. It does not read
existing Codex, ChatGPT, or browser credentials and does not create `auth.json`.
The daemon constructs one `Arc<AuthService>` and shares it between RPC and its
provider credential resolver. `AuthService::new(Arc<VaultService>)` denies network
access by default. Production composition uses `with_network_gate` and the shared
offline switch. Constructors support a locked vault; sign-in and resolution
require unlock. Every auth HTTP request and browser open checks the gate.
Offline disconnect still clears local tokens and reports unconfirmed revocation.

`start(provider_id, connected)` binds an ephemeral `127.0.0.1` listener before
returning its authorization URL. The callback validates state, registration,
PKCE, RS256 JWKS signature, issuer, issued-client audience, nonce, expiry, and
plan scopes. Issued client registration is encrypted before code exchange.
The `connected` closure registers the native provider after verification. It
must not synchronously resolve this account's credentials: the account commit
holds its refresh mutex. Catalog discovery runs separately in the RPC adapter.
Failed activation restores the prior active credentials.

Provider credentials are stored as `oauth.<provider>.access`, released through
`provider:<provider>` vault handles. `oauth.<provider>.profile` holds encrypted
registration, verified identity, scope/expiry, and renewable credentials under
`process:aidex-auth`. The profile is the authoritative atomic token record;
the provider token is reconciled from it before resolution after an interrupted
write. Token structures and decoded HTTP buffers zeroize on drop. Token
structures have no `Debug` implementation. RPC results contain safe account/status
metadata and an authorization URL without an ID-token hint.

The encrypted `oauth.hosts` mapping associates hashed host facts with a random
UUIDv4 URI. Windows uses MachineGuid and Linux uses `/etc/machine-id` only as
local mapping keys. Only the opaque UUID URI is sent to OpenAI. Moving the
portable drive to a different host preserves registrations and receives that
host's own ID.

Browser opening is one shared helper, `open_in_browser(url, network_allowed)`.
It checks the network gate first, then accepts only an absolute `https://` URL
with a host and no embedded credentials, whitespace, or control characters
(`validate_browser_url`). On Windows it runs the absolute system PowerShell
binary with fixed command text (`Start-Process -FilePath $env:AIDEX_OPEN_URL`)
and passes the URL as environment data, so the URL never becomes command text.
Other platforms report unavailable. `AuthService::open` uses it only for the
exact URL of a pending sign-in flow; the daemon's `provider.open_link` uses it
only for links stored in the built-in provider presets. Neither accepts an
arbitrary URL from a client.

Resolution serializes refresh per provider in the daemon, obeys expiry and
`earliest_refresh_at`, preserves an existing refresh token when a replacement
is omitted, and persists rotations together. Revoked tokens or lost plan
consent clear credentials and require sign-in. Disconnect cancels owned flows,
attempts renewable-session revocation with bounded retries, clears local tokens,
and retains the account registration. Unconfirmed remote revocation is reported.

The daemon must retain its single-instance ownership of the vault. This service
does not provide a cross-process token-refresh lock for independently embedded
consumers.

Protocol sources:

- [Official registration and sign-in](https://developers.openai.com/siwc/token-sharing-open-source/sign-in)
- [Official accounts, refresh, and revocation](https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions)
- [Official token reference](https://developers.openai.com/siwc/token-sharing-open-source/token-reference)
- T3 code reviewed at `98beed1a226c42b85c52ff5ee9d5dbb67d776a77`,
  `CodexChatGptAuth.ts`, `CodexManagedRuntime.ts`, and `CodexManagedHome.ts`.
  This is an independent Rust implementation; no T3 implementation was copied.

`cargo test -p aidex-auth` runs local HTTP/JWKS fixtures with RSA keys generated
at runtime. No signing private key files are committed. Live sign-in, account
eligibility, model discovery, and inference still require user verification.

Account/workspace recovery: registrations remain tied to their issued OAuth client and workspace. Use a new provider instance to register another account/workspace; do not replace a saved client ID or relax identity validation. The desktop's “Connect another account or workspace” creates a separate instance, while Reconnect retains the selected instance's registration. Failed/new flows preserve existing accounts and the shared stable host identifier. Fixtures verify a new instance sends dynamic_agent_client while an earlier incomplete registration remains encrypted and unchanged.
