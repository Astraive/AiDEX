import { useState } from "react";
import { Dialog, EmptyState, InlineCode, List, ListRow, MediaItem, Notice, SectionHeader } from "@aidex/ui/components";
import { PlusIcon, PuzzleIcon, ShieldCheckIcon, TrashIcon } from "@aidex/ui/icons";
import { Badge, Button, IconBadge, Split, Stack, Switch, Text } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import type { PluginSummary } from "@aidex/client/dto/plugins";
import { describeError, useAction, useExtensionsQuery } from "../data";
import { PluginContents, trustState } from "./PluginContents";
import { PluginInstallDialog, PluginReviewDialog } from "./PluginDialogs";

type Props = {
  /** The plugin shown in the detail pane (route `id`); defaults to the first. */
  selectedId?: string;
  onSelect: (name: string | undefined) => void;
};

type DialogState = { session: number; open: boolean };

function countsText(plugin: PluginSummary): string {
  const skills = plugin.skill_count === 1 ? "1 skill" : `${plugin.skill_count} skills`;
  const servers = plugin.mcp_server_count === 1 ? "1 server" : `${plugin.mcp_server_count} servers`;
  return `${skills} · ${servers}`;
}

/** Plugins tab: installed plugins with trust state and enable switches, the selected plugin's contents, install and review. */
export function PluginsPanel({ selectedId, onSelect }: Props) {
  const listing = useExtensionsQuery("plugin.list");
  const toggles = useAction();
  const [install, setInstall] = useState<DialogState>({ session: 0, open: false });
  const [review, setReview] = useState<{ name: string; open: boolean } | null>(null);

  const plugins = listing.data?.plugins ?? [];
  const selected = plugins.find((plugin) => plugin.name === selectedId) ?? plugins[0];

  const toggle = (plugin: PluginSummary, enabled: boolean) => {
    // Turning on a plugin whose exact files are not trusted goes through the review.
    if (enabled && !plugin.trusted) {
      setReview({ name: plugin.name, open: true });
      return;
    }
    void toggles.run(enabled ? "enable" : "disable", () => call("plugin.set_enabled", { name: plugin.name, enabled }));
  };

  const list =
    plugins.length === 0 ? (
      listing.loading || listing.error ? (
        listing.error ? null : <Text tone="muted">Loading plugins…</Text>
      ) : (
        <EmptyState
          icon={<PuzzleIcon />}
          title="No plugins installed"
          description="A plugin is a folder of skills and MCP servers, in AiDEX or Claude Code plugin format. Install one from a folder or .zip."
          actions={
            <Button variant="primary" icon={<PlusIcon />} onClick={() => setInstall((current) => ({ session: current.session + 1, open: true }))}>
              Install from folder
            </Button>
          }
        />
      )
    ) : (
      <section aria-labelledby="plugins-heading">
        <SectionHeader
          title={<span id="plugins-heading">Installed</span>}
          meta={`${plugins.filter((plugin) => plugin.enabled).length} of ${plugins.length} enabled`}
        />
        <List>
          {plugins.map((plugin) => {
            const trust = trustState(plugin);
            return (
              <ListRow
                key={plugin.name}
                media={<IconBadge size="md" icon={<PuzzleIcon />} />}
                title={plugin.name}
                subtitle={plugin.error ? `Can't be used: ${plugin.error}` : (plugin.description ?? countsText(plugin))}
                meta={[
                  <Text as="span" key="version" tone="muted">
                    v{plugin.version}
                  </Text>,
                  <Badge key="trust" tone={trust.tone} size="sm">
                    {trust.label}
                  </Badge>,
                ]}
                selected={plugin.name === selected?.name}
                onSelect={() => onSelect(plugin.name)}
                trailing={
                  <Switch
                    label={`Enable ${plugin.name}`}
                    checked={plugin.enabled}
                    disabled={toggles.busy !== null || (!plugin.enabled && !plugin.tree_sha256)}
                    onCheckedChange={(enabled) => toggle(plugin, enabled)}
                  />
                }
              />
            );
          })}
        </List>
      </section>
    );

  return (
    <Stack gap={5}>
      <Stack direction="row" gap={3} align="center" justify="between" wrap>
        <Text tone="secondary">Plugins install turned off. Turning one on trusts its exact files; a change turns it off again.</Text>
        <Button variant="primary" icon={<PlusIcon />} onClick={() => setInstall((current) => ({ session: current.session + 1, open: true }))}>
          Install from folder
        </Button>
      </Stack>
      {listing.error ? (
        <Notice tone="danger" role="alert">
          {describeError(listing.error)}
        </Notice>
      ) : null}
      {toggles.error ? (
        <Notice tone="danger" role="alert">
          {toggles.error}
        </Notice>
      ) : null}
      {selected ? (
        <Split divided emphasis="equal">
          {list}
          <PluginDetailPane
            key={selected.name}
            plugin={selected}
            onReview={() => setReview({ name: selected.name, open: true })}
            onUninstalled={() => onSelect(undefined)}
          />
        </Split>
      ) : (
        list
      )}
      {listing.data && listing.data.unregistered.length > 0 ? (
        <Notice tone="warning">
          These folders under plugins/ were not installed through AiDEX and are ignored:{" "}
          {listing.data.unregistered.map((name, index) => (
            <span key={name}>
              {index > 0 ? ", " : null}
              <InlineCode>{name}</InlineCode>
            </span>
          ))}
        </Notice>
      ) : null}
      {install.session > 0 ? (
        <PluginInstallDialog
          key={install.session}
          open={install.open}
          onClose={() => setInstall((current) => ({ ...current, open: false }))}
          onInstalled={(name) => onSelect(name)}
        />
      ) : null}
      {review ? (
        <PluginReviewDialog
          key={review.name}
          open={review.open}
          name={review.name}
          onClose={() => setReview({ ...review, open: false })}
          onTrusted={(plugin) => {
            setReview({ ...review, open: false });
            onSelect(plugin.name);
          }}
        />
      ) : null}
    </Stack>
  );
}

