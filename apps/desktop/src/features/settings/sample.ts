// Sample data for the settings that have no backend yet: planned shortcuts and host residue cleanup
// (issue #28). Diagnostics, Drive, and Updates are live. Nothing here is read from, or written to,
// the drive or the host.

export type Shortcut = {
  id: string;
  action: string;
  /** Chord such as "Mod+Shift+p"; `Mod` is ⌘ on Apple platforms and Ctrl elsewhere. */
  chord: string;
  /** Whether the shortcut already works in this build. */
  wired: boolean;
};

/** Planned shortcuts that no screen handles yet; working ones come from preferences and `lib/shortcuts.ts`. */
export const plannedShortcuts: readonly Shortcut[] = [
  { id: "search", action: "Search", chord: "Mod+f", wired: false },
  { id: "toggle-sidebar", action: "Toggle sidebar", chord: "Mod+b", wired: false },
  { id: "open-settings", action: "Open settings", chord: "Mod+,", wired: false },
];

export type ResidueItem = { id: string; label: string; description: string; sizeMb: number };

export const sampleResidue: readonly ResidueItem[] = [
  { id: "temp", label: "Temporary files", description: "Scratch files left on this computer by past sessions", sizeMb: 120 },
  { id: "model-cache", label: "Model cache", description: "Host-side copies made to speed up model loading", sizeMb: 340 },
  { id: "logs", label: "Logs older than 30 days", description: "Diagnostic logs no longer needed for support", sizeMb: 45 },
];
