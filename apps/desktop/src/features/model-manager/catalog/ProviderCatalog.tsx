import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import type { ModelPickerModel, ModelPickerProvider } from "@aidex/ui/components";
import { call } from "@aidex/client/rpc";
import { ErrorKind, toAidexError, type AidexError } from "@aidex/client/errors";
import type { InferenceStatus } from "@aidex/client/dto/inference";
import type {
  ModelList,
  ModelRef,
  ProviderAccount,
  ProviderFailure,
  ProviderModel,
  ProviderPreset,
} from "@aidex/client/dto/models";
import type { RuntimeConfig, RuntimeDriverInfo } from "@aidex/client/dto/runtime";
import { usePreferences } from "../../../app/providers/preferences";
import { useInferenceStatus } from "../../../hooks/useInferenceStatus";
import { useRpc } from "../../../hooks/useRpc";
import { buildPickerCatalog } from "./buildCatalog";
import { isChatModel } from "./modelFacts";
import { nativeModelKey, parseModelKey, runtimeOwnerId } from "./modelKeys";
import { useModelPreferences } from "./useModelPreferences";
import {
  supportsModelDiscovery,
  type RuntimeHealthState,
  type RuntimeModelsState,
} from "./runtimes";
import { useRuntimeState } from "./useRuntimeState";

export type ProviderCatalog = {
  /** Picker rail entries, in rail order. */
  providers: ModelPickerProvider[];
  /** Picker rows: streamed chat models and runtime models, without hidden ones. */
  models: ModelPickerModel[];
  /** True until the first model listing arrives. */
  loading: boolean;
  /** True while a refresh started here is running. */
  refreshing: boolean;
  /** Model or provider listing failure; runtime and preference problems are reported separately. */
  error: string | null;
  /**
   * Drop cached listings and list again (one provider or all), then reload accounts and runtime
   * health. Resolves with the fresh listing; rejects with the daemon's error.
   */
  refresh: (providerId?: string) => Promise<ModelList>;
  /** Re-read everything without dropping the daemon's caches. */
  reload: () => void;

  modelList: ModelList | undefined;
  /** Every listed model whose provider can stream chat with it. */
  chatModels: ProviderModel[];
  accounts: ProviderAccount[];
  presets: ProviderPreset[];
  runtimes: RuntimeConfig[];
  runtimeDrivers: RuntimeDriverInfo[];
  runtimeHealth: Record<string, RuntimeHealthState>;
  runtimeModels: Record<string, RuntimeModelsState>;
  /** Why runtimes could not be read; screens show a calm notice instead of failing. */
  runtimesError: AidexError | null;
  /** True until the runtime list first answers. */
  runtimesLoading: boolean;
  checkRuntime: (id: string) => Promise<void>;
  loadRuntimeModels: (id: string, force?: boolean) => void;
  /** Fetch model lists of every enabled runtime that can report them (picker open). */
  loadAllRuntimeModels: () => void;

  inference: InferenceStatus | undefined;
  refreshInference: () => void;

  favorites: ReadonlySet<string>;
  hidden: ReadonlySet<string>;
  toggleFavorite: (key: string) => void;
  setHidden: (key: string, hidden: boolean) => void;
  preferencesError: string | null;

  /** The registry default (`model.use`). */
  registryDefault: ModelRef | null;
  /** Default model for new chats: the saved preference, else the registry default. */
  defaultKey: string | null;
  findModel: (key: string | null | undefined) => ProviderModel | undefined;
  failureFor: (providerId: string) => ProviderFailure | undefined;
  /** Rail entry that owns a model key, if listed. */
  providerFor: (key: string | null | undefined) => ModelPickerProvider | undefined;
};

const CatalogContext = createContext<ProviderCatalog | null>(null);
const NO_ACCOUNTS: ProviderAccount[] = [];
const NO_PRESETS: ProviderPreset[] = [];

/**
 * One shared view of providers, models, runtimes, local inference, and picker preferences, so the
 * chat picker, the Models sidebar, and the providers screen refresh together.
 */
