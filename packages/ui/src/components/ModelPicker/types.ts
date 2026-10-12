/** Readiness of a provider in the picker rail. Only `disabled` providers cannot be chosen. */
export type ModelPickerProviderStatus = "ready" | "warning" | "error" | "disabled" | "setup";

/** One rail entry: a model provider account, a local runtime, or an agent runtime. */
export type ModelPickerProvider = {
  id: string;
  name: string;
  /** Brand glyph key for `BrandIcon`; unknown keys show a monogram of `name`. */
  brand: string;
  status: ModelPickerProviderStatus;
  /** Why the provider is not ready; shown in the rail tooltip and the footer. */
  statusText?: string;
  /** Agent runtimes are grouped after model providers in the rail. */
  kind?: "model" | "runtime" | "local";
  /** Footer action label; defaults to "Open provider setup". */
  setupLabel?: string;
};

export type ModelPickerCapability = "tools" | "vision" | "json" | "reasoning";

export type ModelPickerModel = {
  /** Stable key, e.g. `provider/model` or `runtime:<id>/<model>`; also the favorites key. */
  key: string;
  providerId: string;
  name: string;
  /** Compact name for rows and the trigger; the full `name` stays in the tooltip. */
  shortName?: string;
  description?: string;
  contextWindow?: number | null;
  capabilities?: readonly ModelPickerCapability[];
  badge?: "new" | "default" | "custom";
  /** The model is selected somewhere but no longer listed by its provider. */
  unavailable?: boolean;
  /** When set the row is shown dimmed, cannot be chosen, and shows this reason as its tooltip. */
  disabledReason?: string | null;
  /** Older models collapse under a "Legacy models" row in their provider's view. */
  legacy?: boolean;
};
