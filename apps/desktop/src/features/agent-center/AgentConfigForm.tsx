import { useState, type FormEvent } from "react";
import { Notice, SectionHeader } from "@aidex/ui/components";
import {
  Button,
  Grid,
  Select,
  Stack,
  TextArea,
  TextField,
} from "@aidex/ui/primitives";
import { toAidexError } from "@aidex/client/errors";
import { usePreferences } from "../../app/providers/preferences";
import { useRpc } from "../../hooks/useRpc";
import { permissionOptions } from "./runtimeSelection";
import { SkillSelection } from "../extensions/SkillSelection";

export function AgentConfigForm() {
  const preferences = usePreferences();
  const models = useRpc("model.list");
  const skills = useRpc("extensions.skills.list");
  const [config, setConfig] = useState(() => ({ ...preferences.settings }));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [saved, setSaved] = useState(false);
  const options =
    models.data?.models
      .filter((model) => model.capabilities.chat)
      .map((model) => ({
        value: `${model.model.provider_id}/${model.model.model_id}`,
        label: model.display_name,
      })) ?? [];
  const save = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setError(undefined);
    setSaved(false);
    try {
      if (
        !Number.isInteger(config.max_steps) ||
        config.max_steps < 1 ||
        config.max_steps > 128 ||
        !Number.isInteger(config.max_tokens) ||
        config.max_tokens < 1 ||
        config.max_tokens > 32768
      )
        throw new Error("Use 1–128 steps and 1–32768 output tokens.");
      await preferences.updateSettings({
        model_id: config.model_id,
        instructions: config.instructions,
        skill_ids: config.skill_ids ?? [],
        fallback_model: config.fallback_model,
        permission_mode: config.permission_mode,
        max_steps: config.max_steps,
        max_tokens: config.max_tokens,
      });
      setSaved(true);
    } catch (caught) {
      setError(toAidexError(caught).message);
    } finally {
      setBusy(false);
    }
  };
  return (
    <form onSubmit={(event) => void save(event)}>
      <Stack gap={5}>
        {(error || models.error) && (
          <Notice tone="danger">{error ?? models.error?.message}</Notice>
        )}
        {saved && (
          <Notice tone="success">Defaults saved for new conversations.</Notice>
        )}
        <SectionHeader title="Instructions" />
        <TextArea
          label="Agent instructions"
          value={config.instructions}
          rows={8}
          maxLength={32768}
          onChange={(event) =>
            setConfig({ ...config, instructions: event.target.value })
          }
        />
        <Grid columns={2} gap={4}>
          <Select
            label="Default provider model"
            options={[{ value: "", label: "Use provider default" }, ...options]}
            value={config.model_id}
            onChange={(event) =>
              setConfig({ ...config, model_id: event.target.value })
            }
          />
          <Select
            label="Fallback provider model"
            options={[{ value: "", label: "No fallback" }, ...options]}
            value={config.fallback_model ?? ""}
            onChange={(event) =>
              setConfig({
                ...config,
                fallback_model: event.target.value || null,
              })
            }
          />
        </Grid>
        <Select
          label="Permission mode"
          options={permissionOptions}
          value={config.permission_mode ?? "supervised"}
          onChange={(event) =>
            setConfig({
              ...config,
              permission_mode: event.target
                .value as typeof config.permission_mode,
            })
          }
          hint="Auto currently uses supervised review. Shell execution on Windows requires full access."
        />
        <Grid columns={2} gap={4}>
          <TextField
            label="Maximum steps"
            type="number"
            required
            min={1}
            max={128}
            value={config.max_steps}
            onChange={(event) =>
              setConfig({ ...config, max_steps: Number(event.target.value) })
            }
          />
          <TextField
            label="Maximum output tokens"
            type="number"
            required
            min={1}
            max={32768}
            value={config.max_tokens}
            onChange={(event) =>
              setConfig({ ...config, max_tokens: Number(event.target.value) })
            }
          />
        </Grid>
        <SectionHeader title="Selected skill defaults" />
        <SkillSelection
          skills={skills.data ?? []}
          selectedIds={config.skill_ids ?? []}
          onChange={(skill_ids) => setConfig({ ...config, skill_ids })}
          disabled={busy}
          loading={skills.loading}
          error={skills.error?.message}
        />
        <Stack direction="row" gap={2}>
          <Button type="submit" disabled={busy}>
            {busy ? "Saving…" : "Save defaults"}
          </Button>
          <Button
            variant="ghost"
            disabled={busy}
            onClick={() => {
              setConfig({ ...preferences.settings });
              setSaved(false);
              setError(undefined);
            }}
          >
            Discard changes
          </Button>
        </Stack>
      </Stack>
    </form>
  );
}
