import { useEffect } from "react";
import { Notice, PageHeader } from "@aidex/ui/components";
import { PageFrame } from "@aidex/ui/desktop";
import { Text } from "@aidex/ui/primitives";
import { useNavigation } from "../../app/providers/navigation";
import { usePreferences } from "../../app/providers/preferences";
import { providerSelectionId } from "../model-manager/providers/selection";
import { CoreStatusNotice } from "../system/CoreStatusNotice";
import { AgentConfigForm } from "./AgentConfigForm";
import { NATIVE_AGENT_ID } from "./nativeDefaults";

/**
 * Agent configuration: AiDEX Native's defaults for new conversations (route id `native`, or none). A
 * runtime id opens that runtime in Models → Providers, which owns runtime settings.
 */
export function AgentConfigPage() {
  const { route, back, canGoBack, replace } = useNavigation();
  const preferences = usePreferences();
  const runtimeId = route.id && route.id !== NATIVE_AGENT_ID ? route.id : null;
  useEffect(() => {
    if (runtimeId)
      replace({ page: "models", tab: "providers", id: providerSelectionId("runtime", runtimeId) });
  }, [runtimeId, replace]);
  if (runtimeId) return null;

  return (
    <PageFrame width="narrow">
      <PageHeader
        back={{ label: "Agents", onClick: () => (canGoBack ? back() : replace({ page: "agents" })) }}
        title="AiDEX Native"
        description="Defaults for new conversations. Each conversation keeps its own copy, which you can change in its Conversation settings."
      />
      <CoreStatusNotice requires="daemon" />
      {preferences.loaded ? (
        <AgentConfigForm />
      ) : preferences.error ? (
        <Notice tone="danger" role="alert">
          {preferences.error}
        </Notice>
      ) : (
        <Text tone="muted">Loading the saved defaults…</Text>
      )}
    </PageFrame>
  );
}
