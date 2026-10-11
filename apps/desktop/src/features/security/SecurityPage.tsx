import { useEffect } from "react";
import { PageHeader } from "@aidex/ui/components";
import { PageFrame } from "@aidex/ui/desktop";
import { StatusIndicator, TabPanel, Tabs } from "@aidex/ui/primitives";
import { useNavigation } from "../../app/providers/navigation";
import { tabOf } from "../../lib/routes";
import { LiveApprovalsPanel } from "../approvals/LiveApprovalsPanel";
import { useRpc } from "../../hooks/useRpc";
import { OverviewPanel } from "./OverviewPanel";
import { SandboxPanel } from "./SandboxPanel";
import { VaultPanel } from "./VaultPanel";
import { securityTabs, type SecurityTab } from "./tabs";

const TABS_ID = "security-tabs";

/** Security center: dashboard, vault, sandbox policy, and approvals, one tab each. */
export function SecurityPage() {
  const { route, replace } = useNavigation();
  const tab = tabOf(route, "security");
  // Shared across tabs so the dashboard and tab count reflect decisions and isolation changes.
  const approvals = useRpc("approval.list", {
    status: "pending",
    offset: 0,
    limit: 1,
  });
  useEffect(() => {
    const timer = setInterval(approvals.refetch, 2000);
    return () => clearInterval(timer);
  }, [approvals.refetch]);
  const pendingCount = approvals.data?.total_approvals ?? null;

  const tabItems = securityTabs.map(({ value, label, Icon }) => ({
    value,
    label,
    icon: <Icon />,
    count: value === "approvals" ? (pendingCount ?? undefined) : undefined,
  }));

  return (
    <PageFrame width="default">
      <PageHeader
        title="Security"
        description="Policies, secrets, and approvals that govern what agents can do."
        meta={
          <StatusIndicator
            tone="neutral"
            label="Live vault metadata, broker approvals, host capabilities, and recorded audit"
          />
        }
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
        {tab === "overview" ? (
          <OverviewPanel pendingCount={pendingCount} />
        ) : null}
        {tab === "vault" ? <VaultPanel /> : null}
        {tab === "sandbox" ? <SandboxPanel /> : null}
        {tab === "approvals" ? (
          <LiveApprovalsPanel onChanged={approvals.refetch} />
        ) : null}
      </TabPanel>
    </PageFrame>
  );
}
