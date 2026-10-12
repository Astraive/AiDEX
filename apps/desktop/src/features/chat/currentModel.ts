// The conversation's model as the picker shows it, including models that are no longer listed.
import type { ModelPickerModel, ModelPickerProvider } from "@aidex/ui/components";
import { RUNTIME_DEFAULT_MODEL } from "@aidex/client/dto/runtime";
import type { ProviderCatalog } from "../model-manager/catalog/ProviderCatalog";
import { toPickerModel } from "../model-manager/catalog/modelFacts";
import { parseModelKey, runtimeOwnerId } from "../model-manager/catalog/modelKeys";
import {
  LOCAL_PROVIDER_BRAND,
  LOCAL_PROVIDER_ID,
  LOCAL_PROVIDER_NAME,
} from "../model-manager/catalog/providerAccounts";
import { runtimeBrand, runtimeName } from "../model-manager/catalog/runtimes";
import type { ResolvedConversationModel } from "./resolveConversationModel";

export type CurrentModel = {
  /** Picker row for the chosen key; marked unavailable when it is not runnable now. */
  model?: ModelPickerModel;
  provider?: ModelPickerProvider;
  /** Name for labels and messages. */
  name: string;
};

type CatalogView = Pick<
  ProviderCatalog,
  "models" | "providerFor" | "findModel" | "runtimes" | "runtimeDrivers" | "defaultKey"
>;

/** Picker row, rail entry, and display name for the resolved conversation model. */
export function describeCurrentModel(
  resolved: ResolvedConversationModel,
  catalog: CatalogView,
): CurrentModel {
  const parsed = parseModelKey(resolved.key);
  if (!resolved.key || !parsed) return { name: "Choose a model" };
  // A loading local model is on its way; only models that cannot run are marked unavailable.
  const available = resolved.state === "ready" || resolved.state === "loading-local";
  const listed = catalog.models.find((model) => model.key === resolved.key);
  const descriptor = catalog.findModel(resolved.key);
  let model: ModelPickerModel;
  if (listed) model = available ? listed : { ...listed, unavailable: true };
  else if (descriptor)
    // Listed but hidden from the picker: still the conversation's model.
    model = {
      ...toPickerModel(descriptor, { defaultKey: catalog.defaultKey, custom: false }),
      unavailable: !available,
    };
  else
    model = {
      key: resolved.key,
      providerId: parsed.kind === "runtime" ? runtimeOwnerId(parsed.runtimeId) : parsed.providerId,
      name:
        parsed.kind === "runtime" && parsed.modelId === RUNTIME_DEFAULT_MODEL
          ? "Default model"
          : parsed.modelId,
      description: parsed.modelId,
      unavailable: !available,
    };

  let provider = catalog.providerFor(resolved.key);
  if (!provider) {
    if (parsed.kind === "runtime") {
      const runtime = catalog.runtimes.find((candidate) => candidate.id === parsed.runtimeId);
      provider = {
        id: runtimeOwnerId(parsed.runtimeId),
        name: runtime ? runtimeName(runtime, catalog.runtimeDrivers) : parsed.runtimeId,
        brand: runtime ? runtimeBrand(runtime.driver) : "custom",
        kind: "runtime",
        status: runtime && !runtime.enabled ? "disabled" : "error",
      };
    } else if (parsed.providerId === LOCAL_PROVIDER_ID) {
      provider = { id: LOCAL_PROVIDER_ID, name: LOCAL_PROVIDER_NAME, brand: LOCAL_PROVIDER_BRAND, kind: "local", status: "setup" };
    } else {
      provider = { id: parsed.providerId, name: parsed.providerId, brand: "custom", kind: "model", status: "error" };
    }
  }
  return { model, provider, name: model.name };
}
