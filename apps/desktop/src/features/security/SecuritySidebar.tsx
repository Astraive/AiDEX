import { useEffect } from "react";
import { SidebarItem, SidebarSection } from "@aidex/ui/desktop";
import { Text } from "@aidex/ui/primitives";
import { useNavigation } from "../../app/providers/navigation";
import { useRpc } from "../../hooks/useRpc";
import { tabOf } from "../../lib/routes";
import { SectionSidebar } from "../../layouts/SectionSidebar";
import { securityTabs, type SecurityTab } from "./tabs";

export function SecuritySidebar() {
  const { route, replace } = useNavigation();
  const vault = useRpc("vault.status");
  const pending = useRpc("approval.list", {
    status: "pending",
    offset: 0,
    limit: 1,
  });
  useEffect(() => {
    const refresh = () => {
      vault.refetch();
      pending.refetch();
    };
    const timer = setInterval(refresh, 3000);
    window.addEventListener("aidex:security-changed", refresh);
    return () => {
      clearInterval(timer);
      window.removeEventListener("aidex:security-changed", refresh);
    };
  }, [vault.refetch, pending.refetch]);
  const active =
    route.page === "security" ? tabOf(route, "security") : undefined;
  return (
    <SectionSidebar title="Security">
      <SidebarSection title="Controls and evidence">
        {securityTabs.map(({ value, label, Icon }) => (
          <SidebarItem
            key={value}
            label={label}
            icon={<Icon />}
            active={active === value}
            count={
              value === "approvals" ? pending.data?.total_approvals : undefined
            }
            onSelect={() =>
              replace({ page: "security", tab: value as SecurityTab })
            }
          />
        ))}
      </SidebarSection>
      <SidebarSection title="Core status">
        <Text tone="muted">Vault: {vault.data?.state ?? "unavailable"}</Text>
        {vault.error && <Text tone="muted">{vault.error.message}</Text>}
      </SidebarSection>
    </SectionSidebar>
  );
}
