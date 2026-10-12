# AiDEX vault format (version 1) and threat model

> **Status: independent security review pending before release.** The design uses
> only reviewed RustCrypto primitives, but the composition, this document, and the
> implementation have not been reviewed by an independent party. Treat the vault as
> pre-release until that review is complete (issue #13 acceptance criterion).

This document is normative for `aidex-vault` format version 1. Any change to a byte
layout, label, or cryptographic validation rule requires a new format version.
Operational resource-admission budgets may tighten without changing the byte format;
an otherwise well-formed header exceeding the current KDF budget is refused.

## Directory layout

The vault lives in the drive directory declared as `private_vault` in
`aidex-drive.json` (`private/vault/` by default; `DrivePaths::vault()`).

```text
private/vault/
  vault.json              header: key slots, sealed secret index, header MAC
  records/
    <blob>.json           one sealed secret value per file
  .<name>.<hex>.tmp       transient write files (ignored, removed on unlock/commit)
```

`<blob>` is 32 lowercase hex characters from 128 bits of OS randomness. File names
carry no secret name, record id, or timestamp. Files that do not match these
patterns are ignored and never deleted.

## Key hierarchy

```text
passphrase ──Argon2id(salt_p, m, t, p)──► KEK_p ─┐
                                                 ├─ XChaCha20-Poly1305 unwraps ─► VMK (256-bit, random)
recovery key ──HKDF-SHA256(salt_r)──────► KEK_r ─┘                                   │
                                                                                     ├─ HKDF ─► MAC key   (header HMAC)
                                                                                     ├─ HKDF ─► index key (sealed index)
                                                                                     └─ HKDF ─► record key per record id
```

- The **vault master key (VMK)** is 32 bytes from the OS RNG, created once. Every
  key slot wraps the same VMK, so changing the passphrase rewraps one slot and
  re-encrypts nothing else.
- **Passphrase slot** (exactly one): `KEK_p = Argon2id(version 0x13, m, t, p,
  password = UTF-8 bytes of the passphrase, salt = 32 random bytes, output 32 bytes)`.
- **Recovery slot** (optional): `KEK_r = HKDF-SHA256(salt = 32 random bytes,
  IKM = 32-byte recovery key, info = "aidex/vault/kek/recovery/v1", L = 32)`. The
  recovery key is 256 bits of OS randomness, so no memory-hard KDF is needed.
- **Subkeys**: `HKDF-SHA256(salt = none, IKM = VMK, info, L = 32)` with
  - `info = "aidex/vault/header-mac/v1"` → MAC key,
  - `info = "aidex/vault/index/v1"` → index key,
  - `info = "aidex/vault/record/v1" || record_id` (UTF-8) → record key.

## Primitives

| Purpose | Primitive | Crate |
| --- | --- | --- |
| Passphrase KDF | Argon2id v0x13 (RFC 9106) | `argon2` 0.5 (`zeroize` feature: memory blocks are wiped) |
| Key wrapping, index, records | XChaCha20-Poly1305, random 192-bit nonce per seal | `chacha20poly1305` 0.10 |
| Subkeys, recovery KEK | HKDF-SHA256 (RFC 5869) | `hkdf` 0.12, `sha2` 0.10 |
| Header integrity | HMAC-SHA256, constant-time verify | `hmac` 0.12 |
| Comparisons | constant time | `subtle` 2 |
| Randomness | OS RNG; failure is an error, never a fallback | `getrandom` 0.3 |
| Wiping | `Zeroizing` buffers | `zeroize` 1 |

Random 192-bit nonces make nonce reuse under one key negligible without counters.

## Canonical transcript encoding

Associated data and the MAC input use an unambiguous, length-prefixed encoding:

- `bytes(x)` = `u64_le(len(x)) || x`; `str(x)` = `bytes(UTF-8(x))`
- `u32(n)` = 4 bytes little-endian; `u64(n)` = 8 bytes little-endian
- `T(label)` starts a transcript with `str(label)`.
- `KDF(slot)` = `str("argon2id") u32(19) u32(m) u32(t) u32(p)` for a passphrase slot,
  `str("hkdf-sha256")` for a recovery slot.

| Use | Transcript |
| --- | --- |
| Slot wrap AAD | `T("aidex/vault/slot/v1") u32(1) str(vault_id) str(kind) KDF(slot) bytes(salt)` |
| Index AAD | `T("aidex/vault/index/v1") u32(1) str(vault_id) u64(generation)` |
| Record AAD | `T("aidex/vault/record/v1") u32(1) str(vault_id) str(record_id) u64(revision) str(blob)` |
| Header MAC input | see below |

Header MAC input:

```text
T("aidex/vault/header-mac/v1") str("aidex-vault") u32(1) str(vault_id) str(created_at)
u64(slot_count)
for each slot (passphrase first, then recovery):
    str(kind) KDF(slot) bytes(salt) bytes(nonce) bytes(wrapped_key)
u64(index.generation) bytes(index.nonce) bytes(index.ciphertext)
```

The MAC covers every semantic header field except itself. JSON whitespace and key
order are not covered; they do not affect the parsed values, and duplicate or
unknown keys are rejected.

## Header: `vault.json`

Strict JSON (unknown fields rejected), base64 fields in canonical padded standard
alphabet:

```json
{
  "format": "aidex-vault",
  "format_version": 1,
  "vault_id": "vault_01K...",
  "created_at": "2026-10-10T12:00:00.000Z",
  "slots": [
    { "kind": "passphrase",
      "kdf": { "algorithm": "argon2id", "version": 19, "memory_kib": 65536, "iterations": 3, "parallelism": 1 },
      "salt": "<32 bytes>", "nonce": "<24 bytes>", "wrapped_key": "<48 bytes: VMK + tag>" },
    { "kind": "recovery",
      "kdf": { "algorithm": "hkdf-sha256" },
      "salt": "<32 bytes>", "nonce": "<24 bytes>", "wrapped_key": "<48 bytes>" }
  ],
  "index": { "generation": 7, "nonce": "<24 bytes>", "ciphertext": "<sealed index>" },
  "mac": "<32 bytes HMAC-SHA256>"
}
```

Plaintext header metadata reveals: the vault id, creation time, KDF parameters,
whether a recovery slot exists, the index generation (number of commits), and the
approximate index size. It reveals no secret names, kinds, consumers, or values.

### Sealed index

The index plaintext is JSON, sealed with the index key and the index AAD:

```json
{ "vault_id": "vault_01K...", "generation": 7, "records": [
  { "record_id": "sec_01K...", "name": "openai", "kind": "api_key",
    "created_at": "...", "updated_at": "...",
    "policy": { "allowed_consumers": ["provider:openai"] },
    "blob": "<32 hex>", "revision": 3 } ] }
```

After decryption, `vault_id` and `generation` must equal the header's, and names,
record ids, and blobs must be unique.

## Record files: `records/<blob>.json`

```json
{"format_version":1,"nonce":"<24 bytes>","ciphertext":"<sealed value>"}
```

The plaintext is `u32_le(len(value)) || value || zero padding` up to a multiple of
256 bytes, so exact value lengths are hidden within 256-byte buckets. It is sealed
with the record key and the record AAD; because the AAD binds the vault id, record
id, revision, and blob id, a record file cannot be swapped between names, replayed
from an older revision, moved to another file name, or transplanted from another
vault. Values are at most 64 KiB; `record_id` (the `SecretRef.secret_id`) is stable
across replacements while `revision` and `blob` change on every write.

## Validation order and errors

`Vault::open` (no key needed):

1. The vault directory and every managed path must be real directories/files, not
   links or junctions (`UnsafePath`). Reads are size-bounded (header 8 MiB, record
   256 KiB) before parsing.
2. No `vault.json`: `NotFound` if there are no record files, otherwise
   `MissingHeader`. A missing header is never treated as an empty vault, and
   `create` refuses a directory that still holds records.
3. `format` must be `aidex-vault`; an unknown `format_version` is
   `UnsupportedVersion` and is checked before strict parsing.
4. Strict structural parsing (`CorruptHeader`): one passphrase slot with Argon2id
   parameters inside the accepted bounds (8·p KiB ≤ m ≤ 64 MiB, 1 ≤ t ≤ 3,
   1 ≤ p ≤ 4), at most one HKDF recovery slot, exact salt/nonce/key/MAC lengths.
   These unauthenticated limits cap one unlock at the default memory/work budget;
   headers above them are refused before deriving a key, even if an older version
   accepted their parameters.

`unlock`:

5. Derive the KEK and open the slot. Failure is `WrongPassphrase` /
   `WrongRecoveryKey` (a damaged slot is indistinguishable from a wrong credential).
6. Verify the header MAC (`Integrity(Header)`), then open the index
   (`Integrity(Index)`).

Reading a record: missing or malformed file → `CorruptRecord`; unknown record
version → `UnsupportedVersion`; authentication failure → `Integrity(Record)`.
No error path ever returns partial data or falls back to plaintext.

## Write protocol and crash safety

Every file write is: write a new `.<name>.<random>.tmp` in the same directory
(`create_new`, mode 0600 on Unix) → `fsync` the file → `rename` over the target →
`fsync` the directory (Unix). The header is the single commit point.

- **create**: write `vault.json` (generation 1, empty index). A crash leaves at most
  a temp file; the directory still has no vault.
- **put**: seal the value into a *new* blob file → commit a header whose index
  references it (generation + 1) → delete the previous blob. A crash before the
  header rename leaves the old header, which still references the old blob; the new
  blob is an unreferenced orphan.
- **delete**: commit a header without the entry → delete the blob.
- **passphrase change / reset**: commit a header with a rewrapped passphrase slot.
- **garbage collection**: on unlock (and after commits) temp files and record files
  not referenced by the committed index are removed.
- **conflict detection**: before each commit the on-disk header is re-read and must
  match the SHA-256 of the state this session loaded or wrote; otherwise the write
  fails with `Conflict` and the daemon drops the session (unlock again).
- **ambiguous header commit**: a directory-sync failure after successful header
  rename returns `CommitUncertain` with the original I/O cause (`kind() == Io`,
  service code `Unavailable`). Direct callers reconcile the visible header and
  index and revoke handles; `VaultService` drops the session when its own header
  is affected, requiring fresh unlock before metadata or values can be read.
  Newly referenced records are preserved. Ordinary pre-rename I/O errors retain
  the old session; an uncertain backup commit does not invalidate its source.

Durability limits: Windows provides no directory flush through the Rust standard
library, and FAT/exFAT (common on removable drives) do not journal metadata, so a
power cut immediately after a commit can lose that commit, and the filesystem itself
may not guarantee rename atomicity under power loss. Keep backups. The vault assumes
a single writer (the drive's one daemon); garbage collection is not safe against a
second process writing the same vault concurrently.

## Passphrases and brute force

- Passphrases are 8 to 1024 bytes of UTF-8 without control characters, used as their
  exact bytes. **No Unicode normalization** is applied: the passphrase must be
  entered with the same code points on every host.
- Default Argon2id cost: **m = 64 MiB, t = 3, p = 1** (RFC 9106's second recommended
  profile uses 64 MiB, t = 3). Measured unlock time on the development machine
  (x86-64 desktop): about 90 ms optimized, about 1.6 s in a debug build. The cost is
  bounded by the slowest host the drive must unlock on; it is stored per vault, so
  later versions can raise it (with recalibration) without breaking old vaults.
- **There is no online lockout and none is possible**: an attacker who has the drive
  copies the files and guesses offline, with any hardware. Argon2id makes each guess
  cost 64 MiB of memory and three passes, which limits parallel guessing, but the
  passphrase's entropy is the real protection. A passphrase of five or more random
  words is recommended; an 8-character password is guessable regardless of KDF cost.
- Tests use tiny parameters (`KdfParams::new(8, 1, 1)`); production code paths use
  the default, and the daemon does not expose KDF parameters over IPC.

## Recovery key

`32 key bytes || first 3 bytes of SHA-256("aidex/vault/recovery-key-check/v1" || key)`
= 35 bytes, Crockford base32 (`0123456789ABCDEFGHJKMNPQRSTVWXYZ`, most significant bit
first) = 56 characters, shown as 14 groups of 4 separated by `-`. Parsing ignores
case, `-`, and whitespace and accepts `O`→`0`, `I`/`L`→`1`; the 24-bit checksum
reports typos as `InvalidRecoveryKey` instead of a wrong key. The key is returned
exactly once by `create`/`vault.init` and never stored. A recovery-key unlock may set
a new passphrase without the old one (`reset_passphrase`).

## Backup and recovery semantics

- `UnlockedVault::backup_to(dest)` copies the committed `vault.json` byte-for-byte and
  every record it references (records first, header last), into a directory that
  must not already hold vault data. It then re-reads the copy and verifies the header
  MAC, the index, and every record with the session keys. A failed backup removes
  what it wrote.
- `Vault::restore_from(src, target)` refuses a target that holds vault data, copies
  the header and all well-formed record files, compares them byte-for-byte, and
  returns the restored vault locked. Cryptographic verification needs a credential:
  unlock it and call `UnlockedVault::verify()`.
  Enumeration rejects more than 4096 well-named records before constructing the
  reference collection or copying any record, bounding damaged-backup restores.
- A backup is protected exactly like the live vault: the same passphrase and the
  same recovery key unlock it, and changing the passphrase later does not change
  the backup's slot (the old passphrase keeps opening the old backup).
- **A lost passphrase without a recovery key is unrecoverable.** There is no escrow,
  no back door, and backups do not help. A backup restores a vault after drive loss
  or damage, not after credential loss.
- Restoring an older backup brings back the secrets as of that backup, including
  ones deleted since; rotate provider credentials if a drive or backup was lost.

## Secret handles

Values leave the vault only through `UnlockedVault::resolve` / `VaultService::resolve`
for a `SecretHandle { handle_id, secret_ref, consumer, expires_at }` (serializable, no
secret bytes). Handles are issued only to consumers listed in the secret's policy
(`provider:<id>`, `mcp:<server-id>`, `process:<tool-id>`, `git:<host>`); model,
prompt, agent, session, context, memory, log, event, environment, and shell scopes
are rejected outright. Resolution checks the handle against the in-memory registry
(not the caller's copy), the monotonic-clock expiry, revocation, that the presenting
consumer equals the handle's consumer, that the secret still exists, and that its
current policy still allows the consumer. Handles exist only in the memory of one
unlock session: lock, idle auto-lock, daemon restart, and `revoke_all_handles`
invalidate them. Handle issuance and resolution are Rust APIs for the broker; they
are not exposed over IPC.

Issuance prunes revoked and expired entries before enforcing the 4096-handle
registry limit. Revocation immediately frees capacity; a pruned handle is denied
as unknown rather than remaining retained until its former expiry.

## In-memory handling

- VMK, KEKs, subkeys, recovery keys, decrypted index/record plaintexts, and
  `SecretBytes`/`SecretString` are held in `zeroize::Zeroizing` buffers and wiped on
  drop; `UnlockedVault` wipes its keys on lock or drop. Argon2 memory blocks are
  wiped (`zeroize` feature). `Debug`/`Display` of secret-bearing types print
  `[REDACTED]`; none of them implement `Serialize` or `Clone`.
- Not done: memory locking (`mlock`/`VirtualLock`) is not used, so secrets can reach
  swap, hibernation files, or crash dumps. JSON-RPC request buffers and
  `serde_json::Value` copies that carry passphrases or values over local IPC (daemon
  and CLI side) are not zeroized. Copies made by consumers after resolution are their
  responsibility.

## Threat model

Defends against (spec §90, §92):

- **Lost or stolen drive / offline attacker with the files**: values, names, kinds,
  consumer policies, and per-secret timestamps are encrypted; tampering with any byte
  of the header, index, or records is detected; records cannot be swapped, replayed
  across revisions, or moved between vaults. Offline guessing is limited only by
  passphrase strength and the Argon2id cost.
- **Secret exfiltration via models, prompt injection, or untrusted tool/MCP output**:
  there is no API that returns a value except resolving a scoped handle; model and
  shell-environment consumers are impossible to express; handles carry no bytes.
- **Accidental leakage**: secret types redact `Debug`/`Display`; daemon errors never
  echo request parameters; the CLI never accepts secrets from argv or environment.
- **Interrupted writes and crashes**: the last committed state stays readable.

Does not defend against:

- A host controlled by an attacker (administrator/root malware, keyloggers, kernel
  malware, malicious firmware) while the passphrase is typed or the vault is unlocked.
- **Whole-vault rollback**: someone with write access can replace the vault directory
  with an older complete copy; without a trusted monotonic counter this is
  undetectable offline. Partial rollback (mixing generations) is detected.
- Deletion or destruction of vault files (detected, not prevented; keep backups).
- Metadata visible in plaintext: number of record files (≈ number of secrets),
  256-byte value-size buckets, index size, commit count, creation time, KDF
  parameters, and recovery-slot presence.
- Other processes of the same OS user that can read the daemon's endpoint
  capability: they can call `vault.*` methods (they still cannot read values over
  IPC, but can store, delete, or lock).
- Side channels (timing, cache, power) have not been analysed beyond using
  constant-time comparisons and the primitives' own implementations.
- Weak passphrases.

## Not implemented yet (follow-ups)

- Independent security review (required before release).
- Phone/second-factor unlock (§63), host-trust-dependent unlock policy (§61), and
  platform key wrapping (§15.2 alternative) — new slot kinds can be added in a new
  format version.
- Adding or rotating a recovery key after creation; VMK rotation (re-encrypting all
  records); KDF re-calibration per host.
- Audit events (`vault.unlocked`, `secret.granted`, §94): `aidex-vault` returns state
  changes to its caller; the daemon/broker will emit typed events once the events
  rewrite lands.
- Memory locking and zeroization of IPC buffers.
