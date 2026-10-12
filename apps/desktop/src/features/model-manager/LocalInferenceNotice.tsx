import { useState } from "react";
import { Notice } from "@aidex/ui/components";
import { Button, Stack, Text } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import type { InferenceStatus } from "@aidex/client/dto/inference";
import { useNavigation } from "../../app/providers/navigation";

export function LocalInferenceNotice({
  status,
  onRefresh,
}: {
  status: InferenceStatus | undefined;
  onRefresh: () => void;
}) {
  const [retrying, setRetrying] = useState(false);
  const { navigate } = useNavigation();
  const [error, setError] = useState<string>();
  if (status?.state !== "starting" && status?.state !== "failed") return null;
  const name = status.model_name ?? "your saved local model";
  return (
    <Notice
      tone={status.state === "failed" ? "danger" : "info"}
      role={status.state === "failed" ? "alert" : "status"}
    >
      <Stack gap={3}>
        <strong>
          {status.state === "starting"
            ? `Loading ${name}`
            : `Could not start ${name}`}
        </strong>
        <Text>
          {status.state === "starting"
            ? "Preparing local chat from this drive. You can send a message when loading finishes."
            : (status.startup_error ??
              "Check the local model and runtime in Local models, then retry.")}
        </Text>
        {error && <Text>{error}</Text>}
        {status.state === "failed" && (
          <Stack direction="row" gap={2} wrap>
            <Button
              variant="outline"
              disabled={retrying}
              onClick={() => {
                setRetrying(true);
                setError(undefined);
                void Promise.resolve()
                  .then(async () => {
                    if (!status.model_id)
                      throw new Error(
                        "Choose an installed model in Local models to start local chat.",
                      );
                    const start = call("inference.start", {
                      model_id: status.model_id,
                      backend: status.backend ?? "auto",
                      context_window: status.context_window ?? undefined,
                      threads: status.threads ?? undefined,
                    });
                    onRefresh();
                    await start;
                  })
                  .catch((caught) => setError(toAidexError(caught).message))
                  .finally(() => {
                    setRetrying(false);
                    onRefresh();
                  });
              }}
            >
              {retrying ? "Retrying…" : "Retry local model"}
            </Button>
            <Button
              variant="ghost"
              onClick={() => navigate({ page: "models", tab: "local" })}
            >
              Local models
            </Button>
          </Stack>
        )}
      </Stack>
    </Notice>
  );
}
