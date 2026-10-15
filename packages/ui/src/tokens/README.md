# Design tokens

`tokens.css` defines the shared color, elevation, typography, spacing, sizing, layout, stacking, motion, and chart tokens as `--aidex-*` custom properties on `:root`. Values are px at the comfortable density. `../styles.css` imports it alongside the base element styles, the light theme, the density blocks, and the appearance attribute blocks.

The accent family derives from `--aidex-color-accent` with `color-mix()`, so changing that one token re-themes focus rings, tints, and borders.

`--aidex-color-link` is the accent mixed 75/25 with `--aidex-color-text`, so link text stays readable on both themes (lighter on dark, darker on light) and still follows a runtime accent change.

Theme-dependent values are tokens too, overridden for `html[data-theme="light"]` in `../styles.css`: `--aidex-color-shadow` (floating-surface shadow, derived from text in the light theme so it stays visible) and `--aidex-brand-color-strength` (how much brand color `BrandIcon` keeps before mixing with black; 78% in the light theme).

`tokens.test.ts` checks that every `var(--aidex-*)` in `packages/ui/src` is defined here, in `../styles.css`, or by the runtime setters in `../themes`, and freezes the color, chart, font, and brand declarations. Run it with `bun test src` from `packages/ui`.

## Sizing tokens

| Group | Tokens (comfortable) |
| --- | --- |
| Text | `text-3xs` 10, `text-2xs` 11, `text-xs` 12, `text-label` 13, `text-sm` 14, `text-md` 14 (body), `text-lg` 16, `text-xl` 20, `text-2xl` 24, `text-display` 30 |
| Line height | `leading-body` 1.42857 (unitless, 20px at 14px), `leading-xs` 16px, `leading-sm` 20px, `leading-md` 20px, `leading-relaxed` 1.625 |
| Reading text | `text-chat` 14, `text-code` 13, `code-line-height` 20, `prose-line-height` 1.625, `prose-block-gap` 12, `transcript-gap` 20 |
| Spacing | `space-1` … `space-12` = 4 … 48 (N × 4), half steps `space-0-5` 2, `space-1-5` 6, `space-2-5` 10, `space-3-5` 14 |
| Radius | `radius-xs` 4, `radius-sm` 6, `radius-md` 10, `radius-lg` 12, `radius-xl` 16, `radius-2xl` 22, `radius-full` 999 |
| Controls | `control-xs` 24, `control-sm` 28, `control-md` 32, `control-lg` 36, `control-xl` 40; `control-pad-x` 12, `-pad-x-sm` 10, `-pad-x-xs` 8; `control-gap` 8, `-gap-sm` 6, `-gap-xs` 4 |
| Rows | `row-sm` 28, `row-md` 32, `row-lg` 44 |
| Icons | `icon-xs` 12, `icon-sm` 14, `icon-md` 16, `icon-lg` 18, `icon-xl` 20 |
| Small parts | `badge-height` 20, `badge-height-sm` 16, `kbd-height` 20, `switch-width` 32, `switch-height` 18, `switch-thumb` 14, `checkbox-size` 16, `touch-target` 44 (only for `@media (pointer: coarse)` hit areas), `scrollbar-size` 6 |
| Shell | `shell-pad` 8, `shell-gap` 8, `rail-width` 52, `sidebar-width` 256, `sidebar-min-width` 208, `sidebar-max-width` 480, `topbar-height` 48, `main-min-width` 640 |
| Pages | `page-max-width` 896, `-wide` 1024, `-full` 1152; `page-pad-top` 24, `page-pad-x` 24, `page-pad-x-narrow` 16, `page-pad-bottom` 48, `page-gap` 24 |
| Chat | `chat-max-width` 736, `chat-lane-pad` 24, `chat-lane-pad-narrow` 12, `composer-editor-max` min(40vh, 208px) |
| Floating surfaces | `popover-offset` 4, `popover-min-width` 160, `tooltip-max-width` 320, `dialog-width-md` 512, `-lg` 720, `-xl` 960, `toast-max-width` 360, `popup-shadow` 0 16px 40px -18px shadow |
| Motion | `duration-fast` 120ms, `duration-base` 180ms, `duration-panel` 180ms (0ms when motion is reduced), `ease` |

