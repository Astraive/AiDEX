// Mirrors the `vault.*` DTOs in crates/aidexd/src/methods/vault.rs, `aidex_vault::VaultStatus`
// (crates/vault/src/service.rs) and `aidex_vault::SecretMeta` (crates/vault/src/model.rs).
// Hand-written until schema generation exists. Passphrases, recovery keys and secret values travel
// only as params over the authenticated local IPC; they never appear in results, and no method
// reads a secret's value back.

export type VaultState = "missing" | "locked" | "unlocked" | "unavailable";

/** Result of `vault.status` (and of `vault.unlock`). Contains no secret material. */
export type VaultStatus = {
  state: VaultState;
  vault_id?: string;
  /** Number of secrets, only while unlocked. */
  secrets?: number;
  has_recovery_key?: boolean;
  unlocked_by?: "passphrase" | "recovery_key";
  unlocked_at?: string;
  idle_lock_secs?: number;
  /** Stable error kind when `state` is `unavailable`. */
  reason?: string;
  message?: string;
};

/** Params of `vault.init`. */
export type VaultInitParams = { passphrase: string; recovery?: boolean };

/** Result of `vault.init`. `recovery_key` is returned exactly once when requested; the daemon keeps no copy. */
export type VaultInitResult = { state: VaultState; vault_id: string; recovery_key?: string };

/** Params of `vault.unlock`: exactly one of the two. */
export type VaultUnlockParams = { passphrase: string } | { recovery_key: string };

/** Result of `vault.lock`. */
export type VaultLockResult = { state: VaultState; was_unlocked: boolean; revoked_handles: number };

/** `aidex_vault::MIN_PASSPHRASE_CHARS`. */
export const MIN_PASSPHRASE_CHARS = 8;

/** `aidex_vault::SecretKind`: a label for how a value is used, not how it is protected. */
export type SecretKind = "api_key" | "token" | "password" | "ssh_key" | "private_key" | "generic";

/**
 * A value-free description of a stored secret. `name` is 1-128 characters of `A-Z a-z 0-9 . _ -`
 * starting with a letter or digit; `allowed_consumers` are exact `<kind>:<target>` scopes
 * (`provider`, `mcp`, `process` or `git`) that may be issued handles. Times are RFC 3339.
 */
export type SecretMeta = {
  secret_ref: { vault_id: string; secret_id: string };
  name: string;
  kind: SecretKind;
  created_at: string;
  updated_at: string;
  policy: { allowed_consumers: string[] };
};

/**
 * Params of `vault.secret.set`, which creates or replaces `name`. `consumers` replaces the whole
 * policy (an empty list releases the value to no one), so merge the existing consumers first to
 * keep them. `kind` defaults to `generic`.
 */
export type SecretSetParams = {
  name: string;
  value: string;
  consumers: string[];
  kind?: SecretKind;
};

/**
 * Result of `vault.secret.list` (needs an unlocked vault). `managed` names the records AiDEX itself
 * owns (record and runtime keys, preferences, `oauth.*` sign-ins): `vault.secret.set` and
 * `vault.secret.delete` refuse them with `policy_denied` and `data.reason = "managed_secret"`.
 */
export type SecretList = { secrets: SecretMeta[]; managed: string[] };

/**
 * Params of `vault.passphrase.change` (needs an unlocked vault). `old_passphrase` may be omitted
 * only after a recovery-key unlock; otherwise it is refused (`data.reason = "passphrase_required"`).
 */
export type PassphraseChangeParams = { old_passphrase?: string; new_passphrase: string };
