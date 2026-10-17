# Model manager feature

Providers, local models, and model details, plus the Models sidebar and the shared model catalog that the chat model picker reads. Everything is composed from `@aidex/ui`; all data is live from AiDEX Core.

## Screens

| File | Shows |
| --- | --- |
| `ModelCenterPage.tsx` | Route `models`. Underline tabs bound to the route tab: `providers` (default, "Providers") and `local` ("Local models"). |
| `ProvidersPanel.tsx` | Thin composition of the Providers tab: header with "Add provider" and the "Refresh provider status" button ("Checked …"), the two panes, the add wizard, and the vault footnote. |
| `LocalModelsPanel.tsx` | GGUF import, the verified llama.cpp runtime, start and stop, and `ModelInventory` for local models. Remove asks in a confirmation dialog (Cancel first; a failure shows inside it; the original import is kept), then moves focus to the row that took its place, or to the import field once no model is left (`useRemovalFocus`). |
| `ModelInventory.tsx` | Listed models with brand icon, context, capabilities, and capability source; "Use model" makes one the default for new chats. |
| `ModelDetailsPage.tsx` | Route `model` (`id` = `provider/model`). Provider, limits, tools, capability source, favorite, and "Use this model" (`model.use` plus `preferences.setModelId`). |
| `ModelsSidebar.tsx` | Favorites, then chat models grouped by provider with brand icons, then Manage links (Providers, Local models). |
| `LocalInferenceNotice.tsx` | Loading or failed local model, with Retry. |

## Catalog (`catalog/`)

One `ProviderCatalogProvider` (mounted in `app/Workspace.tsx`) merges `model.list`, `model.providers`, `provider.presets`, `runtime.list`, `runtime.catalog`, `runtime.health`, lazily fetched `runtime.models`, `inference.status`, and `model.preferences.get`. `useProviderCatalog()` returns picker data (`providers`, `models`), the raw reports, `refresh(providerId?)` (`model.list {refresh, provider_id}`), `reload()`, favorites and hidden models, and the default key. Runtime and preference calls fail softly: a locked vault or an older daemon leaves them empty and screens show a calm notice.

| File | Responsibility |
| --- | --- |
| `ProviderCatalog.tsx` | The provider and `useProviderCatalog()`. |
| `buildCatalog.ts` | Pure merge into `ModelPickerProvider[]` and `ModelPickerModel[]`, including provider status mapping. |
| `modelKeys.ts` | Model keys: native `provider/model`, runtime `runtime:<runtime>/<model>`; `parseModelKey`, `sessionModelKey`, `modelTargetOf`. |
| `modelFacts.ts` | Chat eligibility (chat and streaming), capability words, source labels, picker rows. |
| `providerAccounts.ts` | Account title, brand, preset matching, ChatGPT accounts, and full `provider.update` params. |
| `runtimes.ts` | Runtime brands and names, status mapping, Default model and Claude Code alias rows. |
| `useModelPreferences.ts` | Favorites and hidden models with optimistic saves. |
| `useRuntimeState.ts` | Runtime list, health, and per-session model cache. |

Picker status: a disabled account is `disabled`; a listing failure is `error`, or `setup` for missing or rejected credentials; `local-llama` is `ready` only while the local model runs; a ready account without chat models is `warning`. Runtimes map `runtime.health` through `runtimeHealthKind` (`ready`; `*_auth_unverified`, and for a managed account `authenticated_cached` and `auth_pending` → warning; `auth_required`, `signed_out`, and `binary_missing` → setup; an unknown code is a generic warning, never shown raw).

## Providers tab (`providers/`)

Mirrors t3code's Settings → Providers. The selected row lives in the route id (`provider:<id>` or `runtime:<id>`), so it survives refetches and other screens can link to an account.

