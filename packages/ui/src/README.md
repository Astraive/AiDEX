# Shared UI source

Shared components for desktop, installer, and mobile belong in `components/`.
Target-specific UI belongs in `desktop/`, `installer/`, or `mobile/`.

Overlays (`Dialog`, `CommandPalette`, `ImageLightbox`, `Menu`, `Tooltip`, `Select`, the toasts, `ScrollArea` and `Collapsible`) wrap `@base-ui/react`, the package's only UI dependency besides `@pierre/diffs`. It is pinned to an exact version (1.4.1) and imported by subpath (`@base-ui/react/dialog`), so only the parts in use ship. The layer stack shared with the custom `Popover` lives in `primitives/layers.ts`.

Sizes come from the `--aidex-*` tokens in `tokens/tokens.css` (px at the comfortable density); `tokens/README.md` lists them with the density and appearance tables, and `themes/appearance.ts` writes the appearance attributes and the chat, code and sidebar sizes onto `<html>`.

Pure logic sits in its own module, so it runs in unit tests and during static export. Tested with `bun test src`: `desktop/sidebarLayout.ts` (when the sidebar docks, its width limits and key steps), `components/content/fileTreeKeys.ts` (the `FileTree` keyboard model), `primitives/segmentedSelection.ts`, `primitives/selectEvent.ts` and `themes/appearance.ts`, plus the layer stack in `primitives/layers.ts`, the `overflowEdges` helper in `utils/useOverflowEdges.ts`, and the token checks in `tokens/tokens.test.ts`. `components/CommandMenu/model.ts` and `components/ModelPicker/search.ts` / `view.ts` are pure too.

Two internal hooks are not exported from any entry point:

- `utils/useOverflowEdges.ts`: `useOverflowEdges(ref, axis)` sets `data-overflow-start` / `data-overflow-end` on a scroll container while content is cut off at that edge, so its CSS can fade there. `Tabs`, `NavRail`, `Sidebar` and the `Composer` toolbar use it.
- `components/CommandMenu/useTextCut.ts`: `useTextCut(rootId, classNames)` tells whether text inside a row ends in an ellipsis, so `CommandMenu`, `CommandPalette` and `ModelPicker` rows show their full-text tooltip only then.
