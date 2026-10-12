import { List, ListRow, Notice, ProviderRow, SectionHeader } from "@aidex/ui/components";
import { PlusIcon } from "@aidex/ui/icons";
import { Badge, IconBadge, Stack, Switch, Text } from "@aidex/ui/primitives";
import { isOAuthAccount, isRuntimeAccount } from "../catalog/providerAccounts";
import type { AccountItem, ProviderItem, RuntimeItem } from "./providerItems";
import { useProviderActions } from "./useProviderActions";

type Props = {
  accounts: readonly AccountItem[];
  runtimes: readonly RuntimeItem[];
  selectedId: string | null;
  onSelect: (selectionId: string) => void;
  onAdd: () => void;
  /** Why runtimes could not be listed, shown in place of the runtime rows. */
  runtimesNotice: string | null;
};

/** Left pane: model provider accounts and agent runtimes, each with an enable switch. */
export function ProviderList({ accounts, runtimes, selectedId, onSelect, onAdd, runtimesNotice }: Props) {
  const actions = useProviderActions();
  const row = (item: ProviderItem) => {
    const enabled = item.kind === "provider" ? item.account.enabled : item.runtime.enabled;
    const toggleable = item.kind === "runtime" || !isRuntimeAccount(item.account);
    return (
      <ProviderRow
        key={item.selectionId}
        brand={item.brand}
        title={item.title}
        version={item.version}
        headline={item.summary.headline}
        detail={item.summary.detail}
        tone={item.summary.tone}
        selected={item.selectionId === selectedId}
        disabled={!enabled}
        badges={badgesFor(item)}
        onSelect={() => onSelect(item.selectionId)}
        trailing={
          toggleable ? (
            <Switch
              label={`Enable ${item.title}`}
              checked={enabled}
              disabled={actions.busy !== null}
              onCheckedChange={(checked) =>
                void (item.kind === "provider"
                  ? actions.setAccountEnabled(item.account, checked)
                  : actions.setRuntimeEnabled(item.runtime, checked))
              }
            />
          ) : undefined
        }
      />
    );
  };
  return (
    <Stack gap={5}>
      {actions.error && (
        <Notice tone="danger" role="alert">
          {actions.error}
        </Notice>
      )}
      <Stack gap={2}>
        <SectionHeader level={3} title="Model providers" meta={String(accounts.length)} />
        {accounts.length ? (
          <List>{accounts.map(row)}</List>
        ) : (
          <Text tone="muted">No model providers yet.</Text>
        )}
      </Stack>
      <Stack gap={2}>
        <SectionHeader level={3} title="Agent runtimes" meta={String(runtimes.length)} />
        {runtimesNotice ? (
          <Text tone="muted">{runtimesNotice}</Text>
        ) : runtimes.length ? (
          <List>{runtimes.map(row)}</List>
        ) : (
          <Text tone="muted">No agent runtimes yet.</Text>
        )}
      </Stack>
      <List>
        <ListRow
          media={<IconBadge icon={<PlusIcon />} />}
          title="Add provider"
          subtitle="Connect a model provider or an agent runtime."
          onSelect={onAdd}
        />
      </List>
    </Stack>
  );
}

/** "ChatGPT plan" for OAuth accounts and "Local" for loopback ones; runtimes have their own group. */
function badgesFor(item: ProviderItem) {
  if (item.kind === "runtime") return undefined;
  const labels = [
    ...(isOAuthAccount(item.account) ? ["ChatGPT plan"] : []),
    ...(item.account.descriptor.locality === "local" ? ["Local"] : []),
  ];
  if (!labels.length) return undefined;
  return (
    <>
      {labels.map((label) => (
        <Badge key={label} tone="neutral" size="sm">
          {label}
        </Badge>
      ))}
    </>
  );
}
