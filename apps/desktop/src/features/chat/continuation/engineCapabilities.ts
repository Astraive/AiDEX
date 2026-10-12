import { useEffect, useState } from "react";
import { call } from "@aidex/client/rpc";
import type { EngineCapabilities } from "@aidex/client/dto/execution";

// One answer per window: the engine runs on this computer and its sandbox does not change while the
// app is open.
let known: boolean | undefined;

/** The webview's own platform; the daemon runs on the same computer. */
function windowsHost(): boolean {
  return typeof navigator !== "undefined" && /Windows/i.test(navigator.userAgent);
}

/**
 * Whether shell commands outside Full access are refused, from the engine capabilities in
 * `execution.status`: `shell_requires_full_access_on_windows`, or no filesystem confinement for
 * commands on this host.
 */
export function shellNeedsFullAccessFrom(
  capabilities: Pick<EngineCapabilities, "shell_requires_full_access_on_windows" | "filesystem_confinement">,
): boolean {
  return capabilities.shell_requires_full_access_on_windows || !capabilities.filesystem_confinement;
}

/**
 * Whether commands need Full access on this computer, for honest permission descriptions. Asks the
 * engine once `enabled` (a permission choice is on screen); until it answers, assumes so on Windows.
 */
export function useShellNeedsFullAccess(enabled: boolean): boolean {
  const [value, setValue] = useState(() => known ?? windowsHost());
  useEffect(() => {
    if (!enabled) return;
    if (known !== undefined) {
      setValue(known);
      return;
    }
    let live = true;
    call("execution.status")
      .then((status) => {
        const answer = shellNeedsFullAccessFrom(status.capabilities);
        known = answer;
        if (live) setValue(answer);
      })
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, [enabled]);
  return value;
}
