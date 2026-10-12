import { useCallback, useEffect, useRef, useState } from "react";
import { call } from "@aidex/client/rpc";
import { toAidexError, type AidexError } from "@aidex/client/errors";
import type { RuntimeConfig, RuntimeDriverInfo } from "@aidex/client/dto/runtime";
import { useAidexStatus } from "../../../hooks/useAidexStatus";
import { useRpc } from "../../../hooks/useRpc";
import type { RuntimeHealthState, RuntimeModelsState } from "./runtimes";

const NO_RUNTIMES: RuntimeConfig[] = [];
const NO_DRIVERS: RuntimeDriverInfo[] = [];

export type RuntimeState = {
  runtimes: RuntimeConfig[];
  drivers: RuntimeDriverInfo[];
  health: Record<string, RuntimeHealthState>;
  models: Record<string, RuntimeModelsState>;
  /** Why the runtime list could not be read (for example a locked vault or an older daemon). */
  error: AidexError | null;
  loading: boolean;
  reload: () => void;
  checkHealth: (id: string) => Promise<void>;
  /** Fetch a runtime's model list once per session; `force` fetches it again. */
  loadModels: (id: string, force?: boolean) => void;
};

/**
 * External runtimes with their health and model lists. Every call may fail while the vault is
 * locked or against an older daemon; failures are kept per runtime and never thrown.
 */
export function useRuntimeState(): RuntimeState {
  const { connectionKey } = useAidexStatus();
  const list = useRpc("runtime.list");
  const catalog = useRpc("runtime.catalog");
  const [health, setHealth] = useState<Record<string, RuntimeHealthState>>({});
  const [models, setModels] = useState<Record<string, RuntimeModelsState>>({});
  const modelsRef = useRef(models);
  modelsRef.current = models;
  const runtimes = list.data?.runtimes ?? NO_RUNTIMES;

  // Cached answers belong to one drive and daemon.
  useEffect(() => {
    setHealth({});
    setModels({});
  }, [connectionKey]);

  const checkHealth = useCallback(async (id: string) => {
    setHealth((current) => ({ ...current, [id]: { state: "loading" } }));
    try {
      const result = await call("runtime.health", { id });
      setHealth((current) => ({ ...current, [id]: { state: "ready", health: result } }));
    } catch (caught) {
      const message = toAidexError(caught).message;
      setHealth((current) => ({ ...current, [id]: { state: "error", message } }));
    }
  }, []);

  // Check health again whenever a runtime is added, removed, toggled, or pointed at another executable.
  const healthKey = JSON.stringify(
    runtimes.map((runtime) => [runtime.id, runtime.enabled, runtime.executable, runtime.auth.kind]),
  );
  const runtimeIds = useRef<string[]>([]);
  runtimeIds.current = runtimes.map((runtime) => runtime.id);
  useEffect(() => {
    for (const id of runtimeIds.current) void checkHealth(id);
  }, [healthKey, checkHealth]);

  const loadModels = useCallback((id: string, force = false) => {
    const existing = modelsRef.current[id];
    if (!force && existing && existing.state !== "error") return;
    setModels((current) => ({ ...current, [id]: { state: "loading" } }));
    call("runtime.models", { id }).then(
      (result) => setModels((current) => ({ ...current, [id]: { state: "ready", models: result.models } })),
      (caught: unknown) =>
        setModels((current) => ({
          ...current,
          [id]: { state: "error", message: toAidexError(caught).message },
        })),
    );
  }, []);

  const refetchList = list.refetch;
  const refetchCatalog = catalog.refetch;
  const reload = useCallback(() => {
    refetchList();
    refetchCatalog();
    for (const id of runtimeIds.current) void checkHealth(id);
  }, [refetchList, refetchCatalog, checkHealth]);

  return {
    runtimes,
    drivers: catalog.data ?? NO_DRIVERS,
    health,
    models,
    error: list.error,
    loading: !list.data && !list.error && list.loading,
    reload,
    checkHealth,
    loadModels,
  };
}
