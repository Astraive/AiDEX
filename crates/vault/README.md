# `aidex-vault`

The portable, passphrase-encrypted AiDEX secret vault and its scoped secret
handles (spec §15, §92). It works without any host keychain: copy the drive to
another machine and unlock it with the passphrase or the recovery key.

> **Independent security review pending before release.** See
> [`FORMAT.md`](FORMAT.md) for the on-disk format, key hierarchy, validation order,
> crash-safety protocol, brute-force considerations, and threat model.

## Design in one paragraph

passphrase → Argon2id (default 64 MiB, t=3, p=1; stored per vault) → KEK → unwraps a
random 256-bit vault master key (VMK). An optional recovery key (256 random bits,
shown once as Crockford base32 groups) wraps the same VMK. HKDF-SHA256 derives a MAC
key, an index key, and one key per record. Secret names, kinds, policies, and
timestamps live in an encrypted index inside `vault.json`; each value is a separate
XChaCha20-Poly1305 record whose associated data binds vault id, record id, revision,
and file name. All writes are temp file + fsync + rename (+ directory fsync on Unix).

## API

```rust
use aidex_vault::*;

// Create (recovery slot opt-in) — dir is DrivePaths::vault() in the daemon.
let (mut vault, recovery): (UnlockedVault, Option<RecoveryKey>) =
    Vault::create(dir, &SecretString::from(passphrase), CreateOptions { kdf: KdfParams::default(), recovery_key: true })?;
Vault::exists(dir) -> bool;
let locked: LockedVault = Vault::open(dir)?;            // header parsed + validated, not unlocked
let vault = locked.unlock(&passphrase)?;                // or locked.unlock_with_recovery(&RecoveryKey::parse(text)?)

// Secrets: metadata only, never values.
let meta: SecretMeta = vault.put(&SecretName::new("openai")?, SecretKind::ApiKey,
    SecretBytes::from(value), SecretPolicy::new([ConsumerScope::parse("provider:openai")?])?)?;
vault.list() -> Vec<SecretMeta>;   vault.meta(&name)?;   vault.delete(&name)?;
vault.change_passphrase(&old, &new)?;   vault.reset_passphrase(&new)?; // reset: recovery unlock only

// Scoped handles (broker only).
let handle: SecretHandle = vault.issue_handle(&name, &consumer, Duration::from_secs(60))?;
let value: SecretBytes = vault.resolve(&handle, &presenting_consumer)?;
vault.revoke_handle(&handle.handle_id);   vault.revoke_all_handles();

// Revision binding (value-free): resolve only while the bound revision is current.
let version: SecretVersion = vault.secret_version(&name)?;
let value: SecretBytes = vault.resolve_versioned(&name, &consumer, &version)?;

// Backup / restore (still encrypted).
vault.backup_to(dest)?;   Vault::restore_from(src, target)?;   vault.verify()?;
let locked = vault.lock();                                   // keys zeroized, handles gone
```

`VaultService` is the thread-safe, `Arc`-shareable wrapper the daemon owns
(`aidexd::Services::vault()`): `open(dir, VaultServiceConfig { kdf, idle_timeout })`,
`status() -> VaultStatus`, `init`, `unlock`, `unlock_with_recovery`, `lock`,
`put_secret`, `list_secrets`, `secret_meta`, `secret_version`, `delete_secret`,
`change_passphrase(old: Option<_>, new)`, `issue_handle`, `resolve`,
`resolve_versioned`, `revoke_handle`, `revoke_all_handles`, `backup_to`,
`set_kdf_params`, `set_idle_timeout`. It locks itself after 30 minutes
without secret operations (configurable), re-reads the disk while locked (so damage or
an external restore is reported), and drops its session on a `Conflict` or an
ambiguous post-rename commit to its own header.

Types:

- `ConsumerScope` — `provider:<id>`, `mcp:<server-id>`, `process:<tool-id>`,
  `git:<host[:port]>`; lowercase, exact-match. `model:`, `prompt:`, `agent:`,
  `session:`, `context:`, `memory:`, `log:`, `event:`, `env:`, `shell:` (and plurals)
  fail with `ForbiddenConsumer`.
- `SecretHandle { handle_id: EntityId, secret_ref: aidex_core::SecretRef, consumer,
  expires_at: Timestamp }` — `Serialize`/`Deserialize`, no secret bytes, TTL 1 ms to 24 h.
- `SecretBytes`, `SecretString`, `RecoveryKey` — zeroizing, `[REDACTED]` formatting,
  not `Clone`, not `Serialize`; read with `expose()` (`RecoveryKey::encode()` returns
  the display text as a `SecretString`).
- `SecretVersion { secret_ref, revision }` — which record and value revision is
  current; no name or value, never stored by the vault. Replacing a value raises
  `revision`; deleting and re-creating a secret changes `secret_ref`. Callers
  that bind an approval or a resume to a credential keep it and later call
  `resolve_versioned`, which checks it, then issues, resolves and revokes a
  short-lived handle under one lock. A changed version is denied with
  `HandleDenial::RevisionChanged` (`secret_revision_changed`) and leaves no handle.
- `VaultError` — `kind() -> VaultErrorKind` (stable snake_case `as_str()`), `code() ->
  aidex_core::ServiceErrorCode`; handle denials carry a `HandleDenial` reason.

## Guarantees

- Values, names, kinds, consumers, and per-secret timestamps are encrypted at rest; a
  grep of the vault directory finds none of them (tested).
- Wrong passphrase, tampered/truncated/missing/swapped files, unknown versions, and
  links are explicit errors; nothing falls back to plaintext or to an empty vault.
- A failure before header rename leaves the previous committed state readable.
  A directory-sync failure after rename returns `CommitUncertain`: direct callers
  reconcile the visible header/index and revoke handles; the service invalidates
  its session. Ordinary pre-rename errors and backup-destination errors do not
  invalidate unchanged source sessions. Newly referenced records are preserved.
- No API returns a secret value except handle resolution for an allowed consumer.
- Revoked and expired handles are pruned before capacity checks, so revocation
  immediately makes room for replacements without extending an old handle's life.
- Unauthenticated KDF parameters are capped at 64 MiB, three Argon2id iterations,
  and four lanes. Headers above that budget are rejected before key derivation,
  including vaults created with previously accepted oversized parameters.
- Restore accepts at most 4096 well-named record files and rejects excess records
  before collecting references or copying files into the destination.

## Limitations

- Independent security review pending; do not treat as production-grade yet.
- No memory locking; IPC buffers carrying passphrases/values are not zeroized.
- Whole-vault rollback to an older complete copy is undetectable offline.
- No Unicode normalization of passphrases.
- Single writer per vault (the daemon); weaker crash durability on Windows and
  FAT/exFAT (see `FORMAT.md`).
- No audit events yet (the crate must not depend on `aidex-events` in this wave);
  no recovery-key rotation, VMK rotation, phone factor, or host-trust policy.
