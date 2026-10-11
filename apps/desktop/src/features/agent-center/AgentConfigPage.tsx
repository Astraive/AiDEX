import { Notice, PageHeader } from "@aidex/ui/components";
import { PageFrame } from "@aidex/ui/desktop";
import { Stack, Text } from "@aidex/ui/primitives";
import { useNavigation } from "../../app/providers/navigation";
import { usePreferences } from "../../app/providers/preferences";
import { useRpc } from "../../hooks/useRpc";
import { CoreStatusNotice } from "../system/CoreStatusNotice";
import { AgentConfigForm } from "./AgentConfigForm";
import { RuntimeConfigForm } from "./RuntimeConfigForm";

export function AgentConfigPage() {
  const { route, replace } = useNavigation();
  const preferences = usePreferences();
  const native = route.id === "native";
  const runtime = useRpc(
    "runtime.config.get",
    { id: route.id ?? "" },
    { enabled: !!route.id && !native },
  );
  const catalog = useRpc("runtime.catalog");
  return (
    <PageFrame width="narrow">
      <Stack gap={5}>
        <PageHeader
          title={
            native
              ? "AiDEX Native defaults"
              : route.id
                ? `Runtime · ${route.id}`
                : "Add external runtime"
          }
          description={
            native
              ? "Saved on the drive and applied to new conversations."
              : "Each instance owns its executable, credentials, and native conversation history."
          }
          back={{ label: "Agents", onClick: () => replace({ page: "agents" }) }}
        />
        <CoreStatusNotice requires="daemon" />
        {(runtime.error || catalog.error || preferences.error) && (
          <Notice tone="danger">
            {runtime.error?.message ??
              catalog.error?.message ??
              preferences.error}
          </Notice>
        )}
        {native ? (
          preferences.loaded ? (
            <AgentConfigForm />
          ) : (
            <Text tone="muted">Loading saved defaults…</Text>
          )
        ) : catalog.data && (!route.id || runtime.data) ? (
          <RuntimeConfigForm
            key={route.id ?? "new"}
            initial={runtime.data}
            catalog={catalog.data}
            onSaved={(id) => {
              runtime.refetch();
              replace({ page: "agent-config", id });
            }}
          />
        ) : (
          <Text tone="muted">Loading runtime configuration…</Text>
        )}
      </Stack>
    </PageFrame>
  );
}
