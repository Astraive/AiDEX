import { Notice } from "@aidex/ui/components";
import { Button, Stack, Text } from "@aidex/ui/primitives";
import { useNavigation } from "../../app/providers/navigation";
import type { ConversationModelState } from "./resolveConversationModel";

type Props = {
  state: ConversationModelState;
  /** Name of the chosen model. */
  name: string;
  /** Name of a local model installed on the drive, when one is known. */
  installedLocalName?: string;
  /** Name of the conversation's own local model, for the "not running" message. */
  localName?: string;
  /** Whether any model can be chosen at all. */
  hasModels: boolean;
  loading: boolean;
  /** True while the local model loads or failed; `LocalInferenceNotice` explains those. */
  localNoticeShown: boolean;
};

/** Calm explanation of why the conversation cannot send yet, with the next step. */
export function ModelStateNotice({
  state,
  name,
  installedLocalName,
  localName,
  hasModels,
  loading,
  localNoticeShown,
}: Props) {
  const { navigate } = useNavigation();
  if (state === "ready" || state === "loading-local" || localNoticeShown) return null;
  if (loading) return <Notice role="status">Checking the models on this drive…</Notice>;

  const providersButton = (
    <Button variant="outline" onClick={() => navigate({ page: "models", tab: "providers" })}>
      Providers
    </Button>
  );
  const localButton = (
    <Button variant="outline" onClick={() => navigate({ page: "models", tab: "local" })}>
      Local models
    </Button>
  );

  if (state === "unlisted")
    return (
      <Notice tone="warning" role="status">
        <Stack gap={3}>
          <Text>
            {name} is not available right now. Choose another model, or check its provider in Providers.
          </Text>
          <Stack direction="row" gap={2} wrap>
            {providersButton}
          </Stack>
        </Stack>
      </Notice>
    );

  if (state === "local-stopped")
    return (
      <Notice role="status">
        <Stack gap={3}>
          <Text>
            {localName ?? name} is not running. Start it in Local models to use local chat, or
            choose another model.
          </Text>
          <Stack direction="row" gap={2} wrap>
            {localButton}
          </Stack>
        </Stack>
      </Notice>
    );

  return (
    <Notice role="status">
      <Stack gap={3}>
        <Text>
          {hasModels
            ? "Choose a model to start chatting."
            : installedLocalName
              ? `${installedLocalName} is installed on this drive. Start it in Local models, or connect a provider.`
              : "Connect a provider or start a local model to start chatting."}
        </Text>
        {!hasModels && (
          <Stack direction="row" gap={2} wrap>
            {providersButton}
            {localButton}
          </Stack>
        )}
      </Stack>
    </Notice>
  );
}
