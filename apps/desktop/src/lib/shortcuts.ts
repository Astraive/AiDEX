// Client-side shortcut constants. `AppPreferences.shortcuts` must not gain new actions (an older
// daemon would reject the stored blob), so shortcuts without a preference key live here.

/** Opens or closes the conversation model picker. */
export const MODEL_PICKER_SHORTCUT = "Mod+Shift+M";
/** Defaults used when `AppPreferences.shortcuts` has no binding for an action. */
export const DEFAULT_PROVIDERS_SHORTCUT = "Mod+Shift+p";
export const DEFAULT_NEW_SESSION_SHORTCUT = "Mod+k";
