import { useState, type FormEvent } from "react";
import { InlineCode, List, ListRow, Notice, SettingsSection } from "@aidex/ui/components";
import { PlusIcon, SlidersIcon, TrashIcon } from "@aidex/ui/icons";
import {
  Button,
  Checkbox,
  Grid,
  IconButton,
  Select,
  Stack,
  Text,
  TextField,
} from "@aidex/ui/primitives";
import type { ModelConfig, ToolCallSupport } from "@aidex/client/dto/models";
import { contextLabel, TOOL_LABELS } from "../catalog/modelFacts";
import type { AccountItem } from "./providerItems";
import { useProviderActions } from "./useProviderActions";

/** `aidex_providers::ModelId`: 1-256 ASCII letters, digits, and `. _ - : / @ +`. */
const MODEL_ID = /^[A-Za-z0-9._\-:/@+]{1,256}$/;
const MAX_CONTEXT = 10_000_000;

const toolOptions = [
  { value: "native", label: TOOL_LABELS.native },
  { value: "structured", label: TOOL_LABELS.structured },
  { value: "none", label: TOOL_LABELS.none },
];

type Draft = {
  id: string;
  name: string;
  context: string;
  tools: ToolCallSupport;
  vision: boolean;
  json: boolean;
};

const emptyDraft = (tools: ToolCallSupport, json: boolean): Draft => ({
  id: "",
  name: "",
  context: "32000",
  tools,
  vision: false,
  json,
});

/** Models declared for this account, added to (or overriding) whatever the provider lists. */
export function CustomModelsEditor({ item }: { item: AccountItem }) {
  const actions = useProviderActions();
  const { account, preset } = item;
  const declared = account.declared_models;
  const defaultTools: ToolCallSupport = preset?.tool_calls === false ? "none" : "native";
  const defaultJson = preset?.json_schema_output ?? false;
  const [draft, setDraft] = useState<Draft>(() => emptyDraft(defaultTools, defaultJson));
  const [errors, setErrors] = useState<{ id?: string; context?: string }>({});

  const validate = (): { id?: string; context?: string } => {
    const id = draft.id.trim();
    const context = Number(draft.context);
    return {
      id: !id
        ? "Enter the model ID the provider uses."
        : !MODEL_ID.test(id)
          ? "Use letters, digits, and . _ - : / @ + only."
          : declared.some((model) => model.id === id)
            ? "That custom model is already saved."
            : undefined,
      context:
        !Number.isSafeInteger(context) || context < 1 || context > MAX_CONTEXT
          ? "Enter the model's documented context limit in tokens."
          : undefined,
    };
  };

  const add = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const found = validate();
    setErrors(found);
    if (found.id || found.context) return;
    const model: ModelConfig = {
      id: draft.id.trim(),
      display_name: draft.name.trim() || null,
      context_window: Number(draft.context),
      tool_calls: draft.tools,
      vision: draft.vision,
      json_schema_output: draft.json,
    };
    if (await actions.updateAccount(account, { models: [...declared, model] }))
      setDraft(emptyDraft(defaultTools, defaultJson));
  };

  const remove = (id: string) =>
    void actions.updateAccount(account, { models: declared.filter((model) => model.id !== id) });

  return (
    <SettingsSection
      title="Custom models"
      icon={<SlidersIcon />}
      description="Declare models the provider does not list, or set limits it does not report. Declarations override listed metadata."
    >
      {declared.length ? (
        <List metaWidth="150px">
          {declared.map((model) => (
            <ListRow
              key={model.id}
              title={model.display_name || model.id}
              subtitle={<InlineCode>{model.id}</InlineCode>}
              meta={[
                contextLabel(model.context_window) ?? "Context unknown",
                TOOL_LABELS[model.tool_calls ?? "none"],
              ]}
              trailing={
                <IconButton
                  size="sm"
                  label={`Remove ${model.id}`}
                  icon={<TrashIcon />}
                  disabled={actions.busy !== null}
                  onClick={() => remove(model.id)}
                />
              }
            />
          ))}
        </List>
      ) : (
        <Text tone="muted">No custom models yet.</Text>
      )}
      <form onSubmit={(event) => void add(event)} noValidate aria-label="Add a custom model">
        <Stack gap={3}>
          <Grid columns={2} gap={3}>
            <TextField
              label="Model ID"
              size="md"
              spellCheck={false}
              autoCapitalize="off"
              value={draft.id}
              error={errors.id}
              onChange={(event) => setDraft({ ...draft, id: event.target.value })}
            />
            <TextField
              label="Display name (optional)"
              size="md"
              value={draft.name}
              onChange={(event) => setDraft({ ...draft, name: event.target.value })}
            />
            <TextField
              label="Context window"
              size="md"
              inputMode="numeric"
              value={draft.context}
              error={errors.context}
              onChange={(event) => setDraft({ ...draft, context: event.target.value })}
            />
            <Select
              label="Tool calls"
              value={draft.tools}
              options={toolOptions}
              onChange={(event) => setDraft({ ...draft, tools: event.target.value as ToolCallSupport })}
            />
          </Grid>
          <Stack direction="row" gap={5} wrap>
            <Checkbox
              label="Vision"
              checked={draft.vision}
              onChange={(event) => setDraft({ ...draft, vision: event.target.checked })}
            />
            <Checkbox
              label="JSON schema output"
              checked={draft.json}
              onChange={(event) => setDraft({ ...draft, json: event.target.checked })}
            />
          </Stack>
          <Stack direction="row" gap={2}>
            <Button type="submit" variant="outline" icon={<PlusIcon />} disabled={actions.busy !== null}>
              {actions.busy === "update" ? "Saving…" : "Add model"}
            </Button>
          </Stack>
        </Stack>
      </form>
      {actions.error && (
        <Notice tone="danger" role="alert">
          {actions.error}
        </Notice>
      )}
    </SettingsSection>
  );
}
