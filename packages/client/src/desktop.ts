// Typed wrappers for the desktop app's Tauri commands (apps/desktop/src-tauri/src/commands/mod.rs).
// Daemon methods go through `call`/`subscribe` in `./rpc`; the long update calls through `callUpdate`.
import { command } from "./bridge";
import type { BackendCapabilities } from "./dto/sandbox";
import type { AidexStatus, DriveReport } from "./dto/status";
import type { RpcParams, RpcResult } from "./rpc";

/** `aidex_status`: the session drive and whether its daemon answers. */
export function aidexStatus(): Promise<AidexStatus> {
  return command<AidexStatus>("aidex_status");
}

/** `drive_status`: manifest health and volume facts for the session drive. */
export function driveStatus(): Promise<DriveReport> {
  return command<DriveReport>("drive_status");
}

/** `sandbox_capabilities`: what the OS sandbox can enforce on this host. */
export function sandboxCapabilities(): Promise<BackendCapabilities> {
  return command<BackendCapabilities>("sandbox_capabilities");
}

/** `daemon_start`: launch `aidexd` for the session drive (the app stops it on exit only if it started it). */
export function startDaemon(): Promise<AidexStatus> {
  return command<AidexStatus>("daemon_start");
}

/** `daemon_stop`: stop the daemon serving the session drive. */
export function stopDaemon(): Promise<AidexStatus> {
  return command<AidexStatus>("daemon_stop");
}

/** `select_drive`: use `path` as this session's drive. Kept in memory only; nothing is written to the host. */
export function selectDrive(path: string): Promise<AidexStatus> {
  return command<AidexStatus>("select_drive", { path });
}

/** `set_ui_scale`: zoom the whole window to `percent` (75-125). Nothing is saved on the host. */
export function setUiScale(percent: number): Promise<void> {
  return command<void>("set_ui_scale", { percent });
}

/** The daemon methods `rpc_call_long` accepts; the bridge refuses any other (`invalid_method`). */
export type LongRpcMethod = "update.check" | "update.apply" | "update.rollback";

/**
 * `rpc_call_long`: an update operation that can run for minutes (re-verifying, staging, hashing, and
 * self-testing a whole bundle, on a USB drive), on a connection of its own that waits up to 30
 * minutes, so it neither times out at the shared connection's 60 s nor holds up other calls.
 */
export function callUpdate<M extends LongRpcMethod>(method: M, params: RpcParams<M>): Promise<RpcResult<M>> {
  return command<RpcResult<M>>("rpc_call_long", { method, params: params ?? null });
}
