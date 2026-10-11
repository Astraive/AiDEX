import { useState } from "react";
import { PageHeader } from "@aidex/ui/components";
import { PageFrame } from "@aidex/ui/desktop";
import { PlusIcon } from "@aidex/ui/icons";
import { Button, Stack, TabPanel, Tabs } from "@aidex/ui/primitives";
import { useNavigation } from "../../app/providers/navigation";
import { useRpc } from "../../hooks/useRpc";
import { tabOf, type PageTab } from "../../lib/routes";
import { CoreStatusNotice } from "../system/CoreStatusNotice";
import { SkillsPanel } from "./SkillsPanel";
import { McpPanel } from "./McpPanel";
import { PluginsPanel } from "./PluginsPanel";
import { WebSearchPanel } from "./WebSearchPanel";

export function ExtensionsPage() {
  const { route, replace } = useNavigation();
  const tab = tabOf(route, "extensions");
  const skills = useRpc("extensions.skills.list");
  const servers = useRpc("extensions.mcp.list");
  const plugins = useRpc("extensions.plugins.list");
  const [adding, setAdding] = useState(false);
  const refresh = () => {
    skills.refetch();
    servers.refetch();
    plugins.refetch();
    window.dispatchEvent(new Event("aidex:extensions-changed"));
  };
  const action = {
    skills: "Import skill",
    mcp: "Add MCP server",
    plugins: "Install plugin bundle",
  }[tab];
  return (
    <PageFrame width="default">
      <Stack gap={5}>
        <PageHeader
          title="Extensions"
          description="Portable skills, scoped MCP servers, and plugin contributions."
          actions={
            <Button icon={<PlusIcon />} onClick={() => setAdding(true)}>
              {action}
            </Button>
          }
        />
        <CoreStatusNotice requires="daemon" />
        <Tabs
          id="extensions-tabs"
          label="Extension sections"
          variant="underline"
          value={tab}
          items={[
            { value: "skills", label: "Skills", count: skills.data?.length },
            { value: "mcp", label: "MCP servers", count: servers.data?.length },
            { value: "plugins", label: "Plugins", count: plugins.data?.length },
          ]}
          onValueChange={(next: PageTab<"extensions">) => {
            setAdding(false);
            replace({ page: "extensions", tab: next });
          }}
        />
        <TabPanel tabsId="extensions-tabs" value={tab}>
          {tab === "skills" && (
            <Stack gap={5}>
              <SkillsPanel
                key={route.id}
                initialEditingId={route.id}
                adding={adding}
                onCloseImport={() => setAdding(false)}
                onChanged={refresh}
              />
              <WebSearchPanel />
            </Stack>
          )}
          {tab === "mcp" && (
            <McpPanel
              key={route.id}
              initialServerId={route.id}
              adding={adding}
              onCloseAdd={() => setAdding(false)}
              onChanged={refresh}
            />
          )}
          {tab === "plugins" && (
            <PluginsPanel
              adding={adding}
              onCloseInstall={() => setAdding(false)}
              onChanged={refresh}
            />
          )}
        </TabPanel>
      </Stack>
    </PageFrame>
  );
}
