# Portable encrypted preferences

`aidex-preferences` keeps user preferences in the drive vault, never in plain
files. Each record is one vault secret readable only by the consumer
`process:aidex-preferences`, so every read and write needs the vault unlocked; a
locked vault is reported as the vault's own error, never as empty preferences.

| Record             | Vault secret              | Service                   | Daemon methods                         |
| ------------------ | ------------------------- | ------------------------- | -------------------------------------- |
| `AppPreferences`   | `aidex-preferences`       | `PreferencesService`      | `settings.get`, `settings.set`         |
| `ModelPreferences` | `aidex-model-preferences` | `ModelPreferencesService` | `model.preferences.get`, `model.preferences.set` |
| `Appearance`       | `aidex-appearance`        | `AppearanceService`       | `appearance.get`, `appearance.set`     |

`AppPreferences` and `ModelPreferences` are closed schemas (`deny_unknown_fields`);
the appearance record is read leniently (below). A record that was never saved
reads as its defaults.

## Application preferences

`AppPreferences` holds the accent colour, theme, density, motion and keyboard-hint
switches, the composer's model and project choice, permission mode, custom
instructions, fallback model, agent step and token limits, and keyboard shortcuts
for a fixed set of actions. `PreferencesService::update` merges a top-level patch
into the saved record and validates the result; an unknown field or an invalid
value changes nothing.

## Model picker preferences

`ModelPreferences { favorite_models, hidden_models }` lists model keys: either
`provider/model` for a native model or `runtime:<runtime_id>/<model>` for an
external runtime's model. A key is 3-512 bytes, contains `/`, and has no control
characters (`is_model_key`). Duplicates are dropped keeping the first occurrence,
then at most 64 favorites and 512 hidden keys are accepted.
`ModelPreferencesService::set` takes a `ModelPreferencesPatch`: a list that is
omitted (or `null`) keeps its saved value.

## Appearance

`Appearance` holds the interface scale (`ui_scale`, 75-125 percent, default 100),
`density` (compact, comfortable, or spacious), `chat_width` (narrow, comfortable,
wide, or full), `chat_font_size` (12-20 px, default 14), `code_font_size` (10-18
px, default 13), `sidebar_width` (208-480 px, or `None` for the shell default),
`sidebar_auto_collapse` (default on), and `message_spacing` (compact,
comfortable, or relaxed).

- **The patch is strict.** `AppearancePatch` refuses unknown fields and unknown
  values when it is parsed, and `validate()` refuses numbers outside their range;
  either way nothing is saved. An omitted field stays as saved, and
  `sidebar_width: null` resets the width.
- **The record is read leniently.** Numbers are clamped, unknown values and wrong
  types read as their defaults, and unreadable bytes read as all defaults. Fields
  a newer version wrote are kept and written back (at most 32 of them in 4 KiB),
  so a later version can add options to this record instead of adding another
  one. They are never returned.
- **Density is split across two records.** `AppPreferences.density` (compact or
  comfortable) stays the base that every version reads and writes; this record
  only saves whether comfortable is refined to spacious. A compact base always
  wins. `AppearanceService::set` writes the base through the `PreferencesService`
  it is given (only when it changes), then this record; pass the instance that
  serves `settings.*`, so both writes share its lock. Locks are taken appearance
  first, preferences second. If either write fails the error is returned and the
  density in effect is still one the user chose before or after.
- Nothing is written when a patch changes nothing, which saves a vault commit on
  a removable drive.

After `update.rollback` an older app shows the nearest density (spacious reads as
comfortable). If it switches to compact, compact stays after the update; if it
then switches back to comfortable, spacious returns.

## Compatibility

Neither closed record may gain fields casually. `update.rollback` can restart
an older daemon on the same drive, and that daemon rejects a record with fields
it does not know. That is why the model picker preferences are a second record
instead of new `AppPreferences` fields, why appearance is a third record with a
lenient reader, and why the shortcut action list is not extended for client-side
shortcuts such as the model picker's or the zoom keys.

`tests/compat.rs` freezes the shape an older daemon accepts (`AppPreferencesV1`
and `tests/fixtures/app-preferences-v1.json`) and checks every record the
services write against it. It fails when `AppPreferences` gains a field or a
variant: move such a field to its own record instead of re-freezing the shape.
