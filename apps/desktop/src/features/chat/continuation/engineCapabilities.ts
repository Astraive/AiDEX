import { useEffect, useState } from "react";
import { call } from "@aidex/client/rpc";
import type { EngineCapabilities, RuntimeDescriptor } from "@aidex/client/dto/execution";

type EngineFacts = {
  capabilities: EngineCapabilities;
  runtimeDescriptors: readonly RuntimeDescriptor[];
};

// One answer per window: the engine runs on this computer and its sandbox does not change while the
// app is open. The runtime descriptors are read again only when a runtime is missing from them (one
// added since), never on a timer.
let known: EngineFacts | undefined;
let reading: Promise<EngineFacts> | undefined;

/** `execution.status`, shared by every caller while one read is in flight. */
function readEngine(): Promise<EngineFacts> {
  reading ??= call("execution.status")
    .then((status) => {
      known = {
        capabilities: status.capabilities,
        runtimeDescriptors: status.runtime_descriptors ?? [],
      };
      return known;
    })
    .finally(() => {
      reading = undefined;
    });
  return reading;
}

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
  const [value, setValue] = useState(() =>
    known ? shellNeedsFullAccessFrom(known.capabilities) : windowsHost(),
  );
  useEffect(() => {
    if (!enabled) return;
    if (known) {
      setValue(shellNeedsFullAccessFrom(known.capabilities));
      return;
    }
    let live = true;
    readEngine()
      .then((facts) => {
        if (live) setValue(shellNeedsFullAccessFrom(facts.capabilities));
      })
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, [enabled]);
  return value;
}

/**
 * The engine's descriptor of an external runtime (`execution.status` `runtime_descriptors`): its
 * modes, permissions, and whether it can resume. Read once `enabled`; undefined until the engine
 * answers, and when it does not run that runtime.
 */
export function useRuntimeDescriptor(
  runtimeId: string | null | undefined,
  enabled: boolean,
): RuntimeDescriptor | undefined {
  const cached = (id: string) => known?.runtimeDescriptors.find((descriptor) => descriptor.id === id);
  const [descriptor, setDescriptor] = useState(() => (runtimeId ? cached(runtimeId) : undefined));
  useEffect(() => {
    if (!enabled || !runtimeId) return;
    const hit = cached(runtimeId);
    if (hit) {
      setDescriptor(hit);
      return;
    }
    let live = true;
    readEngine()
      .then((facts) => {
        if (live) setDescriptor(facts.runtimeDescriptors.find((item) => item.id === runtimeId));
      })
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, [enabled, runtimeId]);
  return runtimeId && descriptor?.id === runtimeId ? descriptor : undefined;
}
