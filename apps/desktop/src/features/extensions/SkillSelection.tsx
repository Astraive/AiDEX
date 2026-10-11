import { List, ListRow, Notice } from "@aidex/ui/components";
import { Button, Stack, Switch, Text } from "@aidex/ui/primitives";
import type { SkillSummary } from "@aidex/client/dto/extensions";
import { skillSelectionReason, type SkillScope } from "./skillEligibility";

export function SkillSelection({
  skills,
  selectedIds,
  onChange,
  scope,
  disabled,
  loading,
  error,
}: {
  skills: readonly SkillSummary[];
  selectedIds: string[];
  onChange: (ids: string[]) => void;
  scope?: SkillScope;
  disabled?: boolean;
  loading?: boolean;
  error?: string;
}) {
  const missing = selectedIds.filter(
    (id) => !skills.some((skill) => skill.id === id),
  );
  return (
    <Stack gap={3}>
      {error && <Notice tone="danger">{error}</Notice>}
      {loading && <Text tone="muted">Loading installed skills…</Text>}
      {!scope && (
        <Text tone="muted">
          Defaults are checked against each task's workspace and permission
          grants when used.
        </Text>
      )}
      {scope && !scope.workspaceId && (
        <Notice>
          Choose a project to attach skills. Ordinary projectless chat does not
          receive agent skills or tools.
        </Notice>
      )}
      <List>
        {skills.map((skill) => {
          const reason = skillSelectionReason(skill, scope);
          const selected = selectedIds.includes(skill.id);
          return (
            <ListRow
              key={skill.id}
              title={skill.name}
              subtitle={skill.description}
              meta={[
                reason ??
                  (skill.model_invocable
                    ? "User-selectable"
                    : "Manual-only skill"),
              ]}
              trailing={
                <Switch
                  label={`Use skill ${skill.name}`}
                  checked={selected}
                  disabled={
                    disabled ||
                    (!selected && (!!reason || selectedIds.length >= 32))
                  }
                  onCheckedChange={(enabled) =>
                    onChange(
                      enabled
                        ? [...selectedIds, skill.id]
                        : selectedIds.filter((id) => id !== skill.id),
                    )
                  }
                />
              }
            />
          );
        })}
      </List>
      {!loading && !skills.length && !error && (
        <Text tone="muted">
          No installed skills. Import or install a skill in Extensions.
        </Text>
      )}
      {missing.map((id) => (
        <Stack key={id} direction="row" gap={2}>
          <Text>Unavailable selected skill: {id}</Text>
          <Button
            variant="ghost"
            disabled={disabled}
            onClick={() =>
              onChange(selectedIds.filter((candidate) => candidate !== id))
            }
          >
            Remove selection
          </Button>
        </Stack>
      ))}
      <Text tone="muted">
        {selectedIds.length} of 32 selected. Disabling or changing grants later
        blocks use; it does not silently replace the selected skill.
      </Text>
    </Stack>
  );
}
