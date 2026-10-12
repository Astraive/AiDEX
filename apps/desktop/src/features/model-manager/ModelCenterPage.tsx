import { PageHeader } from "@aidex/ui/components";
import { PageFrame } from "@aidex/ui/desktop";
import { TabPanel, Tabs } from "@aidex/ui/primitives";
import { useNavigation } from "../../app/providers/navigation";
import { tabOf, type PageTab } from "../../lib/routes";
import { LocalModelsPanel } from "./LocalModelsPanel";
import { ProvidersPanel } from "./ProvidersPanel";
const items = [
  { value: "providers", label: "Providers" },
  { value: "local", label: "Local models" },
] as const satisfies readonly { value: PageTab<"models">; label: string }[];
export function ModelCenterPage() {
  const { route, replace } = useNavigation();
  const tab = tabOf(route, "models");
  return (
    <PageFrame width="wide">
      <PageHeader
        title="Models"
        description="Connect model providers and agent runtimes, and run verified local models from this drive."
      />
      <Tabs
        id="models-tabs"
        label="Model sections"
        variant="underline"
        items={items}
        value={tab}
        onValueChange={(next) => replace({ page: "models", tab: next })}
      />
      <TabPanel tabsId="models-tabs" value={tab}>
        {tab === "local" ? <LocalModelsPanel /> : <ProvidersPanel />}
      </TabPanel>
    </PageFrame>
  );
}
