import { SidebarItem, SidebarSection } from "@aidex/ui/desktop";
import { PlusCircleIcon } from "@aidex/ui/icons";
import { useNavigation } from "../../app/providers/navigation";
import { SectionSidebar } from "../../layouts/SectionSidebar";
import { agentById, builtInAgents, externalRuntimes, type SampleAgent } from "./sample";
import { useAgentActions } from "./useAgentActions";

/** Shared by the agent catalog and agent settings; the agent being configured (route `id`) is active. */
export function AgentCenterSidebar() {
  const { route } = useNavigation();
  const actions = useAgentActions();
  const activeId = route.page === "agent-config" ? agentById(route.id).id : undefined;

  const item = (agent: SampleAgent) => (
    <SidebarItem
      key={agent.id}
      label={agent.name}
      icon={<agent.Icon />}
      active={agent.id === activeId}
      onSelect={() => actions.configure(agent)}
    />
  );

  return (
    <SectionSidebar title="Agents" action={{ label: "New agent", icon: <PlusCircleIcon />, onClick: actions.createAgent }}>
      <SidebarSection title="Installed">{builtInAgents.map(item)}</SidebarSection>
      <SidebarSection title="External runtimes">{externalRuntimes.map(item)}</SidebarSection>
    </SectionSidebar>
  );
}
