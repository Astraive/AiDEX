import { Notice, PageHeader, SectionHeader } from "@aidex/ui/components";
import { FolderIcon, RefreshIcon } from "@aidex/ui/icons";
import { Badge, ChoiceCardGroup, IconButton, Stack, Text, TextField, type ChoiceCardOption } from "@aidex/ui/primitives";
import { isEligibleVolume, removableLabels, type VolumeInfo } from "@aidex/client/dto/drive";
import { ErrorKind } from "@aidex/client/errors";
import { formatBytes } from "@aidex/client/format";
import { estimateGb, fitsOnDrive, formatGb, type InstallMode } from "../../lib/setup";
import { stepKicker } from "../../lib/steps";
import type { NormalTargetQuery } from "../../lib/useNormalTarget";
import type { DrivesQuery } from "../../lib/useDrives";
import type { StepProps } from "../types";

type Props = StepProps & { drives: DrivesQuery; normalTarget: NormalTargetQuery };

function volumeBadge(volume: VolumeInfo) {
  if (volume.system) return <Badge tone="neutral">System drive</Badge>;
  if (volume.has_aidex_manifest) {
    return <Badge tone="accent">{volume.schema_version ? `AiDEX drive · schema v${volume.schema_version}` : "Unreadable AiDEX manifest"}</Badge>;
  }
  if (!isEligibleVolume(volume)) return <Badge tone="neutral">{volume.removable === "fixed" ? "Internal disk" : "Not supported"}</Badge>;
  return <Badge tone="success">Ready</Badge>;
}

function option(volume: VolumeInfo): ChoiceCardOption<string> {
  const capacity =
    volume.total_bytes === null ? "capacity unknown" : `${formatBytes(volume.free_bytes)} free of ${formatBytes(volume.total_bytes)}`;
  return {
    value: volume.root,
    title: `${volume.label ?? "Untitled drive"} (${volume.root})`,
    description: [removableLabels[volume.removable], volume.filesystem, capacity].filter(Boolean).join(" · "),
    meta: volumeBadge(volume),
    disabled: !isEligibleVolume(volume),
  };
}
const installModeOptions: readonly ChoiceCardOption<InstallMode>[] = [
  {
    value: "normal",
    title: "Standard",
    description: "Prepare a per-user install on this computer. No administrator access is required.",
    meta: "This computer",
  },
  {
    value: "portable",
    title: "Portable",
    description: "Prepare AiDEX on a removable USB drive so the target can travel with you.",
    meta: "USB drive",
  },
];


/** Step 1: choose a per-user target or a removable drive for a portable setup. */
export function DriveStep({ state, update, titleRef, volume, drives, normalTarget }: Props) {
  const isNormal = state.installMode === "normal";
  const options = drives.volumes.map(option);
  const eligible = drives.volumes.filter(isEligibleVolume);
  const targetRoot = isNormal ? normalTarget.inspection?.root : volume?.root;
  const inspection = normalTarget.inspection;

  return (
    <>
      <PageHeader
        eyebrow={stepKicker(0)}
        title="Choose an install type"
        description="Prepare a standard per-user location or a portable USB drive."
        titleRef={titleRef}
      />
      <ChoiceCardGroup
        label="Installation type"
        variant="rows"
        options={installModeOptions}
        value={state.installMode}
        onValueChange={(installMode) => update({ installMode, driveRoot: null })}
      />
      <Stack gap={3}>
        <SectionHeader
          title={isNormal ? "Standard per-user location" : "Portable target drive"}
          level={3}
          meta={
            isNormal
              ? normalTarget.loading
                ? "Checking…"
                : "Local app data · no admin access"
              : drives.loading
                ? "Scanning…"
                : `${eligible.length} of ${drives.volumes.length} drives can be used`
          }
          actions={
            <IconButton
              label={isNormal ? "Refresh installation target" : "Rescan drives"}
              icon={<RefreshIcon />}
              variant="soft"
              size="lg"
              onClick={isNormal ? normalTarget.refresh : drives.rescan}
            />
          }
        />
        {isNormal ? (
          <>
            {normalTarget.error ? (
              <Notice tone="warning">
                {normalTarget.error.kind === ErrorKind.bridgeUnavailable
                  ? "Standard-target inspection needs the AiDEX Setup app; a browser preview cannot read app folders."
                  : `Cannot inspect the standard target: ${normalTarget.error.message}`}
              </Notice>
            ) : null}
            {normalTarget.loading ? <Text tone="secondary">Checking the per-user install location…</Text> : null}
            {inspection && !inspection.eligible ? (
              <Notice tone="danger" role="alert">
                {inspection.ineligible_reason ?? "The standard per-user location cannot be used."}
              </Notice>
            ) : null}
            {inspection?.eligible && volume ? (
              <Text tone="secondary">
                {targetRoot} · {formatBytes(volume.free_bytes)} free on {volume.root}
              </Text>
            ) : null}
          </>
        ) : (
          <>
            {drives.error ? (
              <Notice tone="warning">
                {drives.error.kind === ErrorKind.bridgeUnavailable
                  ? "Drive discovery needs the AiDEX Setup app. This browser preview cannot read drives."
                  : `Cannot list drives: ${drives.error.message}`}
              </Notice>
            ) : null}
            {options.length ? (
              <ChoiceCardGroup
                label="Target drive"
                labelVisibility="hidden"
                variant="rows"
                options={options}
                value={state.driveRoot ?? ""}
                onValueChange={(root) => update({ driveRoot: root })}
              />
            ) : drives.loading ? (
              <Text tone="secondary">Looking for drives…</Text>
            ) : drives.error ? null : (
              <Text tone="secondary">No drives were found. Connect a USB drive or portable SSD, then rescan.</Text>
            )}
            {!drives.loading && !drives.error && options.length > 0 && eligible.length === 0 ? (
              <Notice tone="warning">
                None of these drives can hold AiDEX. Connect a USB drive or portable SSD; the system drive and internal disks are never
                used.
              </Notice>
            ) : null}
          </>
        )}
      </Stack>
      <TextField
        label="Installation location"
        leadingIcon={<FolderIcon />}
        value={targetRoot ?? (isNormal ? "Checking the per-user location…" : "Choose a drive")}
        readOnly
        size="md"
      />
      <Notice>
        {isNormal
          ? "Setup creates AiDEX folders and a drive manifest in your per-user local-data folder. Existing files are preserved."
          : "Setup creates AiDEX folders and an aidex-drive.json manifest at the removable drive root. Existing files stay untouched; nothing is formatted or deleted."}
      </Notice>
      {volume?.has_aidex_manifest ? (
        volume.schema_version === null ? (
          <Notice tone="danger" role="alert">
            {targetRoot} has an aidex-drive.json that cannot be read. Setup will not change it.
          </Notice>
        ) : (
          <Notice tone="success">
            {targetRoot} is already an AiDEX target (schema v{volume.schema_version}). Setup keeps its contents and only adds what is
            missing{volume.schema_version < 2 ? ", migrating the manifest with a backup" : ""}.
          </Notice>
        )
      ) : null}
      {volume && !fitsOnDrive(state, volume) ? (
        <Notice tone="danger" role="alert">
          This target has {formatBytes(volume.free_bytes)} free, but the selected setup needs about {formatGb(estimateGb(state))}.
          Choose another target or a smaller setup.
        </Notice>
      ) : null}
    </>
  );
}