type PaneProps = {
  plugin: PluginSummary;
  onReview: () => void;
  onUninstalled: () => void;
};

/** The selected plugin: trust state, review, uninstall, and its full contents. */
function PluginDetailPane({ plugin, onReview, onUninstalled }: PaneProps) {
  const detail = useExtensionsQuery("plugin.show", { name: plugin.name });
  const action = useAction();
  const [confirming, setConfirming] = useState(false);
  const trust = trustState(plugin);
  const shown = detail.data?.name === plugin.name ? detail.data : undefined;

  const uninstall = async () => {
    const removed = await action.run("uninstall", () => call("plugin.uninstall", { name: plugin.name }));
    if (!removed) return;
    setConfirming(false);
    onUninstalled();
  };

  return (
    <section aria-label={plugin.name}>
      <Stack gap={5}>
        <MediaItem
          titleLevel={2}
          media={<IconBadge size="lg" icon={<PuzzleIcon />} />}
          title={plugin.name}
          description={plugin.description ?? countsText(plugin)}
          trailing={<Badge tone={trust.tone}>{trust.label}</Badge>}
        />
        <Stack direction="row" gap={2} wrap>
          {!plugin.trusted && plugin.tree_sha256 ? (
            <Button variant="primary" icon={<ShieldCheckIcon />} onClick={onReview}>
              Review and trust
            </Button>
          ) : null}
          <Button variant="ghost" icon={<TrashIcon />} onClick={() => setConfirming(true)}>
            Uninstall
          </Button>
        </Stack>
        {plugin.integrity === "modified" ? (
          <Notice tone="warning">
            The plugin&apos;s files changed after you trusted them, so AiDEX turned it off. Review the new files to turn it on again.
          </Notice>
        ) : null}
        {detail.error ? (
          <Notice tone="danger" role="alert">
            {describeError(detail.error)}
          </Notice>
        ) : null}
        {shown ? <PluginContents detail={shown} /> : detail.loading ? <Text tone="muted">Reading the plugin…</Text> : null}
      </Stack>
      <Dialog
        open={confirming}
        title={`Uninstall ${plugin.name}?`}
        description="AiDEX deletes the plugin's files from the drive and removes the skills and MCP servers it brought."
        onClose={() => {
          setConfirming(false);
          action.clearError();
        }}
      >
        <Stack gap={4}>
          {action.error ? (
            <Notice tone="danger" role="alert">
              {action.error}
            </Notice>
          ) : null}
          <Stack direction="row" gap={2} justify="end">
            <Button variant="ghost" onClick={() => setConfirming(false)}>
              Cancel
            </Button>
            <Button variant="primary" icon={<TrashIcon />} disabled={action.busy !== null} onClick={() => void uninstall()}>
              {action.busy ? "Uninstalling…" : "Uninstall"}
            </Button>
          </Stack>
        </Stack>
      </Dialog>
    </section>
  );
}
