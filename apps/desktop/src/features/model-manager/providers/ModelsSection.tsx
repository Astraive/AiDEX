import { useState } from "react";
import {
  InlineCode,
  List,
  ListRow,
  Notice,
  rankBySearch,
  SettingsSection,
} from "@aidex/ui/components";
import { LayersIcon, RefreshIcon, StarFilledIcon, StarIcon } from "@aidex/ui/icons";
import {
  Badge,
  Button,
  IconButton,
  SearchField,
  Stack,
  Switch,
  Text,
} from "@aidex/ui/primitives";
import { toAidexError } from "@aidex/client/errors";
import type { ProviderModel } from "@aidex/client/dto/models";
import { useProviderCatalog } from "../catalog/ProviderCatalog";
import {
  capabilityBadges,
  contextLabel,
  isChatModel,
  SOURCE_LABELS,
} from "../catalog/modelFacts";
import { nativeModelKey } from "../catalog/modelKeys";
import { isAuthFailure } from "../catalog/providerAccounts";
import type { AccountItem } from "./providerItems";
import { useProviderActions } from "./useProviderActions";

/** Show a filter field above this many models (t3code's threshold). */
const FILTER_THRESHOLD = 8;

/** The account's models: favorites, picker visibility, default model, and a refresh. */
export function ModelsSection({ item }: { item: AccountItem }) {
  const catalog = useProviderCatalog();
  const actions = useProviderActions();
  const [query, setQuery] = useState("");
  const [refreshError, setRefreshError] = useState<string | null>(null);
  const { account, models, failure } = item;
  const keyOf = (model: ProviderModel) => nativeModelKey(model.model);
  const filtering = models.length > FILTER_THRESHOLD && query.trim().length > 0;
  const shown = filtering
    ? rankBySearch(models, query, (model) => ({
        name: model.display_name,
        shortName: model.model.model_id,
        providerName: item.title,
        key: keyOf(model),
        isFavorite: catalog.favorites.has(keyOf(model)),
      }))
    : models;
  const favorite = (model: ProviderModel) => catalog.favorites.has(keyOf(model));
  const hidden = (model: ProviderModel) => catalog.hidden.has(keyOf(model));
  const groups = [
    { label: "Favorites", rows: shown.filter(favorite) },
    { label: "Shown in picker", rows: shown.filter((model) => !favorite(model) && !hidden(model)) },
    { label: "Hidden from picker", rows: shown.filter((model) => !favorite(model) && hidden(model)) },
  ].filter((group) => group.rows.length > 0);

  const refresh = async () => {
    setRefreshError(null);
    try {
      await catalog.refresh(account.descriptor.id);
    } catch (caught) {
      setRefreshError(toAidexError(caught).message);
    }
  };

  const row = (model: ProviderModel) => {
    const key = keyOf(model);
    const name = model.display_name || model.model.model_id;
    const isFavorite = favorite(model);
    const chat = isChatModel(model);
    const source = SOURCE_LABELS[model.source] ?? { label: model.source, title: model.source };
    return (
      <ListRow
        key={key}
        media={
          <IconButton
            size="sm"
            label={isFavorite ? `Remove ${name} from favorites` : `Add ${name} to favorites`}
            aria-pressed={isFavorite}
            selected={isFavorite}
            icon={isFavorite ? <StarFilledIcon /> : <StarIcon />}
            onClick={() => catalog.toggleFavorite(key)}
          />
        }
        title={name}
        subtitle={<InlineCode>{model.model.model_id}</InlineCode>}
        meta={[
          contextLabel(model.context_window) ?? "Context unknown",
          <>
            {capabilityBadges(model).map((label) => (
              <Badge key={label} tone="neutral" size="sm">
                {label}
              </Badge>
            ))}
          </>,
          <Badge tone={model.source === "assumed" ? "warning" : "neutral"} size="sm" title={source.title}>
            {source.label}
          </Badge>,
        ]}
        trailing={
          <Stack direction="row" gap={2} align="center">
            {key === catalog.defaultKey ? (
              <Badge tone="accent" size="sm">
                Default
              </Badge>
            ) : chat ? (
              <Button
                variant="ghost"
                disabled={actions.busy !== null}
                onClick={() => void actions.setDefaultModel(model.model)}
              >
                Set as default
              </Button>
            ) : null}
            {chat ? (
              <Switch
                label={`Show ${name} in picker`}
                checked={!hidden(model)}
                onCheckedChange={(checked) => catalog.setHidden(key, !checked)}
              />
            ) : (
              <Badge tone="neutral" size="sm" title="Embedding and other non-chat models never appear in the picker.">
                Not for chat
              </Badge>
            )}
          </Stack>
        }
      />
    );
  };

  return (
    <SettingsSection
      title="Models"
      icon={<LayersIcon />}
      description={
        models.length
          ? `${models.length} ${models.length === 1 ? "model" : "models"} · stars pin models to the top of the picker.`
          : undefined
      }
      actions={
        <Button
          variant="outline"
          icon={<RefreshIcon />}
          disabled={catalog.refreshing || !account.enabled}
          onClick={() => void refresh()}
        >
          {catalog.refreshing ? "Refreshing…" : "Refresh models"}
        </Button>
      }
    >
      {failure && (
        <Notice tone={isAuthFailure(failure.kind) ? "warning" : "danger"}>{failure.message}</Notice>
      )}
      {refreshError && (
        <Notice tone="danger" role="alert">
          {refreshError}
        </Notice>
      )}
      {(actions.error || catalog.preferencesError) && (
        <Notice tone="danger" role="alert">
          {actions.error ?? catalog.preferencesError}
        </Notice>
      )}
      {models.length > FILTER_THRESHOLD && (
        <SearchField
          label="Filter models"
          placeholder="Filter models"
          size="md"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
      )}
      {!account.enabled ? (
        <Text tone="muted">Enable this provider to list its models.</Text>
      ) : groups.length === 0 ? (
        <Text tone="muted">
          {filtering ? "No models match." : failure ? "No models could be listed." : "No models reported for this provider yet."}
        </Text>
      ) : (
        groups.map((group) => (
          <Stack key={group.label} gap={2}>
            <Text variant="label" tone="muted">
              {group.label}
            </Text>
            <List metaWidth="150px">{group.rows.map(row)}</List>
          </Stack>
        ))
      )}
    </SettingsSection>
  );
}
