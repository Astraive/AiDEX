// Mirrors the `vault.*` DTOs in crates/aidexd/src/methods/vault.rs and `aidex_vault::VaultStatus`
// (crates/vault/src/service.rs). Hand-written until schema generation exists. Passphrases and recovery
// keys travel only as params over the authenticated local IPC; they never appear in results.

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
