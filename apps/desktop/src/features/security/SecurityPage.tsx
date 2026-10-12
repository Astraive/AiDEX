import { useState } from "react";
import { PageHeader } from "@aidex/ui/components";
import { PageFrame } from "@aidex/ui/desktop";
import { StatusIndicator, TabPanel, Tabs } from "@aidex/ui/primitives";
import { useNavigation } from "../../app/providers/navigation";
import { tabOf } from "../../lib/routes";
import { ApprovalsPanel } from "../approvals/ApprovalsPanel";
import { useApprovals } from "../approvals/useApprovals";
import { OverviewPanel } from "./OverviewPanel";
import { SandboxPanel } from "./SandboxPanel";
import { VaultPanel } from "./VaultPanel";
import type { IsolationLevel } from "./sample";
import { securityTabs, type SecurityTab } from "./tabs";

const TABS_ID = "security-tabs";

/** Security center: dashboard, vault, sandbox policy, and approvals, one tab each. */
export function SecurityPage() {
  const { route, replace } = useNavigation();
  const tab = tabOf(route, "security");
  // Shared across tabs so the dashboard and tab count reflect decisions and isolation changes.
  const approvals = useApprovals();
  const [isolation, setIsolation] = useState<IsolationLevel>("brokered");

  const tabItems = securityTabs.map(({ value, label, Icon }) => ({
    value,
    label,
    icon: <Icon />,
    count: value === "approvals" ? approvals.pending.length : undefined,
  }));

  return (
    <PageFrame width="default">
      <PageHeader
        title="Security"
        description="Policies, secrets, and approvals that govern what agents can do."
        meta={<StatusIndicator tone="neutral" label="Sandbox capabilities are live; policy, vault, audit, and approvals are sample data" />}
      />
      <Tabs
        id={TABS_ID}
        label="Security sections"
        variant="underline"
        items={tabItems}
        value={tab}
        onValueChange={(next: SecurityTab) => replace({ ...route, tab: next })}
      />
      <TabPanel tabsId={TABS_ID} value={tab}>
        {tab === "overview" ? <OverviewPanel pendingCount={approvals.pending.length} isolation={isolation} /> : null}
        {tab === "vault" ? <VaultPanel /> : null}
        {tab === "sandbox" ? <SandboxPanel isolation={isolation} onIsolationChange={setIsolation} /> : null}
        {tab === "approvals" ? <ApprovalsPanel {...approvals} /> : null}
      </TabPanel>
    </PageFrame>
  );
}
