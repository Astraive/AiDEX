import { useEffect, useId, useRef, useState } from "react";
import { ModelPicker, ModelPickerTrigger } from "@aidex/ui/components";
import { useNavigation } from "../../app/providers/navigation";
import { useKeyChord } from "../../hooks/useKeyChord";
import { useModifierKeyLabel } from "../../hooks/useModifierKeyLabel";
import { formatKeyChord } from "../../lib/keyChord";
import { MODEL_PICKER_SHORTCUT } from "../../lib/shortcuts";
import { useProviderCatalog } from "../model-manager/catalog/ProviderCatalog";
import { RUNTIME_KEY_PREFIX } from "../model-manager/catalog/modelKeys";
import { LOCAL_PROVIDER_ID } from "../model-manager/catalog/providerAccounts";
import { providerSelectionId } from "../model-manager/providers/selection";
import type { CurrentModel } from "./currentModel";

type Props = {
  /** Key of the conversation's model, even when it is unavailable. */
  value: string | null;
  current: CurrentModel;
  onSelect: (key: string) => void;
  disabled?: boolean;
};

/** Composer model button and its picker popover; `Mod+Shift+M` toggles it. */
export function ConversationModelPicker({ value, current, onSelect, disabled = false }: Props) {
  const catalog = useProviderCatalog();
  const { navigate } = useNavigation();
  const modifierLabel = useModifierKeyLabel();
  const [open, setOpen] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const pickerId = useId();
  const shortcutLabel = formatKeyChord(MODEL_PICKER_SHORTCUT, modifierLabel);

  useKeyChord(MODEL_PICKER_SHORTCUT, () => setOpen((wasOpen) => !wasOpen), {
    enabled: !disabled || open,
  });
  useEffect(() => {
    if (disabled) setOpen(false);
  }, [disabled]);

  // Runtimes report their models only on request; ask when the picker opens.
  const loadAllRuntimeModels = catalog.loadAllRuntimeModels;
  useEffect(() => {
    if (open) loadAllRuntimeModels();
  }, [open, loadAllRuntimeModels]);

  // The conversation's model stays visible (as unavailable) even when it is not listed or is hidden.
  const models =
    current.model && !catalog.models.some((model) => model.key === current.model!.key)
      ? [...catalog.models, current.model]
      : catalog.models;

  const openProviderSetup = (providerId: string) => {
    if (providerId === LOCAL_PROVIDER_ID) navigate({ page: "models", tab: "local" });
    else if (providerId.startsWith(RUNTIME_KEY_PREFIX))
      navigate({
        page: "models",
        tab: "providers",
        id: providerSelectionId("runtime", providerId.slice(RUNTIME_KEY_PREFIX.length)),
      });
    else navigate({ page: "models", tab: "providers", id: providerSelectionId("provider", providerId) });
  };

  return (
    <>
      <ModelPickerTrigger
        ref={triggerRef}
        provider={current.provider}
        model={current.model}
        open={open}
        onToggle={() => setOpen((wasOpen) => !wasOpen)}
        shortcutLabel={shortcutLabel}
        disabled={disabled}
        aria-controls={open ? pickerId : undefined}
      />
      <ModelPicker
        id={pickerId}
        anchorRef={triggerRef}
        open={open}
        onOpenChange={setOpen}
        placement="top-start"
        providers={catalog.providers}
        models={models}
        value={value}
        onValueChange={onSelect}
        favorites={catalog.favorites}
        onToggleFavorite={catalog.toggleFavorite}
        loading={catalog.loading}
        jumpModifierLabel={modifierLabel}
        emptyText="Connect a provider or start a local model to choose a model."
        onOpenProviderSetup={openProviderSetup}
        onManageProviders={() => navigate({ page: "models", tab: "providers" })}
      />
    </>
  );
}
