# Settings feature

Preferences, diagnostics, and drive health for this AiDEX drive. Route: `settings`, tabs `appearance`, `shortcuts`, `updates`, `diagnostics`, and `drive` (order and labels in `tabs.ts`, shared with the sidebar).

- `SettingsPage` — header and underline tabs; each tab renders one panel.
- `AppearancePanel` — accent color (`SwatchPicker` over `accentPresets`, bound to `usePreferences().accent`, so it re-themes the app live), theme, density, reduce motion, and keyboard hints.
- `ShortcutsPanel` — shortcut table with the platform modifier (`useModifierKeyLabel`). New chat and Open Providers come from `AppPreferences.shortcuts`, the model picker from `MODEL_PICKER_SHORTCUT` (`lib/shortcuts.ts`); the rest are marked planned.
- `UpdatesPanel` — live `update.status`: the running, selected, and previous versions, startup recovery, an unfinished update, and the selected bundle's integrity (signature, critical files, SBOM) with Verify again. "Install a signed package" takes an absolute local path (`updateSelection.ts` refuses URLs, relative paths, and on Windows network shares and device paths such as `\\server\share`, `\\?\`, and `\\.\`), runs `update.check` with `allow_unsigned: false`, and shows the result; Apply… and Roll back… open a confirmation dialog. Apply re-runs `update.check` and refuses when the package, version, manifest, or signer differs from what was reviewed; rollback re-reads `update.status` and refuses when the previous version changed. Results are dropped when the drive or daemon changes (`connectionKey`). A switch takes effect on the next launch. There is no online catalog or release channel.
- `DiagnosticsPanel` — live doctor-equivalent checks (host, drive manifest, AiDEX Core with Start, sandbox backend), system information, Run diagnostics (re-runs every check), an event-log reader (`events.replay` by workspace id), and Export logs (preview).
- `DrivePanel` — the live session drive: volume label, root, filesystem, attachment, health, capacity meter, manifest details, and migration advice. Integrity opens the Updates tab; safe eject (issue #28) is a preview, and host cleanup (`ResidueCleanup`) is labelled sample data.
- `SettingsSidebar` — one item per tab, active by the route's tab.

The accent color is the only preference with a real effect, and it lasts for this session only (preferences are held in memory). Cleanup selections are local state. Planned shortcuts and residue cleanup come from `sample.ts`; actions without a backend (export logs, clean up, safe eject) show a "Preview" toast instead of claiming they happened.

`updateSelection.test.ts` covers the source, apply, and re-check rules (`bun run --cwd apps/desktop test`).
