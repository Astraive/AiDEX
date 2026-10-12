import { SidebarItem, SidebarSection } from "@aidex/ui/desktop";
import { AlertTriangleIcon } from "@aidex/ui/icons";
import { useNavigation } from "../../app/providers/navigation";
import { tabOf } from "../../lib/routes";
import { SectionSidebar } from "../../layouts/SectionSidebar";
import { sampleAlerts } from "./sample";
import { securityTabs, type SecurityTab } from "./tabs";

/** Security navigation: one item per tab, plus recent alerts that open the related tab. */
export function SecuritySidebar() {
  const { route, replace } = useNavigation();
  const activeTab = route.page === "security" ? tabOf(route, "security") : undefined;

  const openTab = (tab: SecurityTab) => replace({ page: "security", tab });

  return (
    <SectionSidebar title="Security">
      <SidebarSection title="Security">
        {securityTabs.map(({ value, label, Icon }) => (
          <SidebarItem
            key={value}
            label={label}
            icon={<Icon />}
            active={value === activeTab}
            onSelect={() => openTab(value)}
          />
        ))}
      </SidebarSection>
      <SidebarSection title="Recent alerts">
        {sampleAlerts.map((alert) => (
          <SidebarItem key={alert.id} label={alert.label} icon={<AlertTriangleIcon />} size="sm" onSelect={() => openTab(alert.tab)} />
        ))}
      </SidebarSection>
    </SectionSidebar>
  );
}
