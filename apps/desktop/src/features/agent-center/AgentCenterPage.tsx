import { useState } from "react";
import { Notice, PageHeader, SectionHeader } from "@aidex/ui/components";
import { PageFrame } from "@aidex/ui/desktop";
import { PlusIcon } from "@aidex/ui/icons";
import { Badge, Button, Card, Stack, Text } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import { useNavigation } from "../../app/providers/navigation";
import { usePreferences } from "../../app/providers/preferences";
import { useRpc } from "../../hooks/useRpc";
import { CoreStatusNotice } from "../system/CoreStatusNotice";

export function AgentCenterPage() {
  const { navigate } = useNavigation();
  const preferences = usePreferences();
  const runtimes = useRpc("runtime.list");
  const catalog = useRpc("runtime.catalog");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const act = async (operation: () => Promise<unknown>) => {
    setBusy(true);
    setError(undefined);
    try {
      await operation();
      runtimes.refetch();
    } catch (caught) {
      setError(toAidexError(caught).message);
    } finally {
      setBusy(false);
    }
  };
  const defaultRuntime = preferences.settings.runtime_id;
  return (
    <PageFrame width="default">
      <Stack gap={5}>
        <PageHeader
          title="Agents"
          description="Configure AiDEX Native or an external agent runtime."
          actions={
            <Button
              icon={<PlusIcon />}
              onClick={() => navigate({ page: "agent-config" })}
            >
              Add runtime
            </Button>
          }
        />
        <CoreStatusNotice requires="daemon" />
        {(error || runtimes.error || catalog.error || preferences.error) && (
          <Notice tone="danger">
            {error ??
              runtimes.error?.message ??
              catalog.error?.message ??
              preferences.error}
          </Notice>
        )}
        <SectionHeader title="AiDEX Native" />
        <Card padding="md">
          <Stack gap={3}>
            <Text>Built-in coding harness</Text>
            <Text tone="muted">
              Uses your connected provider or local model, instructions,
              permissions, and execution limits.
            </Text>
            {!defaultRuntime && (
              <Badge tone="accent">Default for new agent tasks</Badge>
            )}
            <Stack direction="row" gap={2} wrap>
              <Button
                variant="outline"
                onClick={() => navigate({ page: "agent-config", id: "native" })}
              >
                Configure
              </Button>
              <Button
                variant="ghost"
                disabled={busy || !preferences.loaded}
                onClick={() =>
                  void act(() =>
                    preferences.updateSettings({
                      runtime_id: null,
                      runtime_model_id: null,
                    }),
                  )
                }
              >
                Set as default
              </Button>
              <Button
                variant="ghost"
                onClick={() => navigate({ page: "agent" })}
              >
                New task
              </Button>
            </Stack>
          </Stack>
        </Card>
        <SectionHeader title="External runtime instances" />
        {runtimes.loading && (
          <Text tone="muted">Loading configured runtimes…</Text>
        )}
        {runtimes.data?.runtimes.map((runtime) => {
          const driver = catalog.data?.find(
            (entry) => entry.driver === runtime.driver,
          );
          return (
            <Card key={runtime.id} padding="md">
              <Stack gap={3}>
                <Text>
                  {driver?.name ?? runtime.driver} · {runtime.id}
                </Text>
                <Text tone="muted">{runtime.executable}</Text>
                <Stack direction="row" gap={2}>
                  <Badge>
                    {runtime.enabled
                      ? "Enabled · readiness unchecked"
                      : "Disabled"}
                  </Badge>
                  {defaultRuntime === runtime.id && (
                    <Badge tone="accent">Default for new agent tasks</Badge>
                  )}
                </Stack>
                <Stack direction="row" gap={2} wrap>
                  <Button
                    variant="outline"
                    onClick={() =>
                      navigate({ page: "agent-config", id: runtime.id })
                    }
                  >
                    Configure and check readiness
                  </Button>
                  <Button
                    variant="ghost"
                    disabled={busy || !runtime.enabled || !preferences.loaded}
                    onClick={() =>
                      void act(() =>
                        preferences.updateSettings({
                          runtime_id: runtime.id,
                          runtime_model_id:
                            defaultRuntime === runtime.id
                              ? preferences.settings.runtime_model_id
                              : null,
                        }),
                      )
                    }
                  >
                    Set as default
                  </Button>
                  <Button
                    variant="ghost"
                    disabled={busy}
                    onClick={() => {
                      if (
                        !window.confirm(
                          `Remove runtime instance ${runtime.id}? Saved conversation history will remain.`,
                        )
                      )
                        return;
                      void act(async () => {
                        await call("runtime.remove", { id: runtime.id });
                        if (defaultRuntime === runtime.id)
                          await preferences.updateSettings({
                            runtime_id: null,
                            runtime_model_id: null,
                          });
                      });
                    }}
                  >
                    Remove
                  </Button>
                </Stack>
              </Stack>
            </Card>
          );
        })}
        {runtimes.data && !runtimes.data.runtimes.length && (
          <Text tone="muted">
            No external runtimes configured. Add a literal executable or install
            a verified bundled component.
          </Text>
        )}
      </Stack>
    </PageFrame>
  );
}
