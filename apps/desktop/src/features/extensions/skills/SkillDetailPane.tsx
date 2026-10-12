import { useState } from "react";
import { Dialog, EmptyState, InlineCode, KeyValueList, Markdown, Notice, SectionHeader, formatByteSize } from "@aidex/ui/components";
import { LayersIcon, PencilIcon, TrashIcon } from "@aidex/ui/icons";
import { Badge, Button, Card, Stack, Text } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import type { SkillDetail, SkillSummary } from "@aidex/client/dto/skills";
import { describeError, useAction, useExtensionsQuery, useOpenLink } from "../data";
import { frontMatterText, invocationText, scopeDescriptions, scopeLabels, startsDisabled } from "./skillScopes";

type Props = {
  skill: SkillSummary;
  workspaceId: string | undefined;
  /** Opens the editor for a drive skill with its current text. */
  onEdit: (detail: SkillDetail) => void;
  onDeleted: () => void;
};

/** The selected skill: who can invoke it, where it lives, its front matter, and its rendered instructions. */
export function SkillDetailPane({ skill, workspaceId, onEdit, onDeleted }: Props) {
  const detail = useExtensionsQuery("skill.show", { id: skill.id, ...(workspaceId ? { workspace_id: workspaceId } : {}) });
  const action = useAction();
  const openLink = useOpenLink();
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const headingId = `skill-detail-${skill.id}`;
  const shown = detail.data?.id === skill.id ? detail.data : undefined;
  const drive = skill.scope === "drive";
  // The shown text has credentials replaced by a marker; saving it would write the marker over them.
  const redacted = shown?.body_redacted === true;

  const remove = async () => {
    const deleted = await action.run("delete", () => call("skill.delete", { name: skill.name }));
    if (!deleted) return;
    setConfirmingDelete(false);
    onDeleted();
  };

  return (
    <section aria-labelledby={headingId}>
      <Stack gap={5}>
        <SectionHeader
          title={<span id={headingId}>{skill.display_name}</span>}
          meta={<Badge tone="neutral" size="sm">{scopeLabels[skill.scope]}</Badge>}
          actions={
            drive ? (
              <Stack direction="row" gap={2}>
                <Button
                  variant="outline"
                  icon={<PencilIcon />}
                  disabled={!shown || shown.body === undefined || redacted}
                  onClick={() => shown && !redacted && onEdit(shown)}
                >
                  Edit
                </Button>
                <Button variant="ghost" icon={<TrashIcon />} onClick={() => setConfirmingDelete(true)}>
                  Delete
                </Button>
              </Stack>
            ) : null
          }
        />
        <Text tone="secondary">{skill.description || "No description."}</Text>
        {skill.error ? (
          <Notice tone="danger" variant="banner">
            This skill can't be used: {skill.error}
          </Notice>
        ) : startsDisabled(skill.scope) && !skill.enabled ? (
          <Notice tone="info">
            Skills from a workspace stay off until you turn them on. Read the instructions below before you enable this one.
          </Notice>
        ) : null}
        {drive && redacted ? (
          <Notice tone="info">
            This skill contains text that looks like a credential, so AiDEX hides it here and can't edit the skill.
            Edit <InlineCode>{skill.location}</InlineCode> on the drive instead.
          </Notice>
        ) : null}
        <KeyValueList
          items={[
            { label: "Invoked by", value: invocationText(skill) },
            { label: "Source", value: skill.plugin ? `${scopeDescriptions.plugin} (${skill.plugin})` : scopeDescriptions[skill.scope] },
            { label: "Location", value: <InlineCode>{skill.location}</InlineCode> },
            { label: "Size", value: formatByteSize(skill.size_bytes) },
            ...(skill.version ? [{ label: "Version", value: skill.version }] : []),
          ]}
        />
        {detail.error ? (
          <Notice tone="danger" role="alert">
            {describeError(detail.error)}
          </Notice>
        ) : null}
        {shown && shown.front_matter.length > 0 ? (
          <Stack gap={3}>
            <SectionHeader level={3} title="Front matter" meta="SKILL.md" />
            <KeyValueList
              items={shown.front_matter.map((entry) => ({
                label: entry.key,
                value: frontMatterText(entry.value),
              }))}
            />
          </Stack>
        ) : null}
        <Stack gap={3}>
          <SectionHeader level={3} title="Instructions" />
          {shown?.body !== undefined ? (
            <Card padding="md">
              <Markdown source={shown.body} headingLevelOffset={3} onOpenLink={openLink} />
            </Card>
          ) : shown ? (
            <Text tone="muted">The instructions can't be shown while the skill has an error.</Text>
          ) : detail.loading ? (
            <Text tone="muted">Loading the instructions…</Text>
          ) : (
            <EmptyState icon={<LayersIcon />} title="No instructions to show" />
          )}
        </Stack>
      </Stack>
      <Dialog
        open={confirmingDelete}
        title={`Delete ${skill.display_name}?`}
        description={`AiDEX removes skills/${skill.name} from the drive. Conversations that used it keep their history.`}
        onClose={() => {
          setConfirmingDelete(false);
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
            <Button variant="ghost" onClick={() => setConfirmingDelete(false)}>
              Cancel
            </Button>
            <Button variant="primary" icon={<TrashIcon />} disabled={action.busy !== null} onClick={() => void remove()}>
              {action.busy ? "Deleting…" : "Delete skill"}
            </Button>
          </Stack>
        </Stack>
      </Dialog>
    </section>
  );
}

