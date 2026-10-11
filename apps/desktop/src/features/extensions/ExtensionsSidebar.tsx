import { SidebarItem, SidebarSection } from "@aidex/ui/desktop";
import { useEffect } from "react";
import { LayersIcon, PuzzleIcon, ServerIcon } from "@aidex/ui/icons";
import { useNavigation } from "../../app/providers/navigation";
import { useRpc } from "../../hooks/useRpc";
import { SectionSidebar } from "../../layouts/SectionSidebar";
import { tabOf } from "../../lib/routes";

export function ExtensionsSidebar() {
  const { route, replace } = useNavigation();
  const tab = tabOf(route, "extensions");
  const skills = useRpc("extensions.skills.list");
  const servers = useRpc("extensions.mcp.list");
  const plugins = useRpc("extensions.plugins.list");
  useEffect(() => {
    const refresh = () => {
      skills.refetch();
      servers.refetch();
      plugins.refetch();
    };
    window.addEventListener("aidex:extensions-changed", refresh);
    return () =>
      window.removeEventListener("aidex:extensions-changed", refresh);
  }, [skills.refetch, servers.refetch, plugins.refetch]);
  return (
    <SectionSidebar title="Extensions">
      <SidebarSection title="Browse">
        <SidebarItem
          label="Skills"
          icon={<LayersIcon />}
          count={skills.data?.length}
          active={tab === "skills"}
          onSelect={() => replace({ page: "extensions", tab: "skills" })}
        />
        <SidebarItem
          label="MCP servers"
          icon={<ServerIcon />}
          count={servers.data?.length}
          active={tab === "mcp"}
          onSelect={() => replace({ page: "extensions", tab: "mcp" })}
        />
        <SidebarItem
          label="Plugins"
          icon={<PuzzleIcon />}
          count={plugins.data?.length}
          active={tab === "plugins"}
          onSelect={() => replace({ page: "extensions", tab: "plugins" })}
        />
      </SidebarSection>
      <SidebarSection title="Enabled skills">
        {skills.data
          ?.filter((skill) => skill.enabled)
          .map((skill) => (
            <SidebarItem
              key={skill.id}
              label={skill.name}
              icon={<LayersIcon />}
              active={tab === "skills" && route.id === skill.id}
              onSelect={() =>
                replace({ page: "extensions", tab: "skills", id: skill.id })
              }
            />
          ))}
      </SidebarSection>
      <SidebarSection title="MCP instances">
        {servers.data?.map((server) => (
          <SidebarItem
            key={server.id}
            label={server.name}
            icon={<ServerIcon />}
            active={tab === "mcp" && route.id === server.id}
            onSelect={() =>
              replace({ page: "extensions", tab: "mcp", id: server.id })
            }
          />
        ))}
      </SidebarSection>
    </SectionSidebar>
  );
}
