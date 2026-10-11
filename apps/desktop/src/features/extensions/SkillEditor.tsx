import { useEffect, useState } from "react";
import { Notice } from "@aidex/ui/components";
import { Button, Card, Stack, Text, TextArea } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { useRpc } from "../../hooks/useRpc";
import { useExtensionAction } from "./useExtensionAction";

export function SkillEditor({
  id,
  onClose,
  onChanged,
}: {
  id: string;
  onClose: () => void;
  onChanged: () => void;
}) {
  const document = useRpc("extensions.skills.read", { id });
  const [content, setContent] = useState("");
  useEffect(() => {
    if (document.data) setContent(document.data.content);
  }, [document.data]);
  const action = useExtensionAction(() => {
    document.refetch();
    onChanged();
  });
  return (
    <Card padding="md">
      <Stack gap={3}>
        <Text>{document.data?.name ?? id} · SKILL.md</Text>
        {(document.error || action.error) && (
          <Notice tone="danger">
            {action.error ?? document.error?.message}
          </Notice>
        )}
        {action.message && <Notice tone="success">{action.message}</Notice>}
        {document.data?.plugin_id && (
          <Notice>
            Managed by plugin {document.data.plugin_id}. Replace the plugin
            bundle to update its skill files.
          </Notice>
        )}
        <TextArea
          label="Skill instructions"
          monospace
          rows={14}
          value={content}
          readOnly={!!document.data?.plugin_id}
          onChange={(event) => setContent(event.target.value)}
        />
        {document.data && (
          <Text tone="muted">
            Workspace grants: {document.data.workspace_ids.join(", ") || "none"}{" "}
            · Profiles: {document.data.profiles.join(", ") || "none"}
          </Text>
        )}
        <Stack direction="row" gap={2}>
          <Button
            disabled={
              action.busy || !document.data || !!document.data.plugin_id
            }
            onClick={() =>
              void action.act(
                () =>
                  call("extensions.skills.save", {
                    id,
                    content,
                    expected_digest: document.data!.digest,
                  }),
                "Skill saved.",
              )
            }
          >
            Save
          </Button>
          <Button variant="ghost" disabled={action.busy} onClick={onClose}>
            Close
          </Button>
        </Stack>
      </Stack>
    </Card>
  );
}
