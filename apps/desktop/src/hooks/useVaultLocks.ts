import { useContext } from "react";
import { VaultLockContext } from "../services/VaultLockProvider";

/**
 * How many times the vault locked while the workspace was open (`vault.locked` events, followed by
 * `VaultLockProvider`); always 0 outside it, as on the launch screens. A change means decrypted
 * results on screen may belong to a session that has ended.
 */
export function useVaultLocks(): number {
  return useContext(VaultLockContext);
}
