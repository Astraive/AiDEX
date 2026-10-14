// The update operations that can run for minutes (re-verifying, staging, hashing, and self-testing a
// whole bundle, on a USB drive). They go through the `rpc_call_long` bridge command: a connection of
// their own with a 30-minute limit, so they neither time out at the shared connection's 60 s nor hold
// up every other screen's calls while they run (`apps/desktop/src-tauri/src/bridge/connection.rs`).
import { command } from "@aidex/client/bridge";
import type { RpcParams, RpcResult } from "@aidex/client/rpc";

/** The methods `rpc_call_long` accepts; the bridge refuses any other. */
export type UpdateMethod = "update.check" | "update.apply" | "update.rollback";

export function callUpdate<M extends UpdateMethod>(method: M, params: RpcParams<M>): Promise<RpcResult<M>> {
  return command<RpcResult<M>>("rpc_call_long", { method, params: params ?? null });
}

/**
 * Whether a failed long call may still be running in AiDEX Core: the bridge stopped waiting or lost
 * its connection, and the daemon does not stop an update when its caller goes away. Daemon refusals
 * (bad signature, downgrade, locked, ...) are final.
 */
export function mayStillRun(error: { kind: string }): boolean {
  return error.kind === "io" || error.kind === "protocol" || error.kind === "daemon_unavailable";
}
