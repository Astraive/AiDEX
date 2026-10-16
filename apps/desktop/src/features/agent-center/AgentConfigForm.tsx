import { useRef, useState, type FormEvent } from "react";
import { Notice, SectionHeader } from "@aidex/ui/components";
import { RefreshIcon } from "@aidex/ui/icons";
import { Button, ChoiceCardGroup, Divider, Grid, Select, Stack, TextArea, TextField } from "@aidex/ui/primitives";
import { toAidexError } from "@aidex/client/errors";
import type { PermissionMode } from "@aidex/client/dto/sessions";
import { usePreferences } from "../../app/providers/preferences";
import { useToast } from "../../app/providers/toast";
import { useShellNeedsFullAccess } from "../chat/continuation/engineCapabilities";
import { permissionChoices } from "../chat/continuation/permissions";
import { MAX_STEPS, MAX_TOKENS, fallbackModelOptions } from "../chat/sessionSettings";
import { useProviderCatalog } from "../model-manager/catalog/ProviderCatalog";
import { nativeModelKey, parseModelKey } from "../model-manager/catalog/modelKeys";
import {
  nativeDefaultsChanged,
  nativeDefaultsDraft,
  nativeDefaultsPatch,
  validateNativeDefaults,
  type NativeDefaultsDraft,
} from "./nativeDefaults";

/**
 * AiDEX Native's defaults for new conversations, saved encrypted on the drive (`settings.set`). Mount
 * once the preferences have loaded. Existing conversations keep their own copy (Conversation settings).
 */
