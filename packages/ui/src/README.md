# Shared UI source

Shared components for desktop, installer, and mobile belong in `components/`.
Target-specific UI belongs in `desktop/`, `installer/`, or `mobile/`.

Overlays (`Dialog`, `ImageLightbox`, `Menu`, `Tooltip`, `Select`, the toasts, `ScrollArea` and `Collapsible`) wrap `@base-ui/react`, the package's only UI dependency besides `@pierre/diffs`. It is pinned to an exact version (1.4.1) and imported by subpath (`@base-ui/react/dialog`), so only the parts in use ship. The layer stack shared with the custom `Popover` lives in `primitives/layers.ts`.
