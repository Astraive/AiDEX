import type { PermissionMode } from "./sessions";
/** Mirrors aidex-preferences::AppPreferences; stored encrypted on the drive. */
export type AppPreferences = {
  accent: string;
  theme: "dark" | "light" | "system";
  density: "comfortable" | "compact";
  reduce_motion: boolean;
  keyboard_hints: boolean;
  model_id: string;
  workspace_id: string;
  permission_mode: PermissionMode | null;
  instructions: string;
  fallback_model: string | null;
  max_steps: number;
  max_tokens: number;
  shortcuts: Record<string, string>;
};

/** Mirrors aidex-preferences::ModelPreferences (`model.preferences.get|set`), stored apart from AppPreferences. */
export type ModelPreferences = {
  /** Model keys pinned in the model picker: `provider/model` or `runtime:<runtime>/<model>`. */
  favorite_models: string[];
  /** Model keys hidden from the model picker. */
  hidden_models: string[];
};
