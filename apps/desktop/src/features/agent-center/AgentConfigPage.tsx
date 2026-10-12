import { PageHeader } from "@aidex/ui/components";
import { PageFrame } from "@aidex/ui/desktop";
import { DownloadIcon } from "@aidex/ui/icons";
import { Badge, Button, Stack, Text } from "@aidex/ui/primitives";
import { useNavigation } from "../../app/providers/navigation";
import { AgentConfigForm } from "./AgentConfigForm";
import { agentById, statusLabel } from "./sample";
import { useAgentActions } from "./useAgentActions";

/** Settings for one agent; the route `id` names it (unknown ids fall back to AiDEX Native). */
export function AgentConfigPage() {
  const { route, back, canGoBack, replace } = useNavigation();
  const actions = useAgentActions();
  const agent = agentById(route.id);
  const installed = agent.status !== "not-installed";

  return (
    <PageFrame width="narrow">
      <PageHeader
        back={{ label: "Agents", onClick: () => (canGoBack ? back() : replace({ page: "agents" })) }}
        title={agent.name}
        description={agent.description}
        meta={
          <Stack direction="row" gap={3} align="center" wrap>
            <Badge tone={installed ? "success" : "neutral"}>{statusLabel[agent.status]}</Badge>
            <Text as="span" tone="secondary">
              {agent.runtime}
            </Text>
          </Stack>
        }
        actions={
          installed ? undefined : (
            <Button variant="outline" size="md" icon={<DownloadIcon />} onClick={() => actions.install(agent)}>
              Install
            </Button>
          )
        }
      />
      <AgentConfigForm key={agent.id} agent={agent} />
    </PageFrame>
  );
}
