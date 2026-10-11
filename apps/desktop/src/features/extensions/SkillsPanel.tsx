import { useState } from "react";
import { List, ListRow, Notice } from "@aidex/ui/components";
import {
  Badge,
  Button,
  Card,
  SearchField,
  Stack,
  Switch,
  Text,
  TextArea,
} from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { useRpc } from "../../hooks/useRpc";
import { ScopeFields, splitGrants } from "./ScopeFields";
import { SkillEditor } from "./SkillEditor";
import { useExtensionAction } from "./useExtensionAction";

export function SkillsPanel({
  initialEditingId,
  adding,
  onCloseImport,
  onChanged,
}: {
  initialEditingId?: string;
  adding: boolean;
  onCloseImport: () => void;
  onChanged: () => void;
}) {
  const skills = useRpc("extensions.skills.list");
  const [query, setQuery] = useState("");
  const [editingId, setEditingId] = useState(initialEditingId);
  const [content, setContent] = useState(
    "---\nname: new-skill\ndescription: Describe when this skill should be used.\n---\n\nWrite the skill instructions here.\n",
  );
  const [trusted, setTrusted] = useState(false);
  const [workspaceIds, setWorkspaceIds] = useState("");
  const [profiles, setProfiles] = useState("");
  const refresh = () => {
    skills.refetch();
    onChanged();
  };
  const action = useExtensionAction(refresh);
  const visible =
    skills.data?.filter((skill) =>
      `${skill.name} ${skill.description}`
        .toLowerCase()
        .includes(query.toLowerCase()),
    ) ?? [];
  return (
    <Stack gap={5}>
      {(skills.error || action.error) && (
        <Notice tone="danger">{action.error ?? skills.error?.message}</Notice>
      )}
      {action.message && <Notice tone="success">{action.message}</Notice>}
      {adding && (
        <Card padding="md">
          <Stack gap={3}>
            <TextArea
              label="SKILL.md content"
              rows={10}
              monospace
              value={content}
              onChange={(event) => setContent(event.target.value)}
            />
            <ScopeFields
              workspaceIds={workspaceIds}
              profiles={profiles}
              onWorkspaces={setWorkspaceIds}
              onProfiles={setProfiles}
            />
            <Stack direction="row" gap={2} align="center">
              <Switch
                label="Trust this skill"
                checked={trusted}
                onCheckedChange={setTrusted}
              />
              <Text>I trust these instructions for the granted scopes.</Text>
            </Stack>
            <Stack direction="row" gap={2}>
              <Button
                disabled={action.busy || !content.trim()}
                onClick={() =>
                  void action.act(async () => {
                    await call("extensions.skills.import", {
                      content,
                      trusted,
                      workspace_ids: splitGrants(workspaceIds),
                      profiles: splitGrants(profiles),
                    });
                    onCloseImport();
                  }, "Skill imported.")
                }
              >
                Import skill
              </Button>
              <Button
                variant="ghost"
                disabled={action.busy}
                onClick={onCloseImport}
              >
                Cancel
              </Button>
            </Stack>
          </Stack>
        </Card>
      )}
      <SearchField
        label="Search installed skills"
        value={query}
        onChange={(event) => setQuery(event.target.value)}
      />
      {skills.loading && <Text tone="muted">Loading installed skills…</Text>}
      <List>
        {visible.map((skill) => (
          <ListRow
            key={skill.id}
            title={skill.name}
            subtitle={skill.description}
            selected={editingId === skill.id}
            meta={[
              <Badge key="trust">
                {skill.trusted ? "Trusted" : "Untrusted"}
              </Badge>,
              skill.plugin_id ? `Plugin ${skill.plugin_id}` : "Standalone",
              `${skill.workspace_ids.length} workspace grants`,
            ]}
            trailing={
              <Stack direction="row" gap={2} align="center">
                <Switch
                  label={`Enable ${skill.name}`}
                  checked={skill.enabled}
                  disabled={action.busy}
                  onCheckedChange={(enabled) =>
                    void action.act(() =>
                      call("extensions.skills.enable", {
                        id: skill.id,
                        enabled,
                      }),
                    )
                  }
                />
                <Button variant="ghost" onClick={() => setEditingId(skill.id)}>
                  Read and edit
                </Button>
                {!skill.plugin_id && (
                  <Button
                    variant="ghost"
                    disabled={action.busy}
                    onClick={() => {
                      if (
                        window.confirm(
                          `Remove skill ${skill.name} from the drive?`,
                        )
                      )
                        void action.act(async () => {
                          await call("extensions.skills.remove", {
                            id: skill.id,
                          });
                          if (editingId === skill.id) setEditingId(undefined);
                        });
                    }}
                  >
                    Remove
                  </Button>
                )}
              </Stack>
            }
          />
        ))}
      </List>
      {skills.data && !visible.length && (
        <Text tone="muted">No installed skills match.</Text>
      )}
      {editingId && (
        <SkillEditor
          key={editingId}
          id={editingId}
          onClose={() => setEditingId(undefined)}
          onChanged={refresh}
        />
      )}
    </Stack>
  );
}
