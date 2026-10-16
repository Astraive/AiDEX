import { PageHeader } from "@aidex/ui/components";
import { PageFrame } from "@aidex/ui/desktop";
import { StatusIndicator, TabPanel, Tabs } from "@aidex/ui/primitives";
import { useNavigation } from "../../app/providers/navigation";
import { tabOf } from "../../lib/routes";
import { LiveApprovalsPanel } from "../approvals/LiveApprovalsPanel";
import { OverviewPanel } from "./OverviewPanel";
import { SandboxPanel } from "./SandboxPanel";
import { VaultPanel } from "./VaultPanel";
import { securityTabs, type SecurityTab } from "./tabs";
import { useSecurityStatus } from "./useSecurityStatus";

const TABS_ID = "security-tabs";

/**
 * Security center: dashboard, vault, sandbox and policy, and approvals, one tab each. The page owns the
 * shared status (`useSecurityStatus`, one poll every 5 s) and passes it down; the sidebar reads what it
 * publishes.
 */
export function SecurityPage() {
  const { route, replace } = useNavigation();
  const tab = tabOf(route, "security");
  const status = useSecurityStatus(tab === "overview" || tab === "sandbox");
  const pending = status.pending.data?.total_approvals;
  const openTab = (next: SecurityTab) => replace({ ...route, tab: next });

  const tabItems = securityTabs.map(({ value, label, Icon }) => ({
    value,
    label,
    icon: <Icon />,
    count: value === "approvals" && pending ? pending : undefined,
  }));

  return (
    <PageFrame width="default">
      <PageHeader
        title="Security"
        description="Policies, secrets, and approvals that govern what agents can do."
        meta={<StatusIndicator tone="neutral" label="Live from AiDEX Core" />}
      />
      <Tabs id={TABS_ID} label="Security sections" variant="underline" items={tabItems} value={tab} onValueChange={openTab} />
      <TabPanel tabsId={TABS_ID} value={tab}>
        {tab === "overview" ? <OverviewPanel status={status} onOpenTab={openTab} /> : null}
        {tab === "vault" ? <VaultPanel status={status} /> : null}
        {tab === "sandbox" ? <SandboxPanel execution={status.execution.data} /> : null}
        {tab === "approvals" ? <LiveApprovalsPanel version={status.version} onChanged={status.refresh} /> : null}
      </TabPanel>
    </PageFrame>
  );
}
