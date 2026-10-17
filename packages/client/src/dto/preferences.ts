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

/** Spacing and control sizes; `spacious` exists only in the appearance record. */
export type UiDensity = "compact" | "comfortable" | "spacious";
/** Widest the chat transcript and composer grow (see `CHAT_WIDTH_PX`). */
export type ChatWidth = "narrow" | "comfortable" | "wide" | "full";
/** Space between messages and between their paragraphs. */
export type MessageSpacing = "compact" | "comfortable" | "relaxed";

/**
 * Mirrors aidex-preferences::Appearance (`appearance.get|set`), vault record `aidex-appearance`.
 * It is kept apart from AppPreferences so older daemons are unaffected; `density` is resolved
 * against `AppPreferences.density`, which stays compact or comfortable.
 */
export type Appearance = {
  /** Whole-window zoom in percent, 75-125. */
  ui_scale: number;
  density: UiDensity;
  chat_width: ChatWidth;
  /** Chat message text, CSS px 12-20. */
  chat_font_size: number;
  /** Code, diff, and terminal text, CSS px 10-18. */
  code_font_size: number;
  /** CSS px 208-480, or null for the shell default. */
  sidebar_width: number | null;
  /** Whether the sidebar becomes an overlay in narrow windows. */
  sidebar_auto_collapse: boolean;
  message_spacing: MessageSpacing;
};

/** `appearance.set` params: omitted fields stay, `null` resets `sidebar_width`; unknown fields are refused. */
export type AppearancePatch = Partial<Appearance>;

/** What `appearance.get` returns when nothing was saved. */
export const APPEARANCE_DEFAULTS: Appearance = {
  ui_scale: 100,
  density: "comfortable",
  chat_width: "comfortable",
  chat_font_size: 14,
  code_font_size: 13,
  sidebar_width: null,
  sidebar_auto_collapse: true,
  message_spacing: "comfortable",
};

/** Inclusive ranges `appearance.set` accepts; anything outside is `invalid_params`. */
export const APPEARANCE_LIMITS = {
  ui_scale: { min: 75, max: 125 },
  chat_font_size: { min: 12, max: 20 },
  code_font_size: { min: 10, max: 18 },
  sidebar_width: { min: 208, max: 480 },
} as const;

/** Interface scale steps offered in Appearance and by the zoom keys, in percent. */
export const UI_SCALE_STEPS = [75, 80, 90, 100, 110, 125] as const;

/** Chat lane width in CSS px for each `chat_width`; `null` fills the main area. */
export const CHAT_WIDTH_PX: Record<ChatWidth, number | null> = {
  narrow: 640,
  comfortable: 736,
  wide: 1152,
  full: null,
};
