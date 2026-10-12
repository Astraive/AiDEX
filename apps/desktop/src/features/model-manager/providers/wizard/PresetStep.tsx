import { useState } from "react";
import { Notice, ProviderTile } from "@aidex/ui/components";
import { Badge, Grid, SearchField, SegmentedControl, Stack, Text } from "@aidex/ui/primitives";
import type { ProviderPreset } from "@aidex/client/dto/models";
import type { RuntimeDriverInfo } from "@aidex/client/dto/runtime";
import { runtimeBrand, runtimeDriverName } from "../../catalog/runtimes";
import {
  CATEGORY_OPTIONS,
  presetBadges,
  presetMatches,
  runtimeMatches,
  type WizardCategory,
} from "./wizardLogic";

type Props = {
  presets: readonly ProviderPreset[];
  drivers: readonly RuntimeDriverInfo[];
  /** Why runtimes cannot be added right now, if they cannot. */
  runtimesNotice: string | null;
  onPreset: (preset: ProviderPreset) => void;
  onRuntime: (driver: RuntimeDriverInfo) => void;
};

const badgeRow = (labels: readonly string[]) => (
  <>
    {labels.map((label) => (
      <Badge key={label} tone="neutral" size="sm">
        {label}
      </Badge>
    ))}
  </>
);

/** Step 1: choose a provider preset or an agent runtime driver. */
export function PresetStep({ presets, drivers, runtimesNotice, onPreset, onRuntime }: Props) {
  const [category, setCategory] = useState<WizardCategory>("all");
  const [query, setQuery] = useState("");
  const shownPresets = presets.filter((preset) => presetMatches(preset, category, query));
  const shownDrivers = category === "all" ? drivers.filter((driver) => runtimeMatches(driver, query)) : [];
  return (
    <Stack gap={4}>
      <SegmentedControl
        label="Provider category"
        size="sm"
        options={CATEGORY_OPTIONS}
        value={category}
        onValueChange={setCategory}
      />
      <SearchField
        label="Search providers"
        placeholder="Search providers"
        size="md"
        autoFocus
        value={query}
        onChange={(event) => setQuery(event.target.value)}
      />
      <Stack gap={2}>
        <Text variant="label" tone="muted">
          Model providers
        </Text>
        {shownPresets.length ? (
          <Grid columns={3} gap={3}>
            {shownPresets.map((preset) => (
              <ProviderTile
                key={preset.id}
                brand={preset.icon || "custom"}
                name={preset.display_name}
                description={preset.description || undefined}
                badges={badgeRow(presetBadges(preset))}
                onSelect={() => onPreset(preset)}
              />
            ))}
          </Grid>
        ) : (
          <Text tone="muted">No providers match.</Text>
        )}
      </Stack>
      {category === "all" && (
        <Stack gap={2}>
          <Text variant="label" tone="muted">
            Agent runtimes
          </Text>
          {runtimesNotice ? (
            <Notice>{runtimesNotice}</Notice>
          ) : shownDrivers.length ? (
            <Grid columns={3} gap={3}>
              {shownDrivers.map((driver) => (
                <ProviderTile
                  key={driver.driver}
                  brand={runtimeBrand(driver.driver)}
                  name={runtimeDriverName(driver.driver, drivers)}
                  description={driver.package ?? `Runs ${driver.name} through ${driver.protocol}.`}
                  badges={badgeRow(["Agent runtime"])}
                  onSelect={() => onRuntime(driver)}
                />
              ))}
            </Grid>
          ) : (
            <Text tone="muted">No runtimes match.</Text>
          )}
        </Stack>
      )}
    </Stack>
  );
}
