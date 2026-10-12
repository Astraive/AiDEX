import type { ComponentType } from "react";
import { SidebarItem, SidebarSection } from "@aidex/ui/desktop";
import { BrandIcon, GlobeIcon, LayersIcon, PuzzleIcon, ServerIcon, type IconProps } from "@aidex/ui/icons";
import { useNavigation } from "../../app/providers/navigation";
import { SectionSidebar } from "../../layouts/SectionSidebar";
import { tabOf, type PageTab } from "../../lib/routes";
import { useExtensionLists } from "./data";
import { serverBrand } from "./mcp/mcpLabels";

type ExtensionsTab = PageTab<"extensions">;

const browseItems = [
  { tab: "skills", label: "Skills", Icon: LayersIcon },
  { tab: "mcp", label: "MCP servers", Icon: ServerIcon },
  { tab: "plugins", label: "Plugins", Icon: PuzzleIcon },
  { tab: "web", label: "Web search", Icon: GlobeIcon },
] as const satisfies readonly { tab: ExtensionsTab; label: string; Icon: ComponentType<IconProps> }[];

/** Browse the four tabs (with live counts), plus shortcuts to running MCP servers and enabled plugins. */
export function ExtensionsSidebar() {
  const { route, replace } = useNavigation();
  const tab = tabOf(route, "extensions");
  const lists = useExtensionLists();
  const counts: Partial<Record<ExtensionsTab, number>> = {
    skills: lists.skills?.length,
    mcp: lists.servers?.length,
    plugins: lists.plugins?.length,
  };
  const running = (lists.servers ?? []).filter((server) => server.status === "running" || server.status === "starting");
  const enabledPlugins = (lists.plugins ?? []).filter((plugin) => plugin.enabled);

  return (
    <SectionSidebar title="Plugins">
      <SidebarSection title="Browse">
        {browseItems.map(({ tab: itemTab, label, Icon }) => (
          <SidebarItem
            key={itemTab}
            label={label}
            icon={<Icon />}
            count={counts[itemTab]}
            active={tab === itemTab}
            onSelect={() => replace({ page: "extensions", tab: itemTab })}
          />
        ))}
      </SidebarSection>
      {running.length > 0 ? (
        <SidebarSection title="Running servers">
          {running.map((server) => (
            <SidebarItem
              key={server.id}
              label={server.name}
              icon={<BrandIcon brand={serverBrand(server)} monogramLabel={server.name} size={16} aria-hidden />}
              active={tab === "mcp" && route.id === server.id}
              onSelect={() => replace({ page: "extensions", tab: "mcp", id: server.id })}
            />
          ))}
        </SidebarSection>
      ) : null}
      {enabledPlugins.length > 0 ? (
        <SidebarSection title="Enabled plugins">
          {enabledPlugins.map((plugin) => (
            <SidebarItem
              key={plugin.name}
              label={plugin.name}
              icon={<PuzzleIcon />}
              active={tab === "plugins" && route.id === plugin.name}
              onSelect={() => replace({ page: "extensions", tab: "plugins", id: plugin.name })}
            />
          ))}
        </SidebarSection>
      ) : null}
    </SectionSidebar>
  );
}
