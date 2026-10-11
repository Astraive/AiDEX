import { useState } from "react";
import { Notice, SectionHeader } from "@aidex/ui/components";
import { Badge, Button, Card, Stack, Switch, Text } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { useRpc } from "../../hooks/useRpc";
import { ScopeFields, splitGrants } from "./ScopeFields";
import { useExtensionAction } from "./useExtensionAction";

export function MarketplacePanel({
  installedIds,
  onChanged,
}: {
  installedIds: string[];
  onChanged: () => void;
}) {
  const catalog = useRpc("extensions.marketplace.list");
  const action = useExtensionAction(onChanged);
  const [selectedId, setSelectedId] = useState<string>();
  const [trusted, setTrusted] = useState(false);
  const [workspaceIds, setWorkspaceIds] = useState("");
  const [profiles, setProfiles] = useState("");
  return (
    <Stack gap={4}>
      <SectionHeader
        title="Bundled AiDEX marketplace"
        meta={
          catalog.data ? `Catalog ${catalog.data.catalog_version}` : undefined
        }
      />
      <Text tone="muted">
        Versioned first-party packages shipped with AiDEX. Review permissions
        and explicitly grant access before installation.
      </Text>
      {(catalog.error || action.error) && (
        <Notice tone="danger">{action.error ?? catalog.error?.message}</Notice>
      )}
      {action.message && <Notice tone="success">{action.message}</Notice>}
      {catalog.loading && <Text tone="muted">Loading bundled packages…</Text>}
      {catalog.data?.entries.map((entry) => (
        <Card key={entry.id} padding="md">
          <Stack gap={3}>
            <Text>
              {entry.name} · v{entry.version}
            </Text>
            <Text tone="muted">{entry.description}</Text>
            <Text>Skills: {entry.skill_names.join(", ")}</Text>
            <Stack direction="row" gap={2} wrap>
              {entry.permissions.map((permission) => (
                <Badge key={permission}>{permission}</Badge>
              ))}
            </Stack>
            {installedIds.includes(entry.id) ? (
              <Badge tone="success">Installed</Badge>
            ) : selectedId !== entry.id ? (
              <Button
                variant="outline"
                disabled={action.busy}
                onClick={() => {
                  setSelectedId(entry.id);
                  setTrusted(false);
                }}
              >
                Review grants and install
              </Button>
            ) : (
              <Stack gap={3}>
                <ScopeFields
                  workspaceIds={workspaceIds}
                  profiles={profiles}
                  onWorkspaces={setWorkspaceIds}
                  onProfiles={setProfiles}
                />
                <Stack direction="row" gap={2} align="center">
                  <Switch
                    label={`Trust ${entry.name}`}
                    checked={trusted}
                    onCheckedChange={setTrusted}
                  />
                  <Text>
                    I trust this package for the granted workspaces and
                    profiles.
                  </Text>
                </Stack>
                <details>
                  <summary>Package identity</summary>
                  <Text>
                    Version {entry.version} · Digest {entry.package_digest}
                  </Text>
                </details>
                <Stack direction="row" gap={2}>
                  <Button
                    disabled={
                      action.busy ||
                      !trusted ||
                      !splitGrants(workspaceIds).length ||
                      !splitGrants(profiles).length
                    }
                    onClick={() =>
                      void action.act(async () => {
                        await call("extensions.marketplace.install", {
                          id: entry.id,
                          version: entry.version,
                          package_digest: entry.package_digest,
                          trusted,
                          workspace_ids: splitGrants(workspaceIds),
                          profiles: splitGrants(profiles),
                        });
                        setSelectedId(undefined);
                        setTrusted(false);
                      }, `${entry.name} v${entry.version} installed.`)
                    }
                  >
                    Install reviewed version
                  </Button>
                  <Button
                    variant="ghost"
                    disabled={action.busy}
                    onClick={() => {
                      setSelectedId(undefined);
                      setTrusted(false);
                    }}
                  >
                    Cancel
                  </Button>
                </Stack>
              </Stack>
            )}
          </Stack>
        </Card>
      ))}
    </Stack>
  );
}
