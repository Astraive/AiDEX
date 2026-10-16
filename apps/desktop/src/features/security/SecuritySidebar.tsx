import { SidebarItem, SidebarSection } from "@aidex/ui/desktop";
import { ClipboardCheckIcon, LockIcon, LockOpenIcon } from "@aidex/ui/icons";
import { useNavigation } from "../../app/providers/navigation";
import { tabOf } from "../../lib/routes";
import { SectionSidebar } from "../../layouts/SectionSidebar";
import { securityTabs, type SecurityTab } from "./tabs";
import { useSecuritySnapshot } from "./useSecurityStatus";

const vaultLabels = {
  unlocked: "Vault unlocked",
  locked: "Vault locked",
  missing: "Vault not set up",
  unavailable: "Vault unavailable",
} as const;

/** Security navigation: one item per tab (Approvals with the pending count), plus the vault state and waiting approvals. */
export function SecuritySidebar() {
  const { route, replace } = useNavigation();
  const activeTab = route.page === "security" ? tabOf(route, "security") : undefined;
  // Published by SecurityPage's poll, so the sidebar adds no requests of its own.
  const { vaultState, pendingApprovals } = useSecuritySnapshot();

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
            count={value === "approvals" && pendingApprovals ? pendingApprovals : undefined}
            onSelect={() => openTab(value)}
          />
        ))}
      </SidebarSection>
      {vaultState || pendingApprovals ? (
        <SidebarSection title="Status">
          {vaultState ? (
            <SidebarItem
              label={vaultLabels[vaultState]}
              icon={vaultState === "unlocked" ? <LockOpenIcon /> : <LockIcon />}
              size="sm"
              onSelect={() => openTab("vault")}
            />
          ) : null}
          {pendingApprovals ? (
            <SidebarItem
              label={`${pendingApprovals} waiting for approval`}
              icon={<ClipboardCheckIcon />}
              size="sm"
              onSelect={() => openTab("approvals")}
            />
          ) : null}
        </SidebarSection>
      ) : null}
    </SectionSidebar>
  );
}
