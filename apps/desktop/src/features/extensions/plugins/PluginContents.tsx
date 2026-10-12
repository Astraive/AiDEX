import { InlineCode, KeyValueList, List, ListRow, Markdown, Notice, SectionHeader, formatByteSize, formatDateTime } from "@aidex/ui/components";
import { ExternalLinkIcon } from "@aidex/ui/icons";
import { Badge, Button, Card, Stack, Text, type BadgeTone } from "@aidex/ui/primitives";
import type { PluginDetail, PluginIntegrity, PluginMcpServerEntry, PluginSummary } from "@aidex/client/dto/plugins";
import { useOpenLink } from "../data";

/** Trust state of a plugin in words and a badge tone. */
export function trustState(plugin: PluginSummary): { label: string; tone: BadgeTone } {
  const integrity: Record<Exclude<PluginIntegrity, "intact">, string> = {
    modified: "Files changed",
    missing: "Files missing",
    invalid: "Invalid",
  };
  if (plugin.integrity !== "intact") return { label: integrity[plugin.integrity], tone: "danger" };
  if (plugin.trusted) return { label: "Trusted", tone: "success" };
  return { label: "Not trusted", tone: "warning" };
}

function dateText(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : formatDateTime(date);
}

function commandLine(server: PluginMcpServerEntry): string {
  if (server.transport !== "stdio") return server.url ?? "";
  return [server.command, ...server.args].filter(Boolean).join(" ");
}

function listOrNone(values: readonly string[]) {
  if (values.length === 0) return "None";
  return (
    <Stack direction="row" gap={2} wrap>
      {values.map((value) => (
        <Badge key={value} tone="neutral" size="sm">
          {value}
        </Badge>
      ))}
    </Stack>
  );
}

/**
 * Everything a user reviews before trusting a plugin: manifest facts, the exact tree hash,
 * declared permissions, the skills and MCP servers (with their commands) it brings, and its README.
 * Plugin text is untrusted: it is only rendered, never run.
 */
export function PluginContents({ detail }: { detail: PluginDetail }) {
  const openLink = useOpenLink();
  const manifest = detail.manifest;
  const contents = detail.contents;
  const skipped = [...(contents?.unsupported_components ?? []), ...(contents?.ignored_fields ?? [])];

  return (
    <Stack gap={5}>
      {detail.error ? (
        <Notice tone="danger" variant="banner">
          {detail.error}
        </Notice>
      ) : null}
      <KeyValueList
        items={[
          { label: "Version", value: detail.version },
          ...(manifest?.author ? [{ label: "Author", value: manifest.author.name }] : []),
          ...(manifest?.license ? [{ label: "License", value: manifest.license }] : []),
          ...(manifest?.homepage
            ? [
                {
                  label: "Homepage",
                  value: (
                    <Button variant="ghost" icon={<ExternalLinkIcon />} onClick={() => openLink(manifest.homepage ?? "")}>
                      {manifest.homepage}
                    </Button>
                  ),
                },
              ]
            : []),
          { label: "Installed from", value: <InlineCode>{detail.source}</InlineCode> },
          { label: "Location", value: <InlineCode>{detail.location}</InlineCode> },
          { label: "Installed", value: dateText(detail.installed_at) },
          ...(contents
            ? [{ label: "Files", value: `${contents.file_count} files · ${formatByteSize(contents.total_bytes)}` }]
            : []),
          ...(detail.manifest_file ? [{ label: "Manifest", value: <InlineCode>{detail.manifest_file}</InlineCode> }] : []),
          {
            label: "Tree hash (SHA-256)",
            value: detail.tree_sha256 ? <InlineCode>{detail.tree_sha256}</InlineCode> : "Unavailable: the files can't be read",
          },
          ...(detail.trusted_sha256 && detail.trusted_sha256 !== detail.tree_sha256
            ? [{ label: "Previously trusted hash", value: <InlineCode>{detail.trusted_sha256}</InlineCode> }]
            : []),
        ]}
      />
      {manifest ? (
        <Stack gap={3}>
          <SectionHeader level={3} title="Declared permissions" />
          <KeyValueList
            items={[
              { label: "Network", value: listOrNone(manifest.permissions.network) },
              { label: "Vault secrets", value: listOrNone(manifest.permissions.secrets) },
              { label: "Shell commands", value: manifest.permissions.shell ? "Yes" : "No" },
            ]}
          />
          <Text tone="muted">
            The plugin declares these for your review. AiDEX still applies its approvals and policy to everything its skills and
            servers do.
          </Text>
        </Stack>
      ) : null}
      {contents ? (
        <>
          <Stack gap={3}>
            <SectionHeader level={3} title="Skills" meta={String(contents.skills.length)} />
            {contents.skills.length > 0 ? (
              <List>
                {contents.skills.map((skill) => (
                  <ListRow
                    key={skill.path}
                    title={skill.name ?? skill.dir}
                    subtitle={skill.description}
                    meta={[<InlineCode key="path">{skill.path}</InlineCode>]}
                  />
                ))}
              </List>
            ) : (
              <Text tone="muted">No skills.</Text>
            )}
            {contents.skills_truncated ? <Text tone="muted">More skills exist than are listed here.</Text> : null}
          </Stack>
          <Stack gap={3}>
            <SectionHeader level={3} title="MCP servers" meta={String(contents.mcp_servers.length)} />
            {contents.mcp_servers.length > 0 ? (
              <>
                <List>
                  {contents.mcp_servers.map((server) => (
                    <ListRow
                      key={server.name}
                      title={server.name}
                      subtitle={<InlineCode>{commandLine(server)}</InlineCode>}
                      meta={[
                        <Badge key="transport" tone="neutral" size="sm">
                          {server.transport}
                        </Badge>,
                        server.env_keys.length + server.header_keys.length > 0 ? (
                          <Text as="span" key="secrets" tone="muted">
                            Needs {[...server.env_keys, ...server.header_keys].join(", ")}
                          </Text>
                        ) : null,
                        server.bundled_command ? (
                          <Text as="span" key="bundled" tone="muted">
                            Runs bundled {server.bundled_command}
                          </Text>
                        ) : null,
                      ]}
                    />
                  ))}
                </List>
                <Text tone="muted">
                  Servers are added turned off. Turn each one on under MCP servers when you want agents to use it.
                </Text>
              </>
            ) : (
              <Text tone="muted">No MCP servers.</Text>
            )}
          </Stack>
          {skipped.length > 0 ? (
            <Notice tone="warning">AiDEX ignores these parts of the plugin: {skipped.join(", ")}.</Notice>
          ) : null}
          {contents.readme ? (
            <Stack gap={3}>
              <SectionHeader level={3} title="README" meta={contents.readme_truncated ? "Shortened" : undefined} />
              <Card padding="md">
                <Markdown source={contents.readme} headingLevelOffset={3} onOpenLink={openLink} />
              </Card>
            </Stack>
          ) : null}
        </>
      ) : null}
    </Stack>
  );
}
