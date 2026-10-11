import { SidebarItem, SidebarSection } from "@aidex/ui/desktop";
import { useEffect } from "react";
import { BotIcon, PlusCircleIcon, TerminalIcon } from "@aidex/ui/icons";
import { Text } from "@aidex/ui/primitives";
import { useNavigation } from "../../app/providers/navigation";
import { useRpc } from "../../hooks/useRpc";
import { SectionSidebar } from "../../layouts/SectionSidebar";

export function AgentCenterSidebar() {
  const { route, navigate } = useNavigation();
  const runtimes = useRpc("runtime.list");
  useEffect(runtimes.refetch, [route.id, runtimes.refetch]);
  return (
    <SectionSidebar
      title="Agents"
      action={{
        label: "Add runtime",
        icon: <PlusCircleIcon />,
        onClick: () => navigate({ page: "agent-config" }),
      }}
    >
      <SidebarSection title="Built in">
        <SidebarItem
          label="AiDEX Native"
          icon={<BotIcon />}
          active={route.page === "agent-config" && route.id === "native"}
          onSelect={() => navigate({ page: "agent-config", id: "native" })}
        />
      </SidebarSection>
      <SidebarSection title="External instances">
        {runtimes.data?.runtimes.map((runtime) => (
          <SidebarItem
            key={runtime.id}
            label={runtime.id}
            icon={<TerminalIcon />}
            active={route.id === runtime.id}
            onSelect={() => navigate({ page: "agent-config", id: runtime.id })}
          />
        ))}
        {runtimes.error && <Text tone="muted">{runtimes.error.message}</Text>}
      </SidebarSection>
    </SectionSidebar>
  );
}
