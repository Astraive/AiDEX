# Desktop UI

Desktop-specific frames: the `AppShell` workspace frame, the `NavRail` icon navigation, the `Sidebar` panel with its sections and items, the main-region `TopBar` (with `TopBarStatus` for a status dot and its details panel), the `PageFrame` page body, the `DockLayout` for pages with a docked composer, and the `FocusLayout` for launch and setup flows. Keep cross-device components in `../components/`.

## Shell sizing

The frame is fixed at every density: an 8px pad and gap (`--aidex-shell-pad`, `--aidex-shell-gap`), a 52px rail (`--aidex-rail-width`) and a 256px sidebar (`--aidex-sidebar-width`, 208–480px). `sidebarLayout.ts` holds the same numbers for code: the sidebar docks while the window is at least `dockMinViewport(width)` = 84 + width + 640px wide (980px at 256). With `sidebar_auto_collapse` on (the default) main therefore never gets narrower than `--aidex-main-min-width`; with it off the sidebar stays docked and main takes what is left.

- **Docked** (`sidebarMode="docked"`): the sidebar sits beside main and collapses with a width transition (`--aidex-duration-panel`); a closed sidebar stays mounted, hidden and inert. With `onSidebarResizeEnd` the shell adds a resize separator (`role="separator"`, 16px hit area on the gap): drag, Arrow keys (8px, Shift 32px), Home/End, double-click to reset. The width shows inline during the interaction and is saved once, on release.
- **Overlay** (`sidebarMode="overlay"`): the sidebar floats over main past the rail (`--aidex-z-sidebar-overlay`) above a scrim. Main is inert while it is open, focus moves into it, and it closes on a scrim press (a mouse press at once, a touch or pen tap when it lifts, so the mouse events that follow the tap still reach the scrim) or on Escape when it is the top layer (a popover or dialog opened from it closes first). Focus then returns to `sidebarToggleRef`. It registers in the layer stack (`data-aidex-layer`) and has no transform at rest.
- Focus left inside a closing sidebar returns to `sidebarToggleRef`.

`main` is the `aidex-main` inline-size container (`@container aidex-main (max-width: 640px)` narrows page padding), `position: relative`, with a stable 6px scrollbar gutter. The rail scrolls in short windows with fades at a hidden edge, and keeps the current destination in view; its tooltips sit on the right. `AppShell` mounts one `TooltipProvider` (default 600ms delay) and the rail its own 150ms group, so moving along the rail or a toolbar opens the next tooltip at once.

`PageFrame` widths are 896 / 1024 / 1152px (`narrow` / `default` / `wide`) with the page padding and gap tokens.
