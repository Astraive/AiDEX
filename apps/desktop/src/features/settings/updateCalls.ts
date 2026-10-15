// How the Updates page reads a failed long update call. Check, apply, and rollback go through
// `callUpdate` (`@aidex/client/desktop`, the `rpc_call_long` bridge command): a connection of their own
// with a 30-minute limit (`apps/desktop/src-tauri/src/bridge/connection.rs`).

/**
 * Whether a failed long call may still be running in AiDEX Core: the bridge stopped waiting or lost
 * its connection, and the daemon does not stop an update when its caller goes away. Daemon refusals
 * (bad signature, downgrade, locked, ...) are final.
 */
export function mayStillRun(error: { kind: string }): boolean {
  return error.kind === "io" || error.kind === "protocol" || error.kind === "daemon_unavailable";
}
