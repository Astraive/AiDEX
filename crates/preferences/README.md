# Portable encrypted preferences

`aidex-preferences` keeps user preferences in the drive vault, never in plain
files. Each record is one vault secret readable only by the consumer
`process:aidex-preferences`, so every read and write needs the vault unlocked; a
locked vault is reported as the vault's own error, never as empty preferences.

| Record             | Vault secret              | Service                   | Daemon methods                         |
| ------------------ | ------------------------- | ------------------------- | -------------------------------------- |
| `AppPreferences`   | `aidex-preferences`       | `PreferencesService`      | `settings.get`, `settings.set`         |
| `ModelPreferences` | `aidex-model-preferences` | `ModelPreferencesService` | `model.preferences.get`, `model.preferences.set` |

Both records are closed schemas (`deny_unknown_fields`). A record that was never
saved reads as its defaults.

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

## Compatibility

Neither record may gain fields casually. `update.rollback` can restart an older
daemon on the same drive, and that daemon rejects a record with fields it does
not know. That is why the model picker preferences are a second record instead
of new `AppPreferences` fields, and why the shortcut action list is not extended
for client-side shortcuts such as the model picker's.
