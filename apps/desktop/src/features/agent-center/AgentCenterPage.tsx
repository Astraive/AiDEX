import { useState } from "react";
import { PageHeader, SectionHeader } from "@aidex/ui/components";
import { PageFrame } from "@aidex/ui/desktop";
import { PlusIcon } from "@aidex/ui/icons";
import { Button, Grid, Stack, Text } from "@aidex/ui/primitives";
import { BuiltInAgentCard } from "./BuiltInAgentCard";
import { ExternalRuntimeList } from "./ExternalRuntimeList";
import { DEFAULT_AGENT_ID, builtInAgents, externalRuntimes } from "./sample";
import { useAgentActions } from "./useAgentActions";

const installedCount = externalRuntimes.filter((runtime) => runtime.status !== "not-installed").length;

/** Agent catalog: built-in specialists and optional external runtimes. */
export function AgentCenterPage() {
  const actions = useAgentActions();
  // Held for this session only; Core will persist the default agent.
  const [defaultAgentId, setDefaultAgentId] = useState(DEFAULT_AGENT_ID);

  return (
    <PageFrame width="default">
      <PageHeader
        title="Agents"
        description="Choose a runtime or configure a specialist."
        actions={
          <Button variant="primary" size="md" icon={<PlusIcon />} onClick={actions.createAgent}>
            New agent
          </Button>
        }
      />
      <section>
        <Stack gap={4}>
          <SectionHeader title="Built-in agents" meta={`${builtInAgents.length} ready`} />
          <Grid columns={3} gap={4}>
            {builtInAgents.map((agent) => (
              <BuiltInAgentCard
                key={agent.id}
                agent={agent}
                isDefault={agent.id === defaultAgentId}
                onConfigure={() => actions.configure(agent)}
                onSetDefault={() => setDefaultAgentId(agent.id)}
              />
            ))}
          </Grid>
        </Stack>
      </section>
      <section>
        <Stack gap={4}>
          <Stack gap={2}>
            <SectionHeader title="External runtimes" meta={`${installedCount} installed`} />
            <Text tone="secondary">
              External agents are optional adapters. AiDEX Native works on its own; add a runtime only if you already
              use it.
            </Text>
          </Stack>
          <ExternalRuntimeList runtimes={externalRuntimes} />
        </Stack>
      </section>
    </PageFrame>
  );
}
