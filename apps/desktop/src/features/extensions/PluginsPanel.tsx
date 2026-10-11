import { useState } from "react";
import { Notice } from "@aidex/ui/components";
import {
  Badge,
  Button,
  Card,
  Stack,
  Switch,
  Text,
  TextArea,
} from "@aidex/ui/primitives";
import type { PluginManifest } from "@aidex/client/dto/extensions";
import { call } from "@aidex/client/rpc";
import { useRpc } from "../../hooks/useRpc";
import { useExtensionAction } from "./useExtensionAction";
import { MarketplacePanel } from "./MarketplacePanel";

export function PluginsPanel({
  adding,
  onCloseInstall,
  onChanged,
}: {
  adding: boolean;
  onCloseInstall: () => void;
  onChanged: () => void;
}) {
  const plugins = useRpc("extensions.plugins.list");
  const action = useExtensionAction(() => {
    plugins.refetch();
    onChanged();
  });
  const [manifest, setManifest] = useState(
    JSON.stringify(
      {
        id: "new-plugin",
        name: "New plugin",
        version: "1.0.0",
        description: "",
        trusted: false,
        enabled: true,
        workspace_ids: [],
        profiles: [],
        skills: [],
        mcp_servers: [],
      },
      null,
      2,
    ),
  );
  const [files, setFiles] = useState("{}");
  return (
    <Stack gap={5}>
      <MarketplacePanel
        installedIds={plugins.data?.map((plugin) => plugin.id) ?? []}
        onChanged={() => {
          plugins.refetch();
          onChanged();
        }}
      />
      {(action.error || plugins.error) && (
        <Notice tone="danger">{action.error ?? plugins.error?.message}</Notice>
      )}
      {action.message && <Notice tone="success">{action.message}</Notice>}
      {adding && (
        <Card padding="md">
          <Stack gap={3}>
            <Text>
              Install a portable bundle of skill files and MCP server
              definitions. Review trust and workspace/profile grants in the
              manifest.
            </Text>
            <TextArea
              label="Plugin manifest JSON"
              monospace
              rows={14}
              value={manifest}
              onChange={(event) => setManifest(event.target.value)}
            />
            <TextArea
              label="Declared skill files JSON map"
              monospace
              rows={8}
              value={files}
              onChange={(event) => setFiles(event.target.value)}
              hint="Keys are relative declared paths ending in /SKILL.md; values are complete file contents."
            />
            <Stack direction="row" gap={2}>
              <Button
                disabled={action.busy}
                onClick={() =>
                  void action.act(async () => {
                    await call("extensions.plugins.install", {
                      manifest: JSON.parse(manifest) as PluginManifest,
                      files: JSON.parse(files) as Record<string, string>,
                    });
                    onCloseInstall();
                  }, "Plugin bundle installed.")
                }
              >
                Install reviewed bundle
              </Button>
              <Button
                variant="ghost"
                disabled={action.busy}
                onClick={onCloseInstall}
              >
                Cancel
              </Button>
            </Stack>
          </Stack>
        </Card>
      )}
      {plugins.loading && <Text tone="muted">Loading installed plugins…</Text>}
      {plugins.data?.map((plugin) => (
        <Card key={plugin.id} padding="md">
          <Stack gap={3}>
            <Text>
              {plugin.name} · v{plugin.version}
            </Text>
            <Text tone="muted">{plugin.description}</Text>
            <Stack direction="row" gap={2}>
              <Badge>{plugin.trusted ? "Trusted" : "Untrusted"}</Badge>
              <Badge>
                {plugin.skills.length} skills · {plugin.mcp_servers.length} MCP
                servers
              </Badge>
            </Stack>
            <Text tone="muted">
              Workspace grants: {plugin.workspace_ids.join(", ") || "none"} ·
              Profiles: {plugin.profiles.join(", ") || "none"}
            </Text>
            <Stack direction="row" gap={2} align="center">
              <Switch
                label={`Enable ${plugin.name}`}
                checked={plugin.enabled}
                disabled={action.busy}
                onCheckedChange={(enabled) =>
                  void action.act(() =>
                    call("extensions.plugins.enable", {
                      id: plugin.id,
                      enabled,
                    }),
                  )
                }
              />
              <Text>{plugin.enabled ? "Enabled" : "Disabled"}</Text>
              <Button
                variant="ghost"
                disabled={action.busy}
                onClick={() => {
                  if (
                    window.confirm(
                      `Remove plugin ${plugin.name} and its contributed skills and MCP definitions?`,
                    )
                  )
                    void action.act(() =>
                      call("extensions.plugins.remove", { id: plugin.id }),
                    );
                }}
              >
                Remove
              </Button>
            </Stack>
          </Stack>
        </Card>
      ))}
      {plugins.data && !plugins.data.length && (
        <Text tone="muted">No plugin bundles installed.</Text>
      )}
    </Stack>
  );
}
