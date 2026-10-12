import { useState } from "react";
import { InlineCode, List, ListRow, Notice } from "@aidex/ui/components";
import { BrandIcon, hasBrandIcon } from "@aidex/ui/icons";
import { Badge, Button, Stack, Text } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import { usePreferences } from "../../app/providers/preferences";
import { useNavigation } from "../../app/providers/navigation";
import { useProviderCatalog } from "./catalog/ProviderCatalog";
import { capabilityBadges, contextLabel, SOURCE_LABELS } from "./catalog/modelFacts";
import { nativeModelKey } from "./catalog/modelKeys";
import { LOCAL_PROVIDER_ID } from "./catalog/providerAccounts";

/** Listed models (optionally only local ones) with "Use model" to make one the default for new chats. */
export function ModelInventory({ localOnly = false }: { localOnly?: boolean }) {
  const catalog = useProviderCatalog();
  const { setModelId } = usePreferences();
  const { navigate } = useNavigation();
  const [error, setError] = useState<string>();
  const status = catalog.inference;
  const listed = (catalog.modelList?.models ?? []).filter(
    (model) => !localOnly || model.locality === "local",
  );
  const failures = (catalog.modelList?.failures ?? []).filter(
    (failure) => !localOnly || failure.provider_id === LOCAL_PROVIDER_ID,
  );
  return (
    <Stack gap={4}>
      {(error || catalog.error) && (
        <Notice tone="danger" role="alert">
          {error ?? catalog.error}
        </Notice>
      )}
      {failures.map((failure) => (
        <Notice key={failure.provider_id} tone="warning">
          {catalog.providers.find((provider) => provider.id === failure.provider_id)?.name ?? failure.provider_id}:{" "}
          {failure.message}
        </Notice>
      ))}
      <List metaWidth="150px">
        {listed.map((model) => {
          const key = nativeModelKey(model.model);
          const provider = catalog.providerFor(key);
          const source = SOURCE_LABELS[model.source];
          return (
            <ListRow
              key={key}
              media={
                provider ? (
                  <BrandIcon
                    brand={provider.brand}
                    title={hasBrandIcon(provider.brand) ? undefined : provider.name}
                    size={24}
                    aria-hidden
                  />
                ) : undefined
              }
              title={model.display_name}
              subtitle={<InlineCode>{key}</InlineCode>}
              meta={[
                model.locality === "local" ? "On this computer" : "Remote",
                contextLabel(model.context_window) ?? "Context unknown",
                <>
                  {capabilityBadges(model).map((label) => (
                    <Badge key={label} tone="neutral" size="sm">
                      {label}
                    </Badge>
                  ))}
                </>,
                source ? (
                  <Badge tone={model.source === "assumed" ? "warning" : "neutral"} size="sm" title={source.title}>
                    {source.label}
                  </Badge>
                ) : (
                  model.source
                ),
              ]}
              onSelect={() => navigate({ page: "model", id: key })}
              trailing={
                key === catalog.defaultKey ? (
                  <Badge tone="accent" size="sm">
                    Default
                  </Badge>
                ) : (
                  <Button
                    variant="outline"
                    onClick={() =>
                      void call("model.use", model.model)
                        .then(() => {
                          setModelId(key);
                          catalog.reload();
                        })
                        .catch((caught) => setError(toAidexError(caught).message))
                    }
                  >
                    Use model
                  </Button>
                )
              }
            />
          );
        })}
      </List>
      {!catalog.loading && !listed.length && (
        <Text tone="muted">
          {localOnly
            ? status?.state === "starting"
              ? `Loading ${status.model_name ?? "your local model"}…`
              : status?.state === "failed"
                ? "Local chat will be available after the installed model starts."
                : "Start an installed model above to make it available for chat."
            : "No models available. Connect a provider or start a local model."}
        </Text>
      )}
    </Stack>
  );
}
