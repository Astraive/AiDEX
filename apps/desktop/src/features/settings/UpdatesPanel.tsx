import { useState } from "react";
import { KeyValueList, SectionHeader, SettingRow } from "@aidex/ui/components";
import { RefreshIcon } from "@aidex/ui/icons";
import { Button, Select, Stack, Text } from "@aidex/ui/primitives";
import { useToast } from "../../app/providers/toast";
import { sampleVersion, updateChannels, type UpdateChannel } from "./sample";

const channelLabel = (channel: UpdateChannel) => updateChannels.find((option) => option.value === channel)?.label ?? channel;

/** Installed version, release channel, and how updates are applied to the drive. */
export function UpdatesPanel() {
  const notify = useToast();
  const [channel, setChannel] = useState<UpdateChannel>("stable");

  return (
    <Stack gap={10}>
      <section>
        <SectionHeader
          title="Version"
          actions={
            <Button
              variant="outline"
              size="md"
              icon={<RefreshIcon />}
              onClick={() => notify("Preview: checking for updates will be available when AiDEX Core is connected.")}
            >
              Check for updates
            </Button>
          }
        />
        <KeyValueList
          items={[
            { label: "Version", value: sampleVersion.version },
            { label: "Channel", value: channelLabel(channel) },
            { label: "Last checked", value: sampleVersion.lastChecked },
          ]}
        />
      </section>
      <section>
        <SectionHeader title="Release channel" />
        <SettingRow
          title="Update channel"
          description="Beta gets new features first and may be less stable."
          controlId="setting-channel"
          control={
            <Select
              id="setting-channel"
              label="Update channel"
              labelVisibility="hidden"
              options={updateChannels}
              value={channel}
              onChange={(event) => setChannel(event.target.value as UpdateChannel)}
            />
          }
        />
        <Text tone="muted">
          Updates are staged on the drive, verified against their signature, and switched in atomically. If a new version
          fails to start, AiDEX rolls back to the previous one.
        </Text>
      </section>
    </Stack>
  );
}
