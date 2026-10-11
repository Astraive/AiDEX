# Settings feature

Preferences, diagnostics, and drive health for this AiDEX drive. Route: `settings`, tabs `appearance`, `shortcuts`, `updates`, `diagnostics`, and `drive` (order and labels in `tabs.ts`, shared with the sidebar).

- `SettingsPage` — header and underline tabs; each tab renders one panel.
- `AppearancePanel` — accent color (`SwatchPicker` over `accentPresets`, bound to `usePreferences().accent`, so it re-themes the app live), theme, density, reduce motion, and keyboard hints.
- `ShortcutsPanel` — shortcut table with the platform modifier (`useModifierKeyLabel`); only New session works today, the rest are marked planned.
- `UpdatesPanel` — actual update.status/check/apply/rollback data, mandatory signatures, explicit absolute manual source paths, candidate review, self-test results, recorded rollback, and next-launch requirements. There is no online catalog/channel or unsigned bypass. Installed verification reports the backend's critical-file scope; source checking verifies the signed package.
- `DiagnosticsPanel` — live doctor-equivalent checks (host, drive manifest, AiDEX Core with Start, sandbox backend), system information, Run diagnostics (re-runs every check), an event-log reader (`events.replay` by workspace id), and Export logs (preview).
- `DrivePanel` — the live session drive: volume label, root, filesystem, attachment, health, capacity meter, manifest details, and migration advice. Integrity links to the live Updates verifier; safe eject and host cleanup (`ResidueCleanup`) remain labelled previews.
- `SettingsSidebar` — one item per tab, active by the route's tab.

Preferences are saved through encrypted Core settings. Updates always require a deliberate source check followed by user-controlled apply or rollback; checking does not switch the selected bundle. Core re-verifies and self-tests before an update pointer switch. Shortcuts retain planned actions, and residue cleanup/safe eject remain labelled previews; no online release catalog is inferred.
