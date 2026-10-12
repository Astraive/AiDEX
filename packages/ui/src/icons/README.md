# UI icons

Project-owned SVG React icons built with `createIcon`: 24x24 strokes that inherit `currentColor` and are hidden from assistive technology unless given a `title`. Files group them by purpose (`navigation`, `actions`, `objects`, `interface`, `system`, `development`). `LogoMark` is the AiDEX mark. Keep Tauri application icons under each app.

`brands/` holds provider and runtime logos: `brandGlyphs.ts` is generated path data (see `brands/NOTICE.md`; do not edit it by hand) and `BrandIcon` renders it, falling back to a monogram for unknown keys.
