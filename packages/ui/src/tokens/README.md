# Design tokens

`tokens.css` defines the shared color, elevation, typography, spacing, radius, motion, and chart tokens as `--aidex-*` custom properties. The accent family derives from `--aidex-color-accent` with `color-mix()`, so changing that one token re-themes focus rings, tints, and borders. `../styles.css` imports it alongside the base element styles.

`--aidex-color-link` is the accent mixed 75/25 with `--aidex-color-text`, so link text stays readable on both themes (lighter on dark, darker on light) and still follows a runtime accent change.

Theme-dependent values are tokens too, overridden for `html[data-theme="light"]` in `../styles.css`: `--aidex-color-shadow` (floating-surface shadow, derived from text in the light theme so it stays visible) and `--aidex-brand-color-strength` (how much brand color `BrandIcon` keeps before mixing with black; 78% in the light theme).
