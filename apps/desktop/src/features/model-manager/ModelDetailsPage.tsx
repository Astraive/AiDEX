import { useState } from "react";
import { InlineCode, KeyValueList, Notice, PageHeader } from "@aidex/ui/components";
import { PageFrame } from "@aidex/ui/desktop";
import { BrandIcon, hasBrandIcon, StarFilledIcon, StarIcon } from "@aidex/ui/icons";
import { Badge, Button, Stack, Text } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import { useNavigation } from "../../app/providers/navigation";
import { usePreferences } from "../../app/providers/preferences";
import { useProviderCatalog } from "./catalog/ProviderCatalog";
import {
  capabilityBadges,
  contextLabel,
  SOURCE_LABELS,
  TOOL_LABELS,
} from "./catalog/modelFacts";
import { nativeModelKey } from "./catalog/modelKeys";

export function ModelDetailsPage() {
  const { route, navigate } = useNavigation();
  const preferences = usePreferences();
  const catalog = useProviderCatalog();
  const [error, setError] = useState<string>();
  const model = catalog.findModel(route.id);
  const key = model ? nativeModelKey(model.model) : (route.id ?? "");
  const provider = catalog.providerFor(key);
  const favorite = catalog.favorites.has(key);
  const isDefault = key === catalog.defaultKey;
  const source = model ? SOURCE_LABELS[model.source] : undefined;
  return (
    <PageFrame width="default">
      <PageHeader
        title={model?.display_name ?? "Model"}
        eyebrow={provider?.name ?? model?.model.provider_id}
        back={{ label: "Models", onClick: () => navigate({ page: "models" }) }}
        meta={
          model ? (
            <Stack direction="row" gap={2} wrap>
              {isDefault && <Badge tone="accent">Default</Badge>}
              {capabilityBadges(model).map((label) => (
                <Badge key={label} tone="neutral">
                  {label}
                </Badge>
              ))}
              {source && (
                <Badge tone={model.source === "assumed" ? "warning" : "neutral"} title={source.title}>
                  {source.label}
                </Badge>
              )}
            </Stack>
          ) : undefined
        }
        actions={
          model ? (
            <Stack direction="row" gap={2}>
              <Button
                variant="ghost"
                icon={favorite ? <StarFilledIcon /> : <StarIcon />}
                aria-pressed={favorite}
                onClick={() => catalog.toggleFavorite(key)}
              >
                {favorite ? "Favorite" : "Add to favorites"}
              </Button>
              <Button
                variant="primary"
                disabled={isDefault}
                onClick={() =>
                  void call("model.use", model.model)
                    .then(() => {
                      preferences.setModelId(key);
                      catalog.reload();
                    })
                    .catch((caught) => setError(toAidexError(caught).message))
                }
              >
                {isDefault ? "Default for new chats" : "Use this model"}
              </Button>
            </Stack>
          ) : undefined
        }
      />
      {(error || catalog.error) && (
        <Notice tone="danger" role="alert">
          {error ?? catalog.error}
        </Notice>
      )}
      {model ? (
        <KeyValueList
          items={[
            {
              label: "Provider",
              value: provider ? (
                <Stack direction="row" gap={2} align="center">
                  <BrandIcon
                    brand={provider.brand}
                    title={hasBrandIcon(provider.brand) ? undefined : provider.name}
                    size={16}
                    aria-hidden
                  />
                  <Text as="span">{provider.name}</Text>
                </Stack>
              ) : (
                model.model.provider_id
              ),
            },
            { label: "Model ID", value: <InlineCode>{model.model.model_id}</InlineCode> },
            { label: "Runs", value: model.locality === "local" ? "On this computer" : "Remote service" },
            { label: "Context window", value: contextLabel(model.context_window) ?? "Unknown" },
            {
              label: "Output limit",
              value: model.max_output_tokens ? `${model.max_output_tokens.toLocaleString("en-US")} tokens` : "Unknown",
            },
            { label: "Tool calls", value: TOOL_LABELS[model.capabilities.tool_calls] },
            {
              label: "Use",
              value: model.capabilities.chat
                ? model.capabilities.streaming
                  ? "Chat"
                  : "Chat without streaming (not usable in AiDEX conversations)"
                : model.capabilities.embeddings
                  ? "Embeddings"
                  : "Not a chat model",
            },
            ...(source ? [{ label: "Limits from", value: `${source.label} · ${source.title}` }] : []),
            ...(model.cost
              ? [
                  {
                    label: "Price",
                    value: `$${model.cost.input_per_million} in · $${model.cost.output_per_million} out per million tokens`,
                  },
                ]
              : []),
          ]}
        />
      ) : (
        !catalog.loading && (
          <Notice>
            This model is no longer listed. Reconnect its provider in Providers or start it in Local models.
          </Notice>
        )
      )}
    </PageFrame>
  );
}
