import { useEffect, useRef, useState } from "react";
import { List, ListRow, Notice } from "@aidex/ui/components";
import { Button, Stack, Text } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import type { ProviderKind } from "@aidex/client/dto/models";
import type { RuntimeConfig } from "@aidex/client/dto/runtime";
import { useProviderCatalog } from "../../catalog/ProviderCatalog";
import { runtimeHealthKind, supportsModelDiscovery } from "../../catalog/runtimes";
import { runtimeSummary } from "../providerItems";

const PREVIEW_COUNT = 6;

export type WizardResultTarget =
  | { kind: "provider"; id: string; title: string; providerKind: ProviderKind }
  | { kind: "runtime"; runtime: RuntimeConfig; title: string };

type Outcome =
  | { state: "checking" }
  | { state: "connected"; count: number | null; names: string[]; note?: string }
  | { state: "failed"; message: string };

/** Step 3: list the new account's models (or check the runtime) and report what works. */
export function ResultStep({
  target,
  onBack,
  onDone,
}: {
  target: WizardResultTarget;
  onBack: () => void;
  onDone: () => void;
}) {
  const catalog = useProviderCatalog();
  const [outcome, setOutcome] = useState<Outcome>({ state: "checking" });
  // The check runs once per target; catalog callbacks are read through a ref so reloads do not re-run it.
  const catalogRef = useRef(catalog);
  catalogRef.current = catalog;

  useEffect(() => {
    let live = true;
    const { refresh, reload, checkRuntime, loadRuntimeModels } = catalogRef.current;
    const check = async (): Promise<Outcome> => {
      if (target.kind === "provider") {
        const listing = await refresh(target.id);
        const failure = listing.failures.find((entry) => entry.provider_id === target.id);
        if (failure) return { state: "failed", message: failure.message };
        // Azure lists only its declared deployments without contacting the server, so ask the
        // server directly. (Bedrock has no free liveness check; its health is always unavailable.)
        if (target.providerKind === "azure-openai") {
          const health = await call("provider.health", { id: target.id });
          if (health.status !== "ready")
            return {
              state: "failed",
              message: health.message || "Azure OpenAI did not answer at this endpoint with this key.",
            };
        }
        const models = listing.models.filter((model) => model.model.provider_id === target.id);
        return {
          state: "connected",
          count: models.length,
          names: models.slice(0, PREVIEW_COUNT).map((model) => model.display_name || model.model.model_id),
        };
      }
      const { runtime } = target;
      reload();
      void checkRuntime(runtime.id);
      const health = await call("runtime.health", { id: runtime.id });
      const summary = runtimeSummary(runtime, { state: "ready", health }, target.title);
      const kind = runtimeHealthKind(health.status);
      if (kind === "binary_missing")
        return { state: "failed", message: summary.detail ?? summary.headline };
      const note =
        kind === "ready"
          ? undefined
          : kind === "auth_required"
            ? "Sign in from the runtime's settings before starting a conversation."
            : summary.detail;
      if (!supportsModelDiscovery(runtime.driver) || !health.installed)
        return { state: "connected", count: null, names: [], note };
      const listed = await call("runtime.models", { id: runtime.id });
      loadRuntimeModels(runtime.id, true);
      return {
        state: "connected",
        count: listed.models.length,
        names: listed.models.slice(0, PREVIEW_COUNT).map((model) => model.name || model.id),
        note,
      };
    };
    check().then(
      (result) => live && setOutcome(result),
      (caught: unknown) =>
        live && setOutcome({ state: "failed", message: toAidexError(caught).message }),
    );
    return () => {
      live = false;
    };
  }, [target]);

  if (outcome.state === "checking")
    return (
      <Notice role="status">
        {target.kind === "provider" ? `Listing ${target.title} models…` : `Checking ${target.title}…`}
      </Notice>
    );

  if (outcome.state === "failed")
    return (
      <Stack gap={4}>
        <Notice tone="danger" role="alert">
          {outcome.message}
        </Notice>
        <Text tone="muted">
          {target.kind === "provider"
            ? "Go back to fix the key or endpoint. The account is saved; you can also fix it later in Providers."
            : "Go back to fix the executable or sign-in. The runtime is saved; you can also fix it later in Providers."}
        </Text>
        <Stack direction="row" gap={2} justify="end">
          <Button variant="ghost" onClick={onDone}>
            Fix later
          </Button>
          <Button variant="primary" onClick={onBack}>
            Back
          </Button>
        </Stack>
      </Stack>
    );

  return (
    <Stack gap={4}>
      <Notice tone="success" role="status">
        {outcome.count === null
          ? `Connected · ${target.title} uses its default model.`
          : `Connected · ${outcome.count} ${outcome.count === 1 ? "model" : "models"}`}
      </Notice>
      {outcome.note && <Notice tone="warning">{outcome.note}</Notice>}
      {outcome.names.length > 0 && (
        <List>
          {outcome.names.map((name) => (
            <ListRow key={name} title={name} />
          ))}
        </List>
      )}
      {outcome.count !== null && outcome.count > outcome.names.length && (
        <Text tone="muted">and {outcome.count - outcome.names.length} more.</Text>
      )}
      <Stack direction="row" justify="end">
        <Button variant="primary" onClick={onDone}>
          Done
        </Button>
      </Stack>
    </Stack>
  );
}
