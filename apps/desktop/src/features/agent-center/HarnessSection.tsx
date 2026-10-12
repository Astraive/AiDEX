import { SectionHeader } from "@aidex/ui/components";
import { Grid, SegmentedControl, Select, Stack, Text, TextField } from "@aidex/ui/primitives";
import {
  MAX_STEPS_LIMIT,
  TIMEOUT_LIMIT_MINUTES,
  autonomyOptions,
  sandboxOptions,
  type ConfigErrors,
} from "./agentConfig";
import type { AgentConfig } from "./sample";

type HarnessFields = Pick<AgentConfig, "autonomy" | "maxSteps" | "sandbox" | "timeoutMinutes">;

type Props = {
  values: HarnessFields;
  errors: ConfigErrors;
  /** Receives only the fields that changed. */
  onChange: (patch: Partial<HarnessFields>) => void;
};

/** How far the agent may go on its own: approval mode, step budget, sandbox, and time limit. */
export function HarnessSection({ values, errors, onChange }: Props) {
  return (
    <section>
      <Stack gap={5}>
        <SectionHeader title="Harness" />
        <Stack gap={2} align="start">
          <Text as="span" variant="label">
            Autonomy
          </Text>
          <SegmentedControl
            label="Autonomy"
            options={autonomyOptions}
            value={values.autonomy}
            onValueChange={(value) => onChange({ autonomy: value })}
          />
        </Stack>
        <Grid columns={3} gap={4}>
          <TextField
            label="Max steps"
            size="md"
            type="number"
            inputMode="numeric"
            min={1}
            max={MAX_STEPS_LIMIT}
            value={values.maxSteps}
            error={errors.maxSteps}
            onChange={(event) => onChange({ maxSteps: event.target.value })}
          />
          <Select
            label="Sandbox level"
            options={sandboxOptions}
            value={values.sandbox}
            onChange={(event) => onChange({ sandbox: event.target.value })}
          />
          <TextField
            label="Timeout (minutes)"
            size="md"
            type="number"
            inputMode="numeric"
            min={1}
            max={TIMEOUT_LIMIT_MINUTES}
            value={values.timeoutMinutes}
            error={errors.timeoutMinutes}
            onChange={(event) => onChange({ timeoutMinutes: event.target.value })}
          />
        </Grid>
      </Stack>
    </section>
  );
}
