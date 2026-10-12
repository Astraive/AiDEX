import { PageHeader } from "@aidex/ui/components";
import { PageFrame } from "@aidex/ui/desktop";
import { TabPanel, Tabs, Text } from "@aidex/ui/primitives";
import { useNavigation } from "../../app/providers/navigation";
import { useAidexStatus } from "../../hooks/useAidexStatus";
import { tabOf, type PageTab } from "../../lib/routes";
import { CoreStatusNotice } from "../system/CoreStatusNotice";
import { useExtensionLists } from "./data";
import { McpPanel } from "./mcp/McpPanel";
import { PluginsPanel } from "./plugins/PluginsPanel";
import { SkillsPanel } from "./skills/SkillsPanel";
import { WebPanel } from "./web/WebPanel";

type ExtensionsTab = PageTab<"extensions">;

const TABS_ID = "extensions-tabs";

/** Extensions screen: tabs for skills, MCP servers, plugins, and web search; the route `id` selects an item. */
export function ExtensionsPage() {
  const { route, replace } = useNavigation();
  const tab = tabOf(route, "extensions");
  const lists = useExtensionLists();
  const { daemonRunning } = useAidexStatus();
  const select = (id: string | undefined) => replace({ page: "extensions", tab, id });

  const tabItems: { value: ExtensionsTab; label: string; count?: number }[] = [
    { value: "skills", label: "Skills", count: lists.skills?.length },
    { value: "mcp", label: "MCP servers", count: lists.servers?.length },
    { value: "plugins", label: "Plugins", count: lists.plugins?.length },
    { value: "web", label: "Web search" },
  ];

  return (
    <PageFrame width="wide">
      <PageHeader
        title="Plugins"
        description="Skills, MCP servers, plugins, and web search that extend what your agents can do."
      />
      <CoreStatusNotice requires="daemon" />
      <Tabs
        id={TABS_ID}
        label="Extension sections"
        variant="underline"
        items={tabItems}
        value={tab}
        onValueChange={(next) => replace({ page: "extensions", tab: next })}
      />
      <TabPanel tabsId={TABS_ID} value={tab}>
        {!daemonRunning ? (
          <Text tone="muted">Skills, MCP servers, plugins, and web search settings appear once AiDEX Core is running.</Text>
        ) : tab === "skills" ? (
          <SkillsPanel selectedId={route.id} onSelect={select} />
        ) : tab === "mcp" ? (
          <McpPanel selectedId={route.id} onSelect={select} />
        ) : tab === "plugins" ? (
          <PluginsPanel selectedId={route.id} onSelect={select} />
        ) : (
          <WebPanel />
        )}
      </TabPanel>
    </PageFrame>
  );
}
