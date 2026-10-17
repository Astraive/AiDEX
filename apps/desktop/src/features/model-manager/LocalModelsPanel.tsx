import { useEffect, useRef, useState } from "react";
import { Dialog, List, ListRow, Notice } from "@aidex/ui/components";
import { TrashIcon } from "@aidex/ui/icons";
import {
  Button,
  Card,
  Select,
  Stack,
  Text,
  TextField,
} from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import type { InferenceBackend, ManagedModel } from "@aidex/client/dto/inference";
import { useRpc } from "../../hooks/useRpc";
import { firstControl, useRemovalFocus } from "../../hooks/useRemovalFocus";
import { ModelInventory } from "./ModelInventory";
import { LocalInferenceNotice } from "./LocalInferenceNotice";
import { useInferenceStatus } from "../../hooks/useInferenceStatus";

export function LocalModelsPanel() {
  const catalog = useRpc("inference.catalog");
  const status = useInferenceStatus();
  const [showImport, setShowImport] = useState(false);
  const [source, setSource] = useState("");
  const [runtimeSource, setRuntimeSource] = useState("");
  const [backend, setBackend] = useState<InferenceBackend>("auto");
  const [context, setContext] = useState("2048");
  const [threads, setThreads] = useState("2");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  // The remove confirmation; a failed remove is reported inside it.
  const [removing, setRemoving] = useState<ManagedModel>();
  const [removeBusy, setRemoveBusy] = useState(false);
  const [removeError, setRemoveError] = useState<string>();
  const starting = status.data?.state === "starting";
  const installed = !!catalog.data?.models.length;
  useEffect(() => {
    const settings = catalog.data?.settings;
    if (!settings) return;
    setBackend(settings.backend);
    setContext(String(settings.context_window ?? 2048));
    setThreads(String(settings.threads ?? 2));
  }, [
    catalog.data?.settings?.model_id,
    catalog.data?.settings?.backend,
    catalog.data?.settings?.context_window,
    catalog.data?.settings?.threads,
  ]);
  const act = async (operation: () => Promise<unknown>) => {
    setBusy(true);
    setError(undefined);
    try {
      await operation();
      catalog.refetch();
      status.refetch();
    } catch (caught) {
      setError(toAidexError(caught).message);
    } finally {
      setBusy(false);
    }
  };
  // After a remove, focus moves to the row that took its place, else to the import field shown
  // once no model is left.
  const importRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLUListElement>(null);
  const removalFocus = useRemovalFocus(
    catalog.data?.models.map((model) => model.id) ?? [],
    (index) => firstControl(listRef.current?.children[index]),
    () => importRef.current,
  );
  const remove = async (model: ManagedModel) => {
    setRemoveBusy(true);
    setRemoveError(undefined);
    try {
      await call("inference.model.remove", { id: model.id });
      removalFocus.removed(model.id);
      setRemoving(undefined);
      catalog.refetch();
      status.refetch();
    } catch (caught) {
      setRemoveError(toAidexError(caught).message);
    } finally {
      setRemoveBusy(false);
    }
  };
  const closeRemove = () => {
    if (removeBusy) return;
    setRemoving(undefined);
    setRemoveError(undefined);
  };
  return (
    <Stack gap={5}>
      {(error || catalog.error || status.error) && (
        <Notice tone="danger">
          {error ?? catalog.error?.message ?? status.error?.message}
        </Notice>
      )}
      <LocalInferenceNotice status={status.data} onRefresh={status.refetch} />
      <Card padding="md">
        <Stack gap={3}>
          <Text>
            llama.cpp:{" "}
            {catalog.data?.runtime
              ? `${catalog.data.runtime.release} · ${catalog.data.runtime.verified ? "verified" : "unverified"}`
              : catalog.data
                ? "Not installed"
                : "Checking runtime…"}
          </Text>
          {!!catalog.data && (!catalog.data.runtime || showImport) && (
            <>
              <TextField
                label="Verified runtime distribution folder"
                hint="Leave empty to use the runtime bundled on this drive. A runtime-manifest.json with file checksums is required."
                value={runtimeSource}
                onChange={(event) => setRuntimeSource(event.target.value)}
              />
              <Button
                disabled={busy || starting}
                variant="outline"
                onClick={() =>
                  void act(() =>
                    call(
                      "inference.runtime.install",
                      runtimeSource.trim()
                        ? { source: runtimeSource.trim() }
                        : {},
                    ),
                  )
                }
              >
                Install runtime
              </Button>
            </>
          )}
          {!!catalog.data && (!installed || showImport) && (
            <>
              <TextField
                ref={importRef}
                label="Import GGUF file"
                value={source}
                onChange={(event) => setSource(event.target.value)}
                hint="Absolute file path. The file is checked and copied to the portable drive."
              />
              <Button
                disabled={busy || starting || !source.trim()}
                onClick={() =>
                  void act(async () => {
                    await call("inference.import", { source: source.trim() });
                    setSource("");
                  })
                }
              >
                Import model
              </Button>
            </>
          )}
          {installed && (
            <>
              <Text tone="muted">
                Your installed models and runtime stay on this drive. AiDEX
                remembers the last started model and loads it automatically on
                launch.
              </Text>
              <Button
                variant="ghost"
                disabled={busy || starting}
                onClick={() => setShowImport((value) => !value)}
              >
                {showImport
                  ? "Hide setup options"
                  : "Add another model or replace runtime"}
              </Button>
            </>
          )}
          {busy && (
            <Button
              variant="outline"
              onClick={() =>
                void call("inference.cancel").catch((caught) =>
                  setError(toAidexError(caught).message),
                )
              }
            >
              Cancel operation
            </Button>
          )}
        </Stack>
      </Card>
      <Stack direction="row" gap={3} wrap>
        <Select
          label="Compute"
          value={backend}
          options={[
            { value: "auto", label: "Auto" },
            { value: "cpu", label: "CPU" },
            {
              value: "gpu",
              label: "GPU",
              disabled: !catalog.data?.runtime?.gpu_supported,
            },
          ]}
          onChange={(event) =>
            setBackend(event.target.value as InferenceBackend)
          }
        />
        <TextField
          label="Context tokens"
          value={context}
          onChange={(event) => setContext(event.target.value)}
        />
        <TextField
          label="CPU threads"
          value={threads}
          onChange={(event) => setThreads(event.target.value)}
        />
      </Stack>
      <Text>
        Inference {status.data?.state ?? "loading"}
        {status.data?.model_name ? ` · ${status.data.model_name}` : ""}
        {status.data?.backend ? ` · ${status.data.backend}` : ""}
      </Text>
      {status.data?.limitations.map((limitation) => (
        <Notice key={limitation} tone="warning">
          {limitation}
        </Notice>
      ))}
      {status.data?.state === "ready" && (
        <Button
          disabled={busy}
          variant="outline"
          onClick={() => void act(() => call("inference.stop"))}
        >
          Stop inference
        </Button>
      )}
      <List ref={listRef}>
        {catalog.data?.models.map((model) => (
          <ListRow
            key={model.id}
            title={model.name}
            subtitle={model.path}
            meta={[
              model.availability,
              `${(model.size_bytes / 1024 ** 3).toFixed(2)} GiB`,
              `GGUF v${model.gguf_version}`,
            ]}
            trailing={
              <Stack direction="row" gap={2}>
                <Button
                  disabled={
                    busy ||
                    starting ||
                    model.availability !== "available" ||
                    !catalog.data?.runtime ||
                    status.data?.state === "ready"
                  }
                  onClick={() =>
                    void act(() => {
                      const context_window = Number(context),
                        threadCount = Number(threads);
                      if (
                        !Number.isInteger(context_window) ||
                        context_window < 512 ||
                        !Number.isInteger(threadCount) ||
                        threadCount < 1
                      )
                        throw new Error(
                          "Enter valid positive integer limits (context at least 512).",
                        );
                      const start = call("inference.start", {
                        model_id: model.id,
                        backend,
                        context_window,
                        threads: threadCount,
                      });
                      status.refetch();
                      return start;
                    })
                  }
                >
                  {starting && status.data?.model_id === model.id
                    ? "Loading…"
                    : "Start"}
                </Button>
                <Button
                  disabled={
                    busy || starting || status.data?.model_id === model.id
                  }
                  variant="ghost"
                  aria-haspopup="dialog"
                  onClick={() => {
                    setRemoveError(undefined);
                    setRemoving(model);
                  }}
                >
                  Remove
                </Button>
              </Stack>
            }
          />
        ))}
      </List>
      {!!catalog.data && !catalog.data.models.length && (
        <Text tone="muted">Import a GGUF model to use local chat.</Text>
      )}
      <ModelInventory localOnly />
      <Dialog
        open={removing !== undefined}
        title={
          removing
            ? `Remove the managed copy of ${removing.name}?`
            : "Remove this model?"
        }
        description="The copy AiDEX keeps on this drive is deleted. The original import is preserved."
        onClose={closeRemove}
        returnFocus={removalFocus.returnFocus}
      >
        <Stack gap={4}>
          {removeError && (
            <Notice tone="danger" role="alert">
              {removeError}
            </Notice>
          )}
          <Stack direction="row" gap={2} justify="end">
            <Button variant="ghost" disabled={removeBusy} onClick={closeRemove}>
              Cancel
            </Button>
            <Button
              variant="primary"
              icon={<TrashIcon />}
              disabled={removeBusy || !removing}
              focusableWhenDisabled={removeBusy}
              onClick={() => removing && void remove(removing)}
            >
              {removeBusy ? "Removing…" : "Remove model"}
            </Button>
          </Stack>
        </Stack>
      </Dialog>
    </Stack>
  );
}
