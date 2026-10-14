// Edits the Providers screen makes to a saved runtime: the Connection fields and the enable switch.
// `runtime.update` replaces the whole stored configuration, its account `profile` and `auth`
// included, and AiDEX Core binds those itself when a sign-in or an import finishes. So an edit is
// applied to the configuration read just before the write (`runtime.config.get`), never to the
// screen's cached copy, which can predate a binding that just completed.
import type { RuntimeConfig } from "@aidex/client/dto/runtime";

/** What the screen changes: the launch settings, or whether the runtime is enabled. */
export type RuntimeEdit = { executable: string; argv: string[] } | { enabled: boolean };

/** The stored configuration with `edit` applied; every other field stays as stored. */
export function applyRuntimeEdit(stored: RuntimeConfig, edit: RuntimeEdit): RuntimeConfig {
  return "enabled" in edit
    ? { ...stored, enabled: edit.enabled }
    : { ...stored, executable: edit.executable, argv: [...edit.argv] };
}
