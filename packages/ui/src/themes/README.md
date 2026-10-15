# Themes

`accents.ts` defines the accent presets and `applyAccent`, which sets `--aidex-color-accent` at runtime. The accent family (border, soft, ring, focus) is derived from that one token in `../tokens/tokens.css`. The dark theme is the default; `html[data-theme="light"]` in `../styles.css` switches to the light one.

`appearance.ts` applies the sizing and layout options. It does not depend on `@aidex/client`; the app maps its stored appearance record to `AppearanceTokens`.

- `appearanceAttributes(t)` returns the `<html>` dataset values: `uiDensity` is always set, while `chatWidth` and `messageSpacing` are null (absent) at their comfortable default.
- `appearanceVariables(t)` returns `--aidex-text-chat`, `--aidex-text-code`, `--aidex-code-line-height` (round(size × 1.54) px), and `--aidex-sidebar-width` (null removes it, so the shell default applies). Sizes are clamped to 12–20, 10–18 and 208–480 px.
- `applyAppearance(t, root = document.documentElement)` writes those dataset keys and properties one at a time. It never replaces `style.cssText`, so the accent set by `applyAccent` on the same element survives.

Interface scale is not a token: the desktop host sets it as native webview zoom. The density, chat-width and message-spacing blocks that read these attributes are in `../styles.css`; `../tokens/README.md` lists their values.
