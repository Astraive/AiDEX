import { EmptyState } from "@aidex/ui/components";
import { CloudIcon, PlusIcon } from "@aidex/ui/icons";
import { Button, Stack } from "@aidex/ui/primitives";
import { isOAuthAccount, isRuntimeAccount } from "../catalog/providerAccounts";
import { ConnectionSection } from "./ConnectionSection";
import { CustomModelsEditor } from "./CustomModelsEditor";
import { EditorHeader } from "./EditorHeader";
import { ModelsSection } from "./ModelsSection";
import type { AccountItem, ProviderItem } from "./providerItems";
import { RuntimeEditor } from "./RuntimeEditor";
import { SetupSection } from "./SetupSection";
import { useProviderActions } from "./useProviderActions";

const CATEGORY_LABELS: Record<string, string> = {
  cloud: "Cloud",
  gateway: "Gateway",
  local: "Local",
  enterprise: "Enterprise",
  custom: "Custom",
};

/** Right pane: the selected account or runtime, or a prompt to add the first one. */
export function ProviderEditor({ item, onAdd }: { item: ProviderItem | undefined; onAdd: () => void }) {
  if (!item)
    return (
      <EmptyState
        icon={<CloudIcon />}
        title="No providers yet"
        description="Add a model provider or an agent runtime to start chatting with its models."
        actions={
          <Button variant="primary" icon={<PlusIcon />} onClick={onAdd}>
            Add provider
          </Button>
        }
      />
    );
  // Keyed by row so drafts and results reset when another row is selected.
  if (item.kind === "runtime") return <RuntimeEditor key={item.selectionId} item={item} />;
  return <AccountEditor key={item.selectionId} item={item} />;
}

function AccountEditor({ item }: { item: AccountItem }) {
  const actions = useProviderActions();
  const { account, preset } = item;
  const managed = isRuntimeAccount(account);
  const badges = [
    ...(isOAuthAccount(account) ? ["ChatGPT plan"] : []),
    ...(account.descriptor.locality === "local" ? ["Local"] : []),
    ...(account.enabled ? [] : ["Disabled"]),
    ...(preset && CATEGORY_LABELS[preset.category] && preset.category !== "local"
      ? [CATEGORY_LABELS[preset.category]]
      : []),
  ];
  return (
    <Stack gap={4}>
      <EditorHeader
        brand={item.brand}
        title={item.title}
        badges={badges}
        displayName={account.display_name}
        onRename={managed ? undefined : (name) => actions.renameAccount(account, name)}
        onRemove={managed ? undefined : () => actions.removeAccount(account)}
        busy={actions.busy !== null}
        error={actions.error}
      />
      <SetupSection item={item} />
      <ConnectionSection item={item} />
      <ModelsSection item={item} />
      {!managed && <CustomModelsEditor item={item} />}
    </Stack>
  );
}