export function ProviderCatalogProvider({ children }: { children: ReactNode }) {
  const preferences = usePreferences();
  const models = useRpc("model.list");
  const accounts = useRpc("model.providers");
  const presets = useRpc("provider.presets");
  const inference = useInferenceStatus(models.refetch);
  const runtime = useRuntimeState();
  const modelPreferences = useModelPreferences();
  const [refreshing, setRefreshing] = useState(false);
  const refreshesInFlight = useRef(0);

  // Listing models records each provider's catalog status; read the accounts again to show it.
  const refetchAccounts = accounts.refetch;
  useEffect(() => {
    if (models.data) refetchAccounts();
  }, [models.data, refetchAccounts]);

  const registryDefault = models.data?.default_model ?? null;
  const defaultKey =
    preferences.modelId || (registryDefault ? nativeModelKey(registryDefault) : null);

  const accountList = accounts.data?.providers ?? NO_ACCOUNTS;
  const presetList = presets.data?.presets ?? NO_PRESETS;
  const picker = useMemo(
    () =>
      buildPickerCatalog({
        modelList: models.data,
        accounts: accountList,
        presets: presetList,
        runtimes: runtime.runtimes,
        drivers: runtime.drivers,
        runtimeHealth: runtime.health,
        runtimeModels: runtime.models,
        inference: inference.data,
        hidden: modelPreferences.hidden,
        defaultKey,
      }),
    [
      models.data,
      accountList,
      presetList,
      runtime.runtimes,
      runtime.drivers,
      runtime.health,
      runtime.models,
      inference.data,
      modelPreferences.hidden,
      defaultKey,
    ],
  );

  const refetchModels = models.refetch;
  const reloadRuntimes = runtime.reload;
  // Favorites and hidden models are read again too, so a read that failed while the vault was
  // locked recovers once it is unlocked.
  const reloadModelPreferences = modelPreferences.reload;
  const refresh = useCallback(
    async (providerId?: string): Promise<ModelList> => {
      refreshesInFlight.current += 1;
      setRefreshing(true);
      try {
        const params = providerId ? { provider_id: providerId, refresh: true } : { refresh: true };
        try {
          return await call("model.list", params);
        } catch (caught) {
          // An older daemon takes no params; its listing is still the freshest available.
          if (toAidexError(caught).kind !== ErrorKind.invalidParams) throw caught;
          return await call("model.list");
        }
      } finally {
        refetchModels();
        refetchAccounts();
        if (!providerId) {
          reloadRuntimes();
          reloadModelPreferences();
        }
        refreshesInFlight.current -= 1;
        setRefreshing(refreshesInFlight.current > 0);
      }
    },
    [refetchModels, refetchAccounts, reloadRuntimes, reloadModelPreferences],
  );

  const refetchPresets = presets.refetch;
  const reload = useCallback(() => {
    refetchModels();
    refetchAccounts();
    refetchPresets();
    reloadRuntimes();
    reloadModelPreferences();
  }, [refetchModels, refetchAccounts, refetchPresets, reloadRuntimes, reloadModelPreferences]);

  const loadRuntimeModels = runtime.loadModels;
  const runtimeList = runtime.runtimes;
  const runtimeHealth = runtime.health;
  const loadAllRuntimeModels = useCallback(() => {
    for (const config of runtimeList) {
      const health = runtimeHealth[config.id];
      const installed = health?.state === "ready" && health.health.installed;
      if (config.enabled && installed && supportsModelDiscovery(config.driver))
        loadRuntimeModels(config.id);
    }
  }, [runtimeList, runtimeHealth, loadRuntimeModels]);

  const value = useMemo<ProviderCatalog>(() => {
    const listed = models.data?.models ?? [];
    const findModel = (key: string | null | undefined) => {
      const parsed = parseModelKey(key);
      if (!parsed || parsed.kind !== "native") return undefined;
      return listed.find(
        (model) =>
          model.model.provider_id === parsed.providerId && model.model.model_id === parsed.modelId,
      );
    };
    const providerFor = (key: string | null | undefined) => {
      const parsed = parseModelKey(key);
      if (!parsed) return undefined;
      const owner = parsed.kind === "runtime" ? runtimeOwnerId(parsed.runtimeId) : parsed.providerId;
      return picker.providers.find((provider) => provider.id === owner);
    };
    return {
      providers: picker.providers,
      models: picker.models,
      loading: !models.data && !models.error && models.loading,
      refreshing,
      error: models.error?.message ?? accounts.error?.message ?? null,
      refresh,
      reload,
      modelList: models.data,
      chatModels: listed.filter(isChatModel),
      accounts: accountList,
      presets: presetList,
      runtimes: runtime.runtimes,
      runtimeDrivers: runtime.drivers,
      runtimeHealth: runtime.health,
      runtimeModels: runtime.models,
      runtimesError: runtime.error,
      runtimesLoading: runtime.loading,
      checkRuntime: runtime.checkHealth,
      loadRuntimeModels,
      loadAllRuntimeModels,
      inference: inference.data,
      refreshInference: inference.refetch,
      favorites: modelPreferences.favorites,
      hidden: modelPreferences.hidden,
      toggleFavorite: modelPreferences.toggleFavorite,
      setHidden: modelPreferences.setHidden,
      preferencesError: modelPreferences.error,
      registryDefault,
      defaultKey,
      findModel,
      failureFor: (providerId) =>
        models.data?.failures.find((failure) => failure.provider_id === providerId),
      providerFor,
    };
  }, [
    picker,
    models.data,
    models.error,
    models.loading,
    accounts.error,
    accountList,
    presetList,
    refreshing,
    refresh,
    reload,
    runtime.runtimes,
    runtime.drivers,
    runtime.health,
    runtime.models,
    runtime.error,
    runtime.loading,
    runtime.checkHealth,
    loadRuntimeModels,
    loadAllRuntimeModels,
    inference.data,
    inference.refetch,
    modelPreferences,
    registryDefault,
    defaultKey,
  ]);

  return <CatalogContext.Provider value={value}>{children}</CatalogContext.Provider>;
}

/** Providers, models, runtimes, and picker preferences shared across the workspace. */
export function useProviderCatalog(): ProviderCatalog {
  const catalog = useContext(CatalogContext);
  if (!catalog) throw new Error("useProviderCatalog must be used inside ProviderCatalogProvider");
  return catalog;
}
