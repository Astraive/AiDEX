// Edits the desktop makes to a saved runtime: the Connection fields, the enable switch, and the add
// wizard's connection step when the user comes back to fix a runtime it saved. `runtime.update`
// replaces the whole stored configuration, its account `profile` included, and AiDEX Core binds
// that profile itself when a sign-in or an import finishes. So an edit is applied to the
// configuration read just before the write (`runtime.config.get`), never to a screen's cached copy,
// which can predate a binding that just completed. AiDEX Core refuses (`conflict`) a save that would
// drop or change the profile of a signed-in account; when a binding lands between the read and the
// write, the edit is applied once more to a fresh read.
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import type { RuntimeAuth, RuntimeConfig } from "@aidex/client/dto/runtime";

/**
 * What a screen changes: the launch settings, whether the runtime is enabled, and how it signs in.
 * The id, the driver, and the account `profile` are never edited.
 */
export type RuntimeEdit = {
  executable?: string;
  argv?: readonly string[];
  enabled?: boolean;
  auth?: RuntimeAuth;
};

/** The stored configuration with `edit` applied; every other field stays as stored. */
export function applyRuntimeEdit(stored: RuntimeConfig, edit: RuntimeEdit): RuntimeConfig {
  return {
    ...stored,
    executable: edit.executable ?? stored.executable,
    argv: [...(edit.argv ?? stored.argv)],
    enabled: edit.enabled ?? stored.enabled,
    auth: edit.auth ?? stored.auth,
  };
}

/** The daemon calls an edit makes; injectable so the read-then-write rule is tested without Core. */
export type RuntimeCalls = {
  get: (id: string) => Promise<RuntimeConfig>;
  update: (config: RuntimeConfig) => Promise<RuntimeConfig>;
};

const daemonCalls: RuntimeCalls = {
  get: (id) => call("runtime.config.get", { id }),
  update: (config) => call("runtime.update", config),
};

/**
 * AiDEX Core refused the save because the stored account profile changed under it (`conflict` with
 * `data.reason = "conflict"`). A busy runtime (`runtime_busy`, also `conflict`) is not this.
 */
export function accountChangedMeanwhile(caught: unknown): boolean {
  const error = toAidexError(caught);
  const reason = (error.data as { reason?: unknown } | null | undefined)?.reason;
  return error.code === "conflict" && reason === "conflict";
}

/**
 * Applies `edit` to runtime `id` as stored now and saves it. A save refused because a sign-in or
 * import bound an account between the read and the write is retried once on a fresh read; a second
 * refusal is the caller's to show.
 */
export async function updateStoredRuntime(
  id: string,
  edit: RuntimeEdit,
  calls: RuntimeCalls = daemonCalls,
): Promise<RuntimeConfig> {
  const save = async () => calls.update(applyRuntimeEdit(await calls.get(id), edit));
  try {
    return await save();
  } catch (caught) {
    if (!accountChangedMeanwhile(caught)) throw caught;
    return save();
  }
}
