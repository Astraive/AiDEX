import { SidebarItem, SidebarSection } from "@aidex/ui/desktop";
import { BrandIcon, BoxIcon, CloudIcon, hasBrandIcon } from "@aidex/ui/icons";
import type { ModelPickerModel, ModelPickerProvider } from "@aidex/ui/components";
import { useNavigation } from "../../app/providers/navigation";
import { SectionSidebar } from "../../layouts/SectionSidebar";
import { useProviderCatalog } from "./catalog/ProviderCatalog";
import { RUNTIME_KEY_PREFIX } from "./catalog/modelKeys";

/** Models sidebar: favorites, then available chat models grouped by provider, then Manage links. */
export function ModelsSidebar() {
  const { route, navigate } = useNavigation();
  const catalog = useProviderCatalog();
  // Model details exist for listed native models; runtime models have no details page.
  const listed = catalog.models.filter(
    (model) => !model.key.startsWith(RUNTIME_KEY_PREFIX) && !model.unavailable,
  );
  const providerById = new Map(catalog.providers.map((provider) => [provider.id, provider]));
  const favorites = listed.filter((model) => catalog.favorites.has(model.key));
  const groups = catalog.providers
    .map((provider) => ({
      provider,
      models: listed.filter(
        (model) => model.providerId === provider.id && !catalog.favorites.has(model.key),
      ),
    }))
    .filter((group) => group.models.length > 0);

  const item = (model: ModelPickerModel, provider: ModelPickerProvider | undefined) => (
    <SidebarItem
      key={model.key}
      label={model.shortName || model.name}
      icon={
        provider ? (
          <BrandIcon
            brand={provider.brand}
            title={hasBrandIcon(provider.brand) ? undefined : provider.name}
            size={16}
            aria-hidden
          />
        ) : undefined
      }
      active={route.page === "model" && route.id === model.key}
      onSelect={() => navigate({ page: "model", id: model.key })}
    />
  );

  return (
    <SectionSidebar title="Models">
      {favorites.length > 0 && (
        <SidebarSection title="Favorites">
          {favorites.map((model) => item(model, providerById.get(model.providerId)))}
        </SidebarSection>
      )}
      {groups.map((group) => (
        <SidebarSection key={group.provider.id} title={group.provider.name}>
          {group.models.map((model) => item(model, group.provider))}
        </SidebarSection>
      ))}
      <SidebarSection title="Manage">
        <SidebarItem
          label="Providers"
          icon={<CloudIcon />}
          active={route.page === "models" && route.tab !== "local"}
          onSelect={() => navigate({ page: "models", tab: "providers" })}
        />
        <SidebarItem
          label="Local models"
          icon={<BoxIcon />}
          active={route.page === "models" && route.tab === "local"}
          onSelect={() => navigate({ page: "models", tab: "local" })}
        />
      </SidebarSection>
    </SectionSidebar>
  );
}
