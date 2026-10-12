import { useRef, useState, type FormEvent } from "react";
import { SectionHeader } from "@aidex/ui/components";
import { RefreshIcon } from "@aidex/ui/icons";
import { Button, Divider, Grid, Select, Stack, TextArea } from "@aidex/ui/primitives";
import { useToast } from "../../app/providers/toast";
import { agentModelOptions, fallbackModelOptions, hasErrors, validateConfig } from "./agentConfig";
import { HarnessSection } from "./HarnessSection";
import { PermissionsSection } from "./PermissionsSection";
import type { AgentConfig, SampleAgent } from "./sample";

type Props = { agent: SampleAgent };

/**
 * Editable copy of an agent's settings. Mount with `key={agent.id}` so switching agents starts from that agent's
 * defaults. Nothing is persisted: Save explains that Core is needed, Reset restores the sample defaults.
 */
export function AgentConfigForm({ agent }: Props) {
  const notify = useToast();
  const [config, setConfig] = useState<AgentConfig>(agent.config);
  const [showErrors, setShowErrors] = useState(false);
  const formRef = useRef<HTMLFormElement>(null);
  const errors = showErrors ? validateConfig(config) : {};

  const update = <Key extends keyof AgentConfig>(key: Key, value: AgentConfig[Key]) =>
    setConfig((current) => ({ ...current, [key]: value }));

  const save = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (hasErrors(validateConfig(config))) {
      setShowErrors(true);
      // Errors render on the next commit; then move focus to the first invalid field, as the vault form does.
      requestAnimationFrame(() => formRef.current?.querySelector<HTMLElement>("[aria-invalid='true']")?.focus());
      return;
    }
    setShowErrors(false);
    notify(`Preview: saving settings for ${agent.name} will be available when AiDEX Core is connected.`);
  };

  const reset = () => {
    setConfig(agent.config);
    setShowErrors(false);
    notify(`Restored the default settings for ${agent.name}.`);
  };

  return (
    <form ref={formRef} onSubmit={save} noValidate aria-label={`${agent.name} settings`}>
      <Stack gap={10}>
        <section>
          <Stack gap={4}>
            <SectionHeader title="Instructions" />
            <TextArea
              label="System prompt"
              hint="Sent at the start of every run. Workspace rules files are added after it."
              monospace
              rows={8}
              spellCheck={false}
              value={config.instructions}
              onChange={(event) => update("instructions", event.target.value)}
            />
          </Stack>
        </section>
        <section>
          <Stack gap={4}>
            <SectionHeader title="Model" />
            <Grid columns={2} gap={4}>
              <Select
                label="Default model"
                options={agentModelOptions}
                value={config.modelId}
                onChange={(event) => update("modelId", event.target.value)}
              />
              <Select
                label="Fallback model"
                hint="Used when the default model is unavailable."
                options={fallbackModelOptions}
                value={config.fallbackModelId}
                onChange={(event) => update("fallbackModelId", event.target.value)}
              />
            </Grid>
          </Stack>
        </section>
        <PermissionsSection permissions={config.permissions} onChange={(permissions) => update("permissions", permissions)} />
        <HarnessSection
          values={config}
          errors={errors}
          onChange={(patch) => setConfig((current) => ({ ...current, ...patch }))}
        />
        <Stack gap={6}>
          <Divider />
          <Stack direction="row" gap={3} justify="end" wrap>
            <Button variant="ghost" size="md" icon={<RefreshIcon />} onClick={reset}>
              Reset
            </Button>
            <Button variant="primary" size="md" type="submit">
              Save
            </Button>
          </Stack>
        </Stack>
      </Stack>
    </form>
  );
}