export function AgentConfigForm() {
  const preferences = usePreferences();
  const catalog = useProviderCatalog();
  const notify = useToast();
  const shellNeedsFullAccess = useShellNeedsFullAccess(true);
  const [draft, setDraft] = useState<NativeDefaultsDraft>(() => nativeDefaultsDraft(preferences.settings));
  const [showErrors, setShowErrors] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string>();
  const formRef = useRef<HTMLFormElement>(null);
  const errors = showErrors ? validateNativeDefaults(draft) : {};
  const changed = nativeDefaultsChanged(draft, preferences.settings);

  const update = (patch: Partial<NativeDefaultsDraft>) => setDraft((current) => ({ ...current, ...patch }));
  const providerName = (key: string) => catalog.providerFor(key)?.name;

  const registryDefault = catalog.registryDefault ? nativeModelKey(catalog.registryDefault) : null;
  const registryName = registryDefault
    ? (catalog.models.find((model) => model.key === registryDefault)?.name ?? catalog.registryDefault?.model_id)
    : null;
  const modelOptions = [
    { value: "", label: registryName ? `Models default (${registryName})` : "No default: choose in each chat" },
    ...catalog.models
      .filter((model) => !model.unavailable)
      .map((model) => {
        const provider = providerName(model.key);
        return { value: model.key, label: provider ? `${model.name} · ${provider}` : model.name };
      }),
  ];
  if (draft.model_id && !modelOptions.some((option) => option.value === draft.model_id))
    modelOptions.push({
      value: draft.model_id,
      label: `${parseModelKey(draft.model_id)?.modelId ?? draft.model_id} (not available now)`,
    });
  const fallbackOptions = fallbackModelOptions({
    models: catalog.models,
    findModel: catalog.findModel,
    providerName,
    primaryKey: draft.model_id || registryDefault,
    current: draft.fallback_model || null,
  });
  const runtimeDefault = parseModelKey(draft.model_id)?.kind === "runtime";

  const save = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (saving) return;
    const problems = validateNativeDefaults(draft);
    if (Object.values(problems).some(Boolean)) {
      setShowErrors(true);
      // Errors render on the next commit; then focus the first invalid field.
      requestAnimationFrame(() => formRef.current?.querySelector<HTMLElement>("[aria-invalid='true']")?.focus());
      return;
    }
    setSaving(true);
    setError(undefined);
    try {
      await preferences.updateSettings(nativeDefaultsPatch(draft));
      setShowErrors(false);
      notify("Saved the defaults for new conversations.");
    } catch (caught) {
      setError(toAidexError(caught).message);
    } finally {
      setSaving(false);
    }
  };

  const discard = () => {
    setDraft(nativeDefaultsDraft(preferences.settings));
    setShowErrors(false);
    setError(undefined);
  };

  return (
    <form ref={formRef} onSubmit={(event) => void save(event)} noValidate aria-label="AiDEX Native defaults">
      <Stack gap={10}>
        <section>
          <Stack gap={4}>
            <SectionHeader title="Model" />
            <Grid columns={2} gap={4}>
              <Select
                label="Default model"
                hint="New chats start with it. Agent runtimes run as agents with full access."
                options={modelOptions}
                value={draft.model_id}
                aria-invalid={errors.model_id ? true : undefined}
                disabled={saving}
                onChange={(event) => update({ model_id: event.target.value })}
              />
              <Select
                label="Fallback model"
                hint={
                  runtimeDefault
                    ? "Only chats on an AiDEX Native model use it; agent runtimes ignore it."
                    : "Tried once when the model fails with a temporary error. In Agent mode it must call tools natively, or that attempt fails."
                }
                options={fallbackOptions}
                value={draft.fallback_model}
                aria-invalid={errors.fallback_model ? true : undefined}
                disabled={saving}
                onChange={(event) => update({ fallback_model: event.target.value })}
              />
            </Grid>
            {(errors.model_id || errors.fallback_model) && (
              <Notice tone="danger" role="alert">
                {errors.model_id ?? errors.fallback_model}
              </Notice>
            )}
            {catalog.error && <Notice tone="warning">{catalog.error}</Notice>}
          </Stack>
        </section>
        <section>
          <Stack gap={4}>
            <SectionHeader title="Permission" />
            <ChoiceCardGroup<PermissionMode>
              label="Permission for Plan and Agent"
              variant="rows"
              options={permissionChoices({ shellNeedsFullAccess }, draft.permission_mode).map((option) => ({
                ...option,
                disabled: saving,
              }))}
              value={draft.permission_mode}
              onValueChange={(permission_mode) => update({ permission_mode })}
            />
          </Stack>
        </section>
        <section>
          <Stack gap={4}>
            <SectionHeader title="Instructions" />
            <TextArea
              label="Instructions"
              hint="Sent to the model at the start of every run of a new conversation. Up to 32 KiB."
              monospace
              rows={8}
              spellCheck={false}
              value={draft.instructions}
              error={errors.instructions}
              disabled={saving}
              onChange={(event) => update({ instructions: event.target.value })}
            />
          </Stack>
        </section>
        <section>
          <Stack gap={4}>
            <SectionHeader title="Limits" />
            <Grid columns={2} gap={4}>
              <TextField
                label="Maximum steps"
                size="md"
                type="number"
                inputMode="numeric"
                min={1}
                max={MAX_STEPS}
                hint={`Model requests per run, 1 to ${MAX_STEPS}.`}
                value={draft.max_steps}
                error={errors.max_steps}
                disabled={saving}
                onChange={(event) => update({ max_steps: event.target.value })}
              />
              <TextField
                label="Maximum output tokens"
                size="md"
                type="number"
                inputMode="numeric"
                min={1}
                max={MAX_TOKENS}
                hint={`Per model response, 1 to ${MAX_TOKENS.toLocaleString("en-US")}.`}
                value={draft.max_tokens}
                error={errors.max_tokens}
                disabled={saving}
                onChange={(event) => update({ max_tokens: event.target.value })}
              />
            </Grid>
          </Stack>
        </section>
        <Stack gap={6}>
          <Divider />
          {error && (
            <Notice tone="danger" role="alert">
              {error}
            </Notice>
          )}
          <Stack direction="row" gap={3} justify="end" wrap>
            <Button variant="ghost" size="md" icon={<RefreshIcon />} disabled={saving || !changed} onClick={discard}>
              Discard changes
            </Button>
            <Button variant="primary" size="md" type="submit" disabled={saving || !changed}>
              {saving ? "Saving…" : "Save defaults"}
            </Button>
          </Stack>
        </Stack>
      </Stack>
    </form>
  );
}
