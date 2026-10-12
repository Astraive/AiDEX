import { useCallback, useState } from "react";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import type { ModelRef, ProviderAccount, ProviderAddParams } from "@aidex/client/dto/models";
import type { RuntimeConfig } from "@aidex/client/dto/runtime";
import { usePreferences } from "../../../app/providers/preferences";
import { useToast } from "../../../app/providers/toast";
import { useProviderCatalog } from "../catalog/ProviderCatalog";
import { nativeModelKey, parseModelKey } from "../catalog/modelKeys";
import { accountUpdateParams } from "../catalog/providerAccounts";
import { storeRuntimeKey } from "./runtimeSecret";

export type ProviderActions = {
  /** Label of the action in flight, if any. */
  busy: string | null;
  error: string | null;
  clearError: () => void;
  /** Runs `job` with busy and error handling; resolves with its result, or undefined when it failed. */
  run: <T>(label: string, job: () => Promise<T>) => Promise<T | undefined>;
  setAccountEnabled: (account: ProviderAccount, enabled: boolean) => Promise<void>;
  renameAccount: (account: ProviderAccount, name: string | null) => Promise<boolean>;
  removeAccount: (account: ProviderAccount) => Promise<boolean>;
  /** `provider.update` with the full current account plus `changes`. */
  updateAccount: (account: ProviderAccount, changes: Partial<ProviderAddParams>) => Promise<boolean>;
  replaceKey: (account: ProviderAccount, value: string) => Promise<boolean>;
  setDefaultModel: (model: ModelRef) => Promise<void>;
  openPresetLink: (presetId: string, link: "api_key" | "docs") => Promise<void>;
  setRuntimeEnabled: (runtime: RuntimeConfig, enabled: boolean) => Promise<void>;
  updateRuntime: (runtime: RuntimeConfig) => Promise<boolean>;
  /** Stores a vault-auth runtime's key for that runtime's consumer, then checks the runtime again. */
  replaceRuntimeKey: (runtime: RuntimeConfig, value: string) => Promise<boolean>;
  removeRuntime: (runtime: RuntimeConfig) => Promise<boolean>;
};

/**
 * Account and runtime mutations for the providers screen. Each caller gets its own busy and error
 * state, so a failure shows next to the control that caused it. Every success reloads the catalog.
 */
export function useProviderActions(): ProviderActions {
  const catalog = useProviderCatalog();
  const preferences = usePreferences();
  const notify = useToast();
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const run = useCallback(async <T,>(label: string, job: () => Promise<T>) => {
    setBusy(label);
    setError(null);
    try {
      return await job();
    } catch (caught) {
      setError(toAidexError(caught).message);
      return undefined;
    } finally {
      setBusy(null);
    }
  }, []);

  /** A removed account or runtime must not stay the default for new chats. */
  const forgetDefault = (owner: { provider?: string; runtime?: string }) => {
    const parsed = parseModelKey(preferences.modelId);
    if (!parsed) return;
    if (
      (parsed.kind === "native" && parsed.providerId === owner.provider) ||
      (parsed.kind === "runtime" && parsed.runtimeId === owner.runtime)
    )
      preferences.setModelId("");
  };

  const succeeded = <T,>(result: T | undefined) => result !== undefined;

  return {
    busy,
    error,
    clearError: () => setError(null),
    run,
    setAccountEnabled: async (account, enabled) => {
      await run(enabled ? "enable" : "disable", () =>
        call("provider.patch", { id: account.descriptor.id, enabled }),
      );
      catalog.reload();
    },
    renameAccount: async (account, name) =>
      succeeded(
        await run("rename", async () => {
          await call("provider.patch", { id: account.descriptor.id, display_name: name });
          catalog.reload();
          return true;
        }),
      ),
    removeAccount: async (account) =>
      succeeded(
        await run("remove", async () => {
          await call("provider.remove", { id: account.descriptor.id });
          forgetDefault({ provider: account.descriptor.id });
          catalog.reload();
          return true;
        }),
      ),
    updateAccount: async (account, changes) =>
      succeeded(
        await run("update", async () => {
          await call("provider.update", accountUpdateParams(account, changes));
          await catalog.refresh(account.descriptor.id).catch(() => undefined);
          return true;
        }),
      ),
    replaceKey: async (account, value) =>
      succeeded(
        await run("key", async () => {
          const auth = account.descriptor.auth;
          const name = auth.type === "vault_secret" ? auth.secret_name : `provider-${account.descriptor.id}`;
          await call("vault.secret.set", {
            name,
            value,
            consumers: [`provider:${account.descriptor.id}`],
            kind: "api_key",
          });
          // An account that had no key yet starts using the new secret.
          if (auth.type !== "vault_secret")
            await call("provider.update", accountUpdateParams(account, { secret_name: name }));
          const listing = await catalog.refresh(account.descriptor.id);
          const failure = listing.failures.find(
            (entry) => entry.provider_id === account.descriptor.id,
          );
          if (failure) throw new Error(failure.message);
          notify("API key saved.", "success");
          return true;
        }),
      ),
    setDefaultModel: async (model) => {
      await run("default", async () => {
        await call("model.use", model);
        preferences.setModelId(nativeModelKey(model));
        catalog.reload();
      });
    },
    openPresetLink: async (presetId, link) => {
      await run("link", () => call("provider.open_link", { preset_id: presetId, link }));
    },
    setRuntimeEnabled: async (runtime, enabled) => {
      await run(enabled ? "enable" : "disable", () => call("runtime.update", { ...runtime, enabled }));
      catalog.reload();
    },
    updateRuntime: async (runtime) =>
      succeeded(
        await run("update", async () => {
          await call("runtime.update", runtime);
          catalog.reload();
          return true;
        }),
      ),
    replaceRuntimeKey: async (runtime, value) =>
      succeeded(
        await run("key", async () => {
          if (runtime.auth.kind !== "vault") throw new Error("This runtime does not use a vault key.");
          await storeRuntimeKey(runtime.id, runtime.auth.secret_name, value);
          await catalog.checkRuntime(runtime.id);
          notify("API key saved.", "success");
          return true;
        }),
      ),
    removeRuntime: async (runtime) =>
      succeeded(
        await run("remove", async () => {
          await call("runtime.remove", { id: runtime.id });
          forgetDefault({ runtime: runtime.id });
          catalog.reload();
          return true;
        }),
      ),
  };
}

