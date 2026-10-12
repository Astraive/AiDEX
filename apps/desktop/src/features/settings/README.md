# Settings feature

Preferences, diagnostics, and drive health for this AiDEX drive. Route: `settings`, tabs `appearance`, `shortcuts`, `updates`, `diagnostics`, and `drive` (order and labels in `tabs.ts`, shared with the sidebar).

- `SettingsPage` — header and underline tabs; each tab renders one panel.
- `AppearancePanel` — accent color (`SwatchPicker` over `accentPresets`, bound to `usePreferences().accent`, so it re-themes the app live), theme, density, reduce motion, and keyboard hints.
- `ShortcutsPanel` — shortcut table with the platform modifier (`useModifierKeyLabel`). New chat and Open Providers come from `AppPreferences.shortcuts`, the model picker from `MODEL_PICKER_SHORTCUT` (`lib/shortcuts.ts`); the rest are marked planned.
- `UpdatesPanel` — version, release channel, check for updates, and how updates are applied (staged, verified, switched atomically, rolled back on failure).
- `DiagnosticsPanel` — live doctor-equivalent checks (host, drive manifest, AiDEX Core with Start, sandbox backend), system information, Run diagnostics (re-runs every check), an event-log reader (`events.replay` by workspace id), and Export logs (preview).
- `DrivePanel` — the live session drive: volume label, root, filesystem, attachment, health, capacity meter, manifest details, and migration advice. Integrity verification (issue #27) and safe eject (issue #28) are previews, and host cleanup (`ResidueCleanup`) is labelled sample data.
- `SettingsSidebar` — one item per tab, active by the route's tab.

The accent color is the only preference with a real effect, and it lasts for this session only (preferences are held in memory). Switches, the channel, and cleanup selections are local state. Planned shortcuts, updates, and residue cleanup come from `sample.ts`; actions without a backend (check for updates, export logs, verify, clean up, safe eject) show a "Preview" toast instead of claiming they happened.
