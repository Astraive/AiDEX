import type { ReactNode } from "react";
import { PageHeader } from "@aidex/ui/components";
import { PageFrame } from "@aidex/ui/desktop";
import { StatusIndicator, TabPanel, Tabs } from "@aidex/ui/primitives";
import { useNavigation } from "../../app/providers/navigation";
import { tabOf } from "../../lib/routes";
import { AppearancePanel } from "./AppearancePanel";
import { DiagnosticsPanel } from "./DiagnosticsPanel";
import { DrivePanel } from "./DrivePanel";
import { ShortcutsPanel } from "./ShortcutsPanel";
import { settingsTabs, type SettingsTab } from "./tabs";
import { UpdatesPanel } from "./UpdatesPanel";

const TABS_ID = "settings-tabs";

const tabItems = settingsTabs.map(({ value, label }) => ({ value, label }));

const panels: Record<SettingsTab, () => ReactNode> = {
  appearance: () => <AppearancePanel />,
  shortcuts: () => <ShortcutsPanel />,
  updates: () => <UpdatesPanel />,
  diagnostics: () => <DiagnosticsPanel />,
  drive: () => <DrivePanel />,
};

export function SettingsPage() {
  const { route, replace } = useNavigation();
  const tab = tabOf(route, "settings");

  return (
    <PageFrame width="default">
      <PageHeader
        title="Settings"
        description="Appearance and preferences for this drive."
        meta={<StatusIndicator tone="neutral" label="Diagnostics, Drive, and signed updates are live; cleanup is sample data" />}
      />
      <Tabs
        id={TABS_ID}
        label="Settings sections"
        variant="underline"
        items={tabItems}
        value={tab}
        onValueChange={(next) => replace({ page: "settings", tab: next })}
      />
      <TabPanel tabsId={TABS_ID} value={tab}>
        {panels[tab]()}
      </TabPanel>
    </PageFrame>
  );
}