Body text is `text-md` with `line-height: var(--aidex-leading-body)`. Keep that leading unitless: many rules set only a font size and inherit it, and a px value would make larger headings overlap. The px `leading-*` steps are for rules whose own font size is fixed.

The shell frame (`shell-pad`, `shell-gap`) is the same at every density, and so are text, icons, and radii.

## Density

`../styles.css` remaps these tokens. Comfortable is the `:root` default. `html[data-ui-density]` is written by the appearance settings; `html[data-density="compact"]:not([data-ui-density])` is the legacy fallback from the base preferences, so compact applies before the appearance record loads.

| Token | Compact | Comfortable | Spacious |
| --- | --- | --- | --- |
| `space-3` / `4` / `5` / `6` / `7` / `8` | 10 / 12 / 16 / 20 / 24 / 28 | 12 / 16 / 20 / 24 / 28 / 32 | 14 / 20 / 24 / 28 / 32 / 36 |
| `control-xs` / `sm` / `md` / `lg` / `xl` | 20 / 24 / 28 / 32 / 36 | 24 / 28 / 32 / 36 / 40 | 28 / 32 / 36 / 40 / 44 |
| `row-sm` / `md` / `lg` | 24 / 28 / 40 | 28 / 32 / 44 | 32 / 36 / 52 |
| `control-pad-x` / `-sm` / `-xs` | 10 / 8 / 6 | 12 / 10 / 8 | 14 / 12 / 10 |
| `page-pad-top` / `page-gap` | 20 / 20 | 24 / 24 | 32 / 32 |

## Appearance attributes

| Attribute on `<html>` | Values | Tokens |
| --- | --- | --- |
| `data-chat-width` | `narrow` 640px, (absent) 736px, `wide` 1152px, `full` 100% | `chat-max-width` |
| `data-message-spacing` | `compact`, (absent) comfortable, `relaxed` | see below |
| `data-reduce-motion="true"` | | `duration-panel` 0ms (also under `prefers-reduced-motion`) |

| Token | Compact | Comfortable | Relaxed |
| --- | --- | --- | --- |
| `prose-line-height` | 1.5 | 1.625 | 1.75 |
| `prose-block-gap` | 8px | 12px | 16px |
| `transcript-gap` | 12px | 20px | 28px |

`applyAppearance` in `../themes/appearance.ts` writes these attributes and the inline `text-chat`, `text-code`, `code-line-height` (round(size × 1.54)), and `sidebar-width` properties.

## Stacking order

| Token | Value | Used by |
| --- | --- | --- |
| `z-overlay` | 40 | drop overlays |
| `z-sidebar-overlay` | 45 | the sidebar in overlay mode |
| `z-dialog` | 50 | dialogs |
| `z-dialog-media` | 60 | the image lightbox |
| `z-toast` | 100 | toasts |
| `z-popover` | 130 | popovers, menus, selects, CommandMenu, ModelPicker |
| `z-tooltip` | 140 | tooltips |

## Breakpoints

Media and container queries cannot read custom properties, so these constants are written as literals where they are used:

- `@container aidex-main (max-width: 640px)`: main content (the shell's `.content`) switches to `page-pad-x-narrow` and `chat-lane-pad-narrow`.
- Settings rows: a 512px row container; below it the control stacks under the label.
- Composer: a 520px container; below it the toolbar hides its text labels.
- Dock threshold: `rail-width` + 4 × 8 (shell padding and gaps) + sidebar width + `main-min-width` = 52 + 32 + 256 + 640 = 980px at the default width. Below it the sidebar becomes an overlay.
- Interface scale steps: 75 / 80 / 90 / 100 / 110 / 125%. It is native webview zoom, so media and container queries see the zoomed CSS viewport.
- Hard-floor CSS viewport: 640 × 448 (an 800 × 560 window at 125%).

## Data attributes

These are fixed names, not tokens:

- `data-aidex-portal-root` marks a Dialog popup, so popovers opened inside it portal into it.
- `data-aidex-layer` marks a dismissable layer (popover, menu, overlay sidebar) for the Escape order.
