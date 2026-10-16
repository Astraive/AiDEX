import { useEffect, useRef, useState, type FormEvent } from "react";
import { Dialog, Notice } from "@aidex/ui/components";
import { Button, Grid, Select, Stack, TextArea, TextField } from "@aidex/ui/primitives";
import { toAidexError } from "@aidex/client/errors";
import {
  MAX_STEPS,
  MAX_TOKENS,
  draftOf,
  hasErrors,
  sessionSettingsErrors,
  settingsOf,
  type SessionSettings,
  type SessionSettingsDraft,
} from "./sessionSettings";

export type ConversationSettingsDialogProps = {
  open: boolean;
  onClose: () => void;
  /** The conversation's saved values; each opening starts from them. */
  settings: SessionSettings;
  /** Runs on an external runtime, which takes no fallback model. */
  external: boolean;
  /** Fallback choices (`fallbackModelOptions`), "No fallback" first. */
  fallbackOptions: readonly { value: string; label: string }[];
  /** A run is active; its settings were fixed when it started, so changes wait until it ends. */
  locked: boolean;
  /** Saves the whole configuration with these values (`agent.configuration.set`). */
  onSave: (settings: SessionSettings) => Promise<void>;
};

/**
 * Instructions, fallback model, and step and token limits of one conversation, checked against the
 * daemon's bounds. They apply from the next run on; new conversations keep using the defaults in
 * Agents → AiDEX Native.
 */
export function ConversationSettingsDialog({
  open,
  onClose,
  settings,
  external,
  fallbackOptions,
  locked,
  onSave,
}: ConversationSettingsDialogProps) {
  const [draft, setDraft] = useState<SessionSettingsDraft>(() => draftOf(settings));
  const [showErrors, setShowErrors] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string>();
  const formRef = useRef<HTMLFormElement>(null);
  // Each opening starts from the conversation as it is now.
  useEffect(() => {
    if (!open) return;
    setDraft(draftOf(settings));
    setShowErrors(false);
    setError(undefined);
  }, [open]);
  const errors = showErrors ? sessionSettingsErrors(draft) : {};
  const disabled = locked || saving;
  const update = (patch: Partial<SessionSettingsDraft>) => setDraft((current) => ({ ...current, ...patch }));
  const save = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (disabled) return;
    if (hasErrors(sessionSettingsErrors(draft))) {
      setShowErrors(true);
      // Errors render on the next commit; then focus the first invalid field.
      requestAnimationFrame(() => formRef.current?.querySelector<HTMLElement>("[aria-invalid='true']")?.focus());
      return;
    }
    setSaving(true);
    setError(undefined);
    try {
      await onSave(settingsOf(draft));
      onClose();
    } catch (caught) {
      setError(toAidexError(caught).message);
    } finally {
      setSaving(false);
    }
  };
  return (
    <Dialog
      open={open}
      title="Conversation settings"
      description="Used by this conversation's next runs. New conversations start from the defaults in Agents."
      size="lg"
      onClose={() => {
        if (!saving) onClose();
      }}
    >
      <form ref={formRef} onSubmit={(event) => void save(event)} noValidate aria-label="Conversation settings">
        <Stack gap={4}>
          {locked && (
            <Notice tone="info">
              A run is in progress. Change these settings once it finishes, or stop it first.
            </Notice>
          )}
          <TextArea
            label="Instructions"
            hint="Sent to the model at the start of every run of this conversation. Up to 32 KiB."
            monospace
            rows={6}
            spellCheck={false}
            value={draft.instructions}
            error={errors.instructions}
            disabled={disabled}
            onChange={(event) => update({ instructions: event.target.value })}
          />
          {!external && (
            <Select
              label="Fallback model"
              hint="Tried once when the model fails with a temporary error."
              value={draft.fallback_model}
              options={fallbackOptions}
              aria-invalid={errors.fallback_model ? true : undefined}
              disabled={disabled}
              onChange={(event) => update({ fallback_model: event.target.value })}
            />
          )}
          {errors.fallback_model && (
            <Notice tone="danger" role="alert">
              {errors.fallback_model}
            </Notice>
          )}
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
              disabled={disabled}
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
              disabled={disabled}
              onChange={(event) => update({ max_tokens: event.target.value })}
            />
          </Grid>
          {error && (
            <Notice tone="danger" role="alert">
              {error}
            </Notice>
          )}
          <Stack direction="row" gap={2} justify="end">
            <Button variant="ghost" disabled={saving} onClick={onClose}>
              Cancel
            </Button>
            <Button type="submit" variant="primary" disabled={disabled}>
              {saving ? "Saving…" : "Save"}
            </Button>
          </Stack>
        </Stack>
      </form>
    </Dialog>
  );
}
