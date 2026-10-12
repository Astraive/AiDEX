// Pure merge of the daemon's provider, model, runtime, and inference reports into model picker data.
import type {
  ModelPickerModel,
  ModelPickerProvider,
  ModelPickerProviderStatus,
} from "@aidex/ui/components";
import type { InferenceStatus } from "@aidex/client/dto/inference";
import type {
  ModelList,
  ProviderAccount,
  ProviderFailure,
  ProviderPreset,
} from "@aidex/client/dto/models";
import type { RuntimeConfig, RuntimeDriverInfo } from "@aidex/client/dto/runtime";
import { isChatModel, toPickerModel } from "./modelFacts";
import { nativeModelKey, runtimeOwnerId } from "./modelKeys";
import {
  LOCAL_PROVIDER_BRAND,
  LOCAL_PROVIDER_ID,
  LOCAL_PROVIDER_NAME,
  accountBrand,
  accountTitles,
  isAuthFailure,
  isRuntimeAccount,
  presetForAccount,
} from "./providerAccounts";
import {
  runtimeBrand,
  runtimeName,
  runtimePickerModels,
  runtimePickerStatus,
  type RuntimeHealthState,
  type RuntimeModelsState,
} from "./runtimes";

export type CatalogInputs = {
  modelList: ModelList | undefined;
  accounts: readonly ProviderAccount[];
  presets: readonly ProviderPreset[];
  runtimes: readonly RuntimeConfig[];
  drivers: readonly RuntimeDriverInfo[];
  runtimeHealth: Readonly<Record<string, RuntimeHealthState>>;
  runtimeModels: Readonly<Record<string, RuntimeModelsState>>;
  inference: InferenceStatus | undefined;
  hidden: ReadonlySet<string>;
  /** Effective default for new chats; its row gets the "Default" badge. */
  defaultKey: string | null;
};

export type PickerCatalog = {
  providers: ModelPickerProvider[];
  /** Picker rows: streamed chat models and runtime models, without hidden ones. */
  models: ModelPickerModel[];
};

const LOCAL_SETUP_TEXT = "Start the local model in Models to use it.";

/** The failure the latest listing reported for a provider, falling back to its recorded catalog status. */
export function providerFailure(
  account: ProviderAccount | undefined,
  providerId: string,
  modelList: ModelList | undefined,
): ProviderFailure | undefined {
  if (modelList) return modelList.failures.find((failure) => failure.provider_id === providerId);
  return account?.catalog?.error ?? undefined;
}

function localStatus(inference: InferenceStatus | undefined, listed: boolean) {
  if (listed && inference?.state === "ready") return { status: "ready" as const };
  if (inference?.state === "starting")
    return {
      status: "setup" as const,
      statusText: `Loading ${inference.model_name ?? "the local model"}…`,
    };
  if (inference?.state === "failed")
    return {
      status: "error" as const,
      statusText: inference.startup_error ?? LOCAL_SETUP_TEXT,
    };
  return { status: "setup" as const, statusText: LOCAL_SETUP_TEXT };
}

/** Picker readiness of a configured or runtime-registered provider account. */
export function accountPickerStatus(input: {
  account: ProviderAccount;
  failure: ProviderFailure | undefined;
  chatModelCount: number;
  listed: boolean;
  inference: InferenceStatus | undefined;
}): { status: ModelPickerProviderStatus; statusText?: string } {
  const { account, failure, chatModelCount, listed, inference } = input;
  if (account.descriptor.id === LOCAL_PROVIDER_ID) return localStatus(inference, listed);
  if (!account.enabled) return { status: "disabled" };
  if (failure) {
    if (isAuthFailure(failure.kind))
      return { status: "setup", statusText: failure.message || "Open provider setup to sign in." };
    return { status: "error", statusText: failure.message };
  }
  if (listed && chatModelCount === 0)
    return { status: "warning", statusText: "No chat models are available." };
  return { status: "ready" };
}

