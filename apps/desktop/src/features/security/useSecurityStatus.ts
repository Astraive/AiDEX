// The Security page's live status: one set of queries and one poll for every tab, plus a small store
// the sidebar (a sibling in the shell, so it cannot take props) reads instead of polling on its own.
import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";
import { ErrorKind, type AidexError } from "@aidex/client/errors";
import type { ExecutionStatus } from "@aidex/client/dto/execution";
import type { NetworkStatus } from "@aidex/client/dto/models";
import type { ApprovalList } from "@aidex/client/dto/sessions";
import type { VaultState, VaultStatus } from "@aidex/client/dto/vault";
import type { BridgeQuery } from "../../hooks/useBridgeQuery";
import { useRpc } from "../../hooks/useRpc";

/** How often the Security page re-reads its status while it is open. */
export const SECURITY_POLL_MS = 5000;

const ACTIVE_RUN_STATUSES = new Set(["pending", "running", "waiting_approval"]);

/** Whether a run summary is still active (it can be cancelled or is waiting for a decision). */
export const activeRun = (run: { status: string }): boolean => ACTIVE_RUN_STATUSES.has(run.status);

/** Whether a read failed only because the vault is locked: an expected state the vault tile already shows. */
export const lockedByVault = (error: Pick<AidexError, "kind"> | null | undefined): boolean =>
  error?.kind === ErrorKind.vaultLocked;

export type SecurityStatus = {
  vault: BridgeQuery<VaultStatus>;
  /** `approval.list` filtered to pending with one item: only `total_approvals` is used. */
  pending: BridgeQuery<ApprovalList>;
  network: BridgeQuery<NetworkStatus>;
  execution: BridgeQuery<ExecutionStatus>;
  /** Changes on every poll and refresh; panels re-read their own queries when it does. */
  version: number;
  /** Re-reads every shared query now, e.g. after a decision, a vault change, or a cancelled run. */
  refresh: () => void;
};

/** What the sidebar shows: published by the page, `undefined` until known. */
export type SecuritySnapshot = { vaultState?: VaultState; pendingApprovals?: number };

const EMPTY: SecuritySnapshot = {};
let snapshot: SecuritySnapshot = EMPTY;
const listeners = new Set<() => void>();

function publish(next: SecuritySnapshot) {
  if (next.vaultState === snapshot.vaultState && next.pendingApprovals === snapshot.pendingApprovals) return;
  snapshot = next;
  for (const listener of listeners) listener();
}

function subscribeSnapshot(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** The latest status the Security page published, for its sidebar. */
export function useSecuritySnapshot(): SecuritySnapshot {
  return useSyncExternalStore(subscribeSnapshot, () => snapshot, () => EMPTY);
}

/**
 * Owns the Security page's shared queries and polls them every `SECURITY_POLL_MS`. `execution.status`
 * (up to 1000 run summaries) is read only while `withExecution` is set.
 */
export function useSecurityStatus(withExecution: boolean): SecurityStatus {
  const vault = useRpc("vault.status");
  const pending = useRpc("approval.list", { status: "pending", limit: 1 });
  const network = useRpc("network.status");
  const execution = useRpc("execution.status", undefined, { enabled: withExecution });
  const [version, setVersion] = useState(0);

  const refetchers = useRef([vault.refetch, pending.refetch, network.refetch, execution.refetch]);
  refetchers.current = [vault.refetch, pending.refetch, network.refetch, execution.refetch];
  const refresh = useCallback(() => {
    for (const refetch of refetchers.current) refetch();
    setVersion((value) => value + 1);
  }, []);

  useEffect(() => {
    const timer = setInterval(refresh, SECURITY_POLL_MS);
    return () => clearInterval(timer);
  }, [refresh]);

  const vaultState = vault.data?.state;
  const pendingApprovals = pending.data?.total_approvals;
  useEffect(() => {
    publish({ vaultState, pendingApprovals });
  }, [vaultState, pendingApprovals]);
  useEffect(
    () => () => {
      publish(EMPTY);
    },
    [],
  );

  return { vault, pending, network, execution, version, refresh };
}

/** Calls `refetch` whenever `version` changes after the first render (the query already loaded once). */
export function useRefetchOnVersion(version: number, refetch: () => void): void {
  const first = useRef(version);
  useEffect(() => {
    if (version !== first.current) refetch();
  }, [version, refetch]);
}
