import { useEffect, useMemo, useState } from "react";
import { Notice, SectionHeader } from "@aidex/ui/components";
import { LockIcon, PlusIcon, RefreshIcon } from "@aidex/ui/icons";
import { Button, Split, Stack, Text } from "@aidex/ui/primitives";
import { ErrorKind, toAidexError } from "@aidex/client/errors";
import { useNavigation } from "../../app/providers/navigation";
import { useProviderCatalog } from "./catalog/ProviderCatalog";
import { AddProviderWizard } from "./providers/AddProviderWizard";
import { ProviderEditor } from "./providers/ProviderEditor";
import { ProviderList } from "./providers/ProviderList";
import { buildProviderItems, lastCheckedAt, relativeTime } from "./providers/providerItems";
import { parseProviderSelection } from "./providers/selection";

/** Providers tab: the account and runtime list beside the selected row's editor, and the add wizard. */
export function ProvidersPanel() {
  const catalog = useProviderCatalog();
  const { route, replace } = useNavigation();
  const [adding, setAdding] = useState(false);
  const [refreshError, setRefreshError] = useState<string | null>(null);
  const now = useNow(30_000);

  const items = useMemo(
    () =>
      buildProviderItems({
        accounts: catalog.accounts,
        presets: catalog.presets,
        modelList: catalog.modelList,
        runtimes: catalog.runtimes,
        drivers: catalog.runtimeDrivers,
        runtimeHealth: catalog.runtimeHealth,
      }),
    [catalog.accounts, catalog.presets, catalog.modelList, catalog.runtimes, catalog.runtimeDrivers, catalog.runtimeHealth],
  );
  const all = [...items.accounts, ...items.runtimes];
  const requested = parseProviderSelection(route.id);
  const selected =
    (requested && all.find((item) => item.kind === requested.kind && item.id === requested.id)) || all[0];
  const select = (selectionId: string) => replace({ page: "models", tab: "providers", id: selectionId });

  const checkedAt = lastCheckedAt(catalog.accounts);
  const checkedText = catalog.refreshing
    ? "Refreshing providers"
    : `Checked ${checkedAt ? (relativeTime(checkedAt, now) ?? "unavailable") : "unavailable"}`;
  const refreshAll = async () => {
    setRefreshError(null);
    try {
      await catalog.refresh();
    } catch (caught) {
      setRefreshError(toAidexError(caught).message);
    }
  };

  const runtimesNotice = runtimeNotice(catalog.runtimesError);

  return (
    <Stack gap={5}>
      <SectionHeader
        title="Providers"
        actions={
          <Stack direction="row" gap={2} align="center">
            <Button
              variant="ghost"
              icon={<RefreshIcon />}
              aria-label="Refresh provider status"
              title="Refresh provider status"
              disabled={catalog.refreshing}
              onClick={() => void refreshAll()}
            >
              {checkedText}
            </Button>
            <Button variant="primary" icon={<PlusIcon />} onClick={() => setAdding(true)}>
              Add provider
            </Button>
          </Stack>
        }
      />
      {(catalog.error || refreshError) && (
        <Notice tone="danger" role="alert">
          {refreshError ?? catalog.error}
        </Notice>
      )}
      <Split emphasis="end" divided>
        <ProviderList
          accounts={items.accounts}
          runtimes={items.runtimes}
          selectedId={selected?.selectionId ?? null}
          onSelect={select}
          onAdd={() => setAdding(true)}
          runtimesNotice={runtimesNotice}
        />
        <ProviderEditor item={selected} onAdd={() => setAdding(true)} />
      </Split>
      <Stack direction="row" gap={2} align="center">
        <LockIcon size={16} />
        <Text tone="muted">Credentials are encrypted in the drive vault and excluded from model prompts.</Text>
      </Stack>
      <AddProviderWizard
        open={adding}
        onClose={() => setAdding(false)}
        onDone={select}
        runtimesNotice={runtimesNotice}
      />
    </Stack>
  );
}

/** Calm wording for a runtime list that could not be read. */
function runtimeNotice(error: ReturnType<typeof useProviderCatalog>["runtimesError"]): string | null {
  if (!error) return null;
  if (error.kind === ErrorKind.methodNotFound)
    return "Agent runtimes need a newer AiDEX Core. Update AiDEX to add Codex, Claude Code, and other runtimes.";
  if (error.kind === ErrorKind.vaultLocked) return "Unlock the drive vault to see agent runtimes.";
  return `Agent runtimes are unavailable right now: ${error.message}`;
}

/** The current time, updated every `interval` milliseconds, for relative timestamps. */
function useNow(interval: number): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), interval);
    return () => clearInterval(timer);
  }, [interval]);
  return now;
}
