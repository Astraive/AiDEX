import { useState } from "react";
import { EntityCard, List, ListRow, Notice, PageHeader, SectionHeader } from "@aidex/ui/components";
import { PageFrame } from "@aidex/ui/desktop";
import { BotIcon, BrandIcon, CheckCircleIcon, hasBrandIcon, PlusIcon, SettingsIcon } from "@aidex/ui/icons";
import { Badge, Button, IconBadge, Stack, StatusIndicator, Text } from "@aidex/ui/primitives";
import { ErrorKind, toAidexError } from "@aidex/client/errors";
import { RUNTIME_DEFAULT_MODEL, type RuntimeConfig } from "@aidex/client/dto/runtime";
import { useNavigation } from "../../app/providers/navigation";
import { usePreferences } from "../../app/providers/preferences";
import { useToast } from "../../app/providers/toast";
import { useProviderCatalog } from "../model-manager/catalog/ProviderCatalog";
import { parseModelKey, runtimeModelKey } from "../model-manager/catalog/modelKeys";
import { runtimeBrand, runtimeName } from "../model-manager/catalog/runtimes";
import { runtimeSummary } from "../model-manager/providers/providerItems";
import { providerSelectionId } from "../model-manager/providers/selection";
import { NATIVE_AGENT_ID } from "./nativeDefaults";

/**
 * Agents: AiDEX Native and the external runtimes configured on this drive, with their status. The
 * default for new chats is the saved default model (`AppPreferences.model_id`): a native key makes
 * AiDEX Native the default, a `runtime:<id>/<model>` key makes that runtime the default.
 */
export function AgentCenterPage() {
  const { navigate } = useNavigation();
  const preferences = usePreferences();
  const catalog = useProviderCatalog();
  const notify = useToast();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const saved = parseModelKey(preferences.modelId);
  const defaultRuntimeId = saved?.kind === "runtime" ? saved.runtimeId : null;
  const openProviders = (runtimeId?: string) =>
    navigate({
      page: "models",
      tab: "providers",
      id: runtimeId ? providerSelectionId("runtime", runtimeId) : undefined,
    });

  const setDefault = async (modelId: string, name: string) => {
    setBusy(true);
    setError(undefined);
    try {
      await preferences.updateSettings({ model_id: modelId });
      notify(`${name} is now the default for new chats.`);
    } catch (caught) {
      setError(toAidexError(caught).message);
    } finally {
      setBusy(false);
    }
  };

  const runtimeRow = (runtime: RuntimeConfig) => {
    const name = runtimeName(runtime, catalog.runtimeDrivers);
    const brand = runtimeBrand(runtime.driver);
    const summary = runtimeSummary(runtime, catalog.runtimeHealth[runtime.id], name);
    const isDefault = runtime.id === defaultRuntimeId;
    return (
      <ListRow
        key={runtime.id}
        media={<BrandIcon brand={brand} title={hasBrandIcon(brand) ? undefined : name} size={28} aria-hidden />}
        title={name}
        subtitle={summary.detail}
        meta={[
          <StatusIndicator key="status" tone={summary.tone} label={summary.headline} />,
          ...(isDefault
            ? [
                <Badge key="default" icon={<CheckCircleIcon />}>
                  Default
                </Badge>,
              ]
            : []),
        ]}
        trailing={
          <Stack direction="row" gap={2}>
            <Button
              variant="outline"
              size="md"
              aria-label={`Configure ${name}`}
              onClick={() => openProviders(runtime.id)}
            >
              Configure
            </Button>
            {isDefault ? null : (
              <Button
                variant="ghost"
                size="md"
                aria-label={`Set ${name} as default`}
                title={runtime.enabled ? undefined : "Enable this runtime in Models → Providers first."}
                disabled={busy || !runtime.enabled || !preferences.loaded}
                onClick={() => void setDefault(runtimeModelKey(runtime.id, RUNTIME_DEFAULT_MODEL), name)}
              >
                Set as default
              </Button>
            )}
          </Stack>
        }
      />
    );
  };

  return (
    <PageFrame width="default">
      <PageHeader
        title="Agents"
        description="AiDEX Native and the agent runtimes on this drive. New chats start with the default."
        actions={
          <Button variant="outline" size="md" icon={<SettingsIcon />} onClick={() => openProviders()}>
            Manage runtimes
          </Button>
        }
      />
      {(error || preferences.error) && (
        <Notice tone="danger" role="alert">
          {error ?? preferences.error}
        </Notice>
      )}
      <section>
        <Stack gap={4}>
          <SectionHeader title="Built in" />
          <EntityCard
            media={<IconBadge icon={<BotIcon />} tone={defaultRuntimeId ? "neutral" : "accent"} />}
            title="AiDEX Native"
            subtitle="AiDEX's own agent"
            badges={defaultRuntimeId ? undefined : <Badge icon={<CheckCircleIcon />}>Default</Badge>}
            description="Runs your connected and local models with AiDEX's own tools, approvals, and checkpoints. Configure its default model, permission, instructions, and limits."
            footer={
              <Stack direction="row" gap={2} wrap>
                <Button
                  variant="outline"
                  size="md"
                  aria-label="Configure AiDEX Native"
                  onClick={() => navigate({ page: "agent-config", id: NATIVE_AGENT_ID })}
                >
                  Configure
                </Button>
                {defaultRuntimeId ? (
                  <Button
                    variant="ghost"
                    size="md"
                    aria-label="Set AiDEX Native as default"
                    disabled={busy || !preferences.loaded}
                    // An empty key uses the Models default model (`model.use`).
                    onClick={() => void setDefault("", "AiDEX Native")}
                  >
                    Set as default
                  </Button>
                ) : null}
              </Stack>
            }
          />
        </Stack>
      </section>
      <section>
        <Stack gap={4}>
          <Stack gap={2}>
            <SectionHeader title="Agent runtimes" meta={String(catalog.runtimes.length)} />
            <Text tone="secondary">
              Optional: Codex, Claude Code, and other agents you already use, run inside a project with full access.
              AiDEX Native works without them.
            </Text>
          </Stack>
          {catalog.runtimesError ? (
            <Text tone="muted">{runtimesNotice(catalog.runtimesError)}</Text>
          ) : catalog.runtimesLoading ? (
            <Text tone="muted">Loading agent runtimes…</Text>
          ) : (
            <List metaWidth="160px">
              {catalog.runtimes.map(runtimeRow)}
              <ListRow
                media={<IconBadge size="md" icon={<PlusIcon />} />}
                title="Add a runtime"
                subtitle="Connect Codex, Claude Code, or another agent in Models → Providers."
                onSelect={() => openProviders()}
              />
            </List>
          )}
        </Stack>
      </section>
    </PageFrame>
  );
}

/** Calm wording for a runtime list that could not be read. */
function runtimesNotice(error: NonNullable<ReturnType<typeof useProviderCatalog>["runtimesError"]>): string {
  if (error.kind === ErrorKind.methodNotFound)
    return "Agent runtimes need a newer AiDEX Core. Update AiDEX to add Codex, Claude Code, and other runtimes.";
  if (error.kind === ErrorKind.vaultLocked) return "Unlock the drive vault to see agent runtimes.";
  return `Agent runtimes are unavailable right now: ${error.message}`;
}