| File | Responsibility |
| --- | --- |
| `providerItems.ts` | Rows with status copy (Available, Disabled, Not authenticated, Unavailable, Needs attention, Not found) and relative "Checked" times. |
| `ProviderList.tsx` | Left pane: "Model providers" and "Agent runtimes" `ProviderRow`s with enable switches, and the Add provider row. |
| `ProviderEditor.tsx` | Right pane for an account: header, Setup, Connection, Models, Custom models. |
| `EditorHeader.tsx` | Brand, badges, Rename (`provider.patch`), Remove with confirmation. |
| `SetupSection.tsx` | ChatGPT sign-in (Reconnect, Sign out), Replace API key, Get an API key (`provider.open_link`), or key-less local servers. |
| `ConnectionSection.tsx` | Endpoint facts, "Discover models automatically", and "Check connection" (`provider.health`). |
| `ModelsSection.tsx` | Favorites, Shown in picker, and Hidden groups; star, picker switch, Set as default, capability and source tags. |
| `CustomModelsEditor.tsx` | Declared models saved through `provider.update` with the full account. |
| `RuntimeEditor.tsx`, `RuntimeConnectionSection.tsx`, `runtimeEdit.ts` | Runtime sign-in (including Replace API key for vault auth), the Account section, executable, arguments, health, install, and models. The account profile is not edited as text. Connection Save and the enable switch apply only their fields to the configuration read just before the write (`runtime.config.get`, then `runtime.update`; `updateStoredRuntime` in `runtimeEdit.ts`), so an account that a sign-in or import bound meanwhile is kept; when AiDEX Core refuses the write because an account was bound between the read and the write (`conflict`), the edit is applied once more to a fresh read. Save waits while the Account section has an import, sign-in, or sign-out running. Removing a runtime with a saved account signs it out first (`runtime.login.logout`, best effort), and the confirmation says so. |
| `RuntimeAccountSection.tsx` | The runtime's own account, kept encrypted on the drive, for drivers whose `runtime.catalog` `auth_methods` include `native_cli` or `profile_import` (Codex): status from `runtime.account.status` (Signed in means saved credentials plus the CLI's own login check, not proof the account can use a model), Sign in with Codex (`runtime.login.start`, then `runtime.login.status` about every second, one check at a time; three failed checks in a row, a locked vault, or a vanished flow end the sign-in; the sign-in page opens once per flow through `web.open` as soon as the flow reports its address (`runtimeLogin.ts`), outside the sandbox job that runs the CLI, and "Open the sign-in page again" opens it once more; Cancel sign-in, also on leaving the screen, uses `runtime.login.cancel`), import of an `auth.json` path the user types (`runtime.profile.import`; nothing is searched for), and Sign out with confirmation (`runtime.login.logout`). A failed or cancelled sign-in leaves the saved account as it was. Each change reloads the runtime list and checks the runtime again. While the runtime has no saved account and uses the sign-in in Setup (ChatGPT or a vault key), a notice says signing in or importing replaces that sign-in. Other drivers, and a daemon answering `auth_unsupported` or `method_not_found` (an older AiDEX Core), say AiDEX does not manage their account. No credential value reaches the screen. |
| `runtimeSecret.ts` | Stores a runtime's vault key for its `process:runtime-<id>` consumer, keeping the secret's other consumers. |
| `AddProviderWizard.tsx`, `wizard/` | Provider → Connect → Models. Presets by category and search, ChatGPT sign-in or API key, advanced endpoint settings, agent runtimes, and the first model listing. Coming back to fix a runtime the wizard already added saves its executable, arguments, and sign-in onto the configuration as stored now (`updateStoredRuntime`), never the copy the step kept, so an account bound since is not dropped. |
| `useProviderActions.ts` | Mutations with per-section busy and error state. |
| `useAuthFlow.ts` | Browser sign-in: start, open, poll every second, cancel. A start that resolves after a cancel or unmount cancels its flow instead of opening the browser. |

API keys go to the vault (`vault.secret.set` with the account's secret name and `provider:<id>` consumer, or `process:runtime-<id>` for a runtime) and leave React state as soon as they are sent. Removing an account or runtime that holds the default for new chats clears that default.
