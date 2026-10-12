import { KeyValueList, MediaItem, Meter, Notice, SectionHeader } from "@aidex/ui/components";
import { EjectIcon, HardDriveIcon, RefreshIcon, ShieldCheckIcon } from "@aidex/ui/icons";
import { Button, Card, IconBadge, Stack, StatusIndicator, Text } from "@aidex/ui/primitives";
import { removableLabels } from "@aidex/client/dto/drive";
import { formatBytes, UNKNOWN } from "@aidex/client/format";
import { useToast } from "../../app/providers/toast";
import { useDriveReport } from "../../hooks/useCoreReports";
import { CoreStatusNotice } from "../system/CoreStatusNotice";
import { driveSourceLabels } from "../system/labels";
import { ResidueCleanup } from "./ResidueCleanup";

/** The session's AiDEX drive: live identity, health, capacity, and migration advice; integrity and eject are planned. */
export function DrivePanel() {
  const notify = useToast();
  const drive = useDriveReport();
  const report = drive.data;
  const health = report?.health ?? null;
  const volume = report?.volume ?? null;

  const healthIndicator = !report ? (
    <StatusIndicator tone="neutral" label={drive.loading ? "Checking" : "Unknown"} />
  ) : !health ? (
    <StatusIndicator tone="warning" label="Problem" />
  ) : health.migration_required ? (
    <StatusIndicator tone="warning" label="Migration available" />
  ) : !health.writable ? (
    <StatusIndicator tone="warning" label="Read-only" />
  ) : (
    <StatusIndicator tone="success" label="Healthy" />
  );

  const used = volume?.total_bytes != null && volume.free_bytes != null ? volume.total_bytes - volume.free_bytes : null;

  return (
    <Stack gap={10}>
      <CoreStatusNotice requires="drive" />
      {report ? (
        <>
          <Card padding="md">
            <MediaItem
              media={<IconBadge icon={<HardDriveIcon />} tone="accent" />}
              title={volume?.label ?? report.root}
              titleLevel={2}
              description={[report.root, volume?.filesystem, volume ? removableLabels[volume.removable] : null]
                .filter(Boolean)
                .join(" · ")}
              trailing={
                <Stack direction="row" gap={4} align="center" wrap>
                  {healthIndicator}
                  <Button variant="ghost" size="md" icon={<RefreshIcon />} onClick={drive.refetch}>
                    Refresh
                  </Button>
                  <Button
                    variant="primary"
                    size="md"
                    icon={<EjectIcon />}
                    onClick={() =>
                      notify("Preview: safe eject (checkpoint sessions, stop AiDEX Core, clean up) arrives with the cleanup service.")
                    }
                  >
                    Safe eject
                  </Button>
                </Stack>
              }
            />
          </Card>
          {health?.migration_advice ? <Notice tone="warning">{health.migration_advice}</Notice> : null}
          {report.health_error ? (
            <Notice tone="danger" role="alert">
              {report.health_error.message}
            </Notice>
          ) : null}
          <section>
            <SectionHeader title="Storage" />
            {volume?.total_bytes != null && used !== null ? (
              <Meter
                label="Drive storage"
                value={used}
                max={volume.total_bytes}
                valueText={`${formatBytes(used)} of ${formatBytes(volume.total_bytes)} used · ${formatBytes(volume.free_bytes)} free`}
              />
            ) : (
              <Text tone="secondary">
                Capacity is unknown{report.volume_error ? `: ${report.volume_error.message}` : "."}
              </Text>
            )}
          </section>
          <section>
            <SectionHeader title="Details" />
            <KeyValueList
              items={[
                { label: "Root", value: report.root },
                { label: "Found", value: driveSourceLabels[report.source] },
                {
                  label: "Manifest schema",
                  value: health ? `v${health.schema_version ?? "?"} (current v${health.current_schema_version})` : UNKNOWN,
                },
                { label: "Writable", value: health ? (health.writable ? "Yes" : "No") : UNKNOWN },
                { label: "Free space", value: formatBytes(health?.free_bytes ?? volume?.free_bytes) },
                { label: "Attachment", value: volume ? removableLabels[volume.removable] : UNKNOWN },
                { label: "System volume", value: volume ? (volume.system ? "Yes" : "No") : UNKNOWN },
              ]}
            />
          </section>
        </>
      ) : null}
      <section>
        <SectionHeader
          title="Integrity"
          meta="Planned"
          actions={
            <Button
              variant="outline"
              size="md"
              icon={<ShieldCheckIcon />}
              onClick={() => notify("Preview: verifying signed integrity manifests arrives with the updater (issue #27).")}
            >
              Verify now
            </Button>
          }
        />
        <Text tone="secondary">
          Checksums and signed manifests for the AiDEX bundle are not part of this build yet, so no integrity result is shown.
        </Text>
      </section>
      <ResidueCleanup />
    </Stack>
  );
}
