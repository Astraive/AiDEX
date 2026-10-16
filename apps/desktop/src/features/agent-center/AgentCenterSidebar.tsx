import { SidebarItem, SidebarSection } from "@aidex/ui/desktop";
import { BotIcon, BrandIcon, hasBrandIcon, PlusCircleIcon } from "@aidex/ui/icons";
import { useNavigation } from "../../app/providers/navigation";
import { SectionSidebar } from "../../layouts/SectionSidebar";
import { useProviderCatalog } from "../model-manager/catalog/ProviderCatalog";
import { runtimeBrand, runtimeName } from "../model-manager/catalog/runtimes";
import { providerSelectionId } from "../model-manager/providers/selection";
import { NATIVE_AGENT_ID } from "./nativeDefaults";

/**
 * Shared by the Agents catalog and AiDEX Native's configuration: AiDEX Native, then the configured
 * runtimes, which open in Models → Providers.
 */
export function AgentCenterSidebar() {
  const { route, navigate, replace } = useNavigation();
  const catalog = useProviderCatalog();
  const openProviders = (runtimeId?: string) =>
    navigate({
      page: "models",
      tab: "providers",
      id: runtimeId ? providerSelectionId("runtime", runtimeId) : undefined,
    });

  return (
    <SectionSidebar
      title="Agents"
      action={{ label: "Add runtime", icon: <PlusCircleIcon />, onClick: () => openProviders() }}
    >
      <SidebarSection title="Built in">
        <SidebarItem
          label="AiDEX Native"
          icon={<BotIcon />}
          active={route.page === "agent-config"}
          // From the catalog, configuring adds a step Back returns from.
          onSelect={() =>
            (route.page === "agent-config" ? replace : navigate)({ page: "agent-config", id: NATIVE_AGENT_ID })
          }
        />
      </SidebarSection>
      {catalog.runtimes.length > 0 && (
        <SidebarSection title="Agent runtimes">
          {catalog.runtimes.map((runtime) => {
            const name = runtimeName(runtime, catalog.runtimeDrivers);
            const brand = runtimeBrand(runtime.driver);
            return (
              <SidebarItem
                key={runtime.id}
                label={name}
                icon={<BrandIcon brand={brand} title={hasBrandIcon(brand) ? undefined : name} size={16} aria-hidden />}
                onSelect={() => openProviders(runtime.id)}
              />
            );
          })}
        </SidebarSection>
      )}
    </SectionSidebar>
  );
}