/** Rail entries and picker rows, in rail order: configured accounts, the local model, then runtimes. */
export function buildPickerCatalog(inputs: CatalogInputs): PickerCatalog {
  const { modelList, accounts, presets, runtimes, drivers, inference, hidden } = inputs;
  const chatModels = (modelList?.models ?? []).filter(isChatModel);
  const titles = accountTitles(accounts, presets);
  const accountProviders: ModelPickerProvider[] = [];
  let localProvider: ModelPickerProvider | undefined;

  for (const account of accounts) {
    const id = account.descriptor.id;
    const preset = presetForAccount(account, presets);
    const provider: ModelPickerProvider = {
      id,
      name: titles.get(id) ?? id,
      brand: accountBrand(account, preset),
      kind: isRuntimeAccount(account) ? "local" : "model",
      ...accountPickerStatus({
        account,
        failure: providerFailure(account, id, modelList),
        chatModelCount: chatModels.filter((model) => model.model.provider_id === id).length,
        listed: !!modelList,
        inference,
      }),
    };
    if (id === LOCAL_PROVIDER_ID) localProvider = provider;
    else accountProviders.push(provider);
  }

  // Providers that listed models or failures but are missing from `model.providers` (an older daemon
  // or a race) still get a rail entry, so their models stay reachable.
  const known = new Set(accounts.map((account) => account.descriptor.id));
  const orphanIds = new Set<string>();
  for (const model of chatModels) orphanIds.add(model.model.provider_id);
  for (const failure of modelList?.failures ?? []) orphanIds.add(failure.provider_id);
  for (const id of orphanIds) {
    if (known.has(id) || id === LOCAL_PROVIDER_ID) continue;
    const failure = modelList?.failures.find((entry) => entry.provider_id === id);
    accountProviders.push({
      id,
      name: id,
      brand: "custom",
      kind: "model",
      status: failure ? (isAuthFailure(failure.kind) ? "setup" : "error") : "ready",
      statusText: failure?.message,
    });
  }

  // The local model gets an entry while it is loading, failed, or running, even before it registers.
  if (
    !localProvider &&
    (inference?.state === "starting" ||
      inference?.state === "failed" ||
      inference?.state === "ready" ||
      chatModels.some((model) => model.model.provider_id === LOCAL_PROVIDER_ID))
  ) {
    localProvider = {
      id: LOCAL_PROVIDER_ID,
      name: LOCAL_PROVIDER_NAME,
      brand: LOCAL_PROVIDER_BRAND,
      kind: "local",
      ...localStatus(
        inference,
        chatModels.some((model) => model.model.provider_id === LOCAL_PROVIDER_ID),
      ),
    };
  }

  const runtimeProviders: ModelPickerProvider[] = runtimes
    .filter((runtime) => runtime.enabled)
    .map((runtime) => {
      const name = runtimeName(runtime, drivers);
      return {
        id: runtimeOwnerId(runtime.id),
        name,
        brand: runtimeBrand(runtime.driver),
        kind: "runtime" as const,
        setupLabel: "Open runtime setup",
        ...runtimePickerStatus(inputs.runtimeHealth[runtime.id], name),
      };
    });

  // Models declared on a configured account are the user's custom models.
  const customKeys = new Set(
    accounts
      .filter((account) => !isRuntimeAccount(account))
      .flatMap((account) =>
        account.declared_models.map((model) =>
          nativeModelKey({ provider_id: account.descriptor.id, model_id: model.id }),
        ),
      ),
  );
  const nativeRows = chatModels.map((model) =>
    toPickerModel(model, {
      defaultKey: inputs.defaultKey,
      custom: customKeys.has(nativeModelKey(model.model)),
    }),
  );
  const runtimeRows = runtimes
    .filter((runtime) => runtime.enabled)
    .flatMap((runtime) => runtimePickerModels(runtime, inputs.runtimeModels[runtime.id]));

  return {
    providers: [...accountProviders, ...(localProvider ? [localProvider] : []), ...runtimeProviders],
    models: [...nativeRows, ...runtimeRows].filter((model) => !hidden.has(model.key)),
  };
}
