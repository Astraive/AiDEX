import { useMemo, useState } from "react";
import { EmptyState, InlineCode, List, ListRow, Notice, SectionHeader } from "@aidex/ui/components";
import { LayersIcon, PlusIcon, SearchIcon } from "@aidex/ui/icons";
import { Badge, Button, IconBadge, SearchField, SegmentedControl, Split, Stack, Switch, Text } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import type { SkillDetail, SkillSummary } from "@aidex/client/dto/skills";
import { describeError, useAction, useExtensionsQuery, useWorkspaceScope } from "../data";
import { WorkspaceSelect } from "../WorkspaceSelect";
import { SkillDetailPane } from "./SkillDetailPane";
import { SkillEditor } from "./SkillEditor";
import { filterSkills, scopeFilterOptions, scopeLabels, type ScopeFilter } from "./skillScopes";

type Props = {
  /** The skill shown in the detail pane (route `id`); defaults to the first listed. */
  selectedId?: string;
  onSelect: (id: string | undefined) => void;
};

type EditorState = { session: number; open: boolean; initial?: SkillDetail };

/** Skills tab: search and scope filter, the skill list with enable switches, the detail pane, and the editor. */
export function SkillsPanel({ selectedId, onSelect }: Props) {
  const scope = useWorkspaceScope();
  const workspaceId = scope.workspaceId;
  const listing = useExtensionsQuery("skill.list", workspaceId ? { workspace_id: workspaceId } : undefined);
  const toggles = useAction();
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<ScopeFilter>("all");
  const [editor, setEditor] = useState<EditorState>({ session: 0, open: false });

  const skills = useMemo(() => listing.data?.skills ?? [], [listing.data]);
  const visible = useMemo(() => filterSkills(skills, filter, query), [skills, filter, query]);
  const selected = visible.find((skill) => skill.id === selectedId) ?? visible[0];
  const driveNames = useMemo(
    () => new Set(skills.filter((skill) => skill.scope === "drive").map((skill) => skill.name)),
    [skills],
  );
  const enabledCount = skills.filter((skill) => skill.enabled && !skill.error).length;

  const openEditor = (initial?: SkillDetail) =>
    setEditor((current) => ({ session: current.session + 1, open: true, initial }));
  const closeEditor = () => setEditor((current) => ({ ...current, open: false }));

  const toggle = (skill: SkillSummary, enabled: boolean) =>
    void toggles.run(enabled ? "enable" : "disable", () =>
      call("skill.set_enabled", { id: skill.id, enabled, ...(workspaceId ? { workspace_id: workspaceId } : {}) }),
    );

  const list =
    visible.length === 0 ? (
      skills.length === 0 ? (
        listing.loading || listing.error ? (
          listing.error ? null : <Text tone="muted">Loading skills…</Text>
        ) : (
          <EmptyState
            icon={<LayersIcon />}
            title="No skills yet"
            description="Write a skill for your drive, add SKILL.md folders to a workspace's .aidex/skills, or install a plugin that brings some."
            actions={
              <Button variant="primary" icon={<PlusIcon />} onClick={() => openEditor()}>
                New skill
              </Button>
            }
          />
        )
      ) : (
        <EmptyState icon={<SearchIcon />} title="No skills match" description="Try another search or scope." />
      )
    ) : (
      <section aria-labelledby="skills-list-heading">
        <SectionHeader
          title={<span id="skills-list-heading">Skills</span>}
          meta={`${enabledCount} of ${skills.length} enabled`}
        />
        <List>
          {visible.map((skill) => (
            <ListRow
              key={skill.id}
              media={<IconBadge size="md" icon={<LayersIcon />} />}
              title={skill.display_name}
              subtitle={skill.error ? `Can't be used: ${skill.error}` : skill.description}
              meta={[
                <Badge key="scope" tone="neutral" size="sm">
                  {skill.plugin ? `${scopeLabels.plugin} · ${skill.plugin}` : scopeLabels[skill.scope]}
                </Badge>,
                skill.error ? (
                  <Badge key="state" tone="danger" size="sm">
                    Error
                  </Badge>
                ) : (
                  <InlineCode key="state">${skill.name}</InlineCode>
                ),
              ]}
              selected={skill.id === selected?.id}
              onSelect={() => onSelect(skill.id)}
              trailing={
                <Switch
                  label={`Enable ${skill.display_name}`}
                  checked={skill.enabled && !skill.error}
                  disabled={Boolean(skill.error) || toggles.busy !== null}
                  onCheckedChange={(enabled) => toggle(skill, enabled)}
                />
              }
            />
          ))}
        </List>
      </section>
    );

  const issues = listing.data?.issues ?? [];

  return (
    <Stack gap={5}>
      <Stack gap={3}>
        <Stack direction="row" gap={3} align="center">
          <SearchField
            label="Search skills"
            placeholder="Search skills…"
            size="md"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
          <Button variant="primary" icon={<PlusIcon />} onClick={() => openEditor()}>
            New skill
          </Button>
        </Stack>
        <Stack direction="row" gap={3} align="center">
          <SegmentedControl size="sm" label="Skill scope" options={scopeFilterOptions} value={filter} onValueChange={setFilter} />
          <WorkspaceSelect
            label="Workspace for project and repo skills"
            labelVisibility="hidden"
            workspaces={scope.workspaces}
            value={workspaceId}
            noneLabel="No workspace: drive and plugin skills only"
            optionPrefix="Workspace: "
            onChange={scope.choose}
          />
        </Stack>
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
          <SkillDetailPane
            key={selected.id}
            skill={selected}
            workspaceId={workspaceId}
            onEdit={openEditor}
            onDeleted={() => onSelect(undefined)}
          />
        </Split>
      ) : (
        list
      )}
      {issues.length > 0 ? (
        <Notice tone="warning">
          <Stack gap={2}>
            <Text as="span">
              {issues.length === 1 ? "One skill folder was skipped:" : `${issues.length} skill folders were skipped:`}
            </Text>
            {issues.map((issue) => (
              <Text as="span" key={`${issue.scope}:${issue.location}`} tone="secondary">
                <InlineCode>{issue.location}</InlineCode> ({scopeLabels[issue.scope]}): {issue.error}
              </Text>
            ))}
          </Stack>
        </Notice>
      ) : null}
      {listing.data?.truncated ? <Text tone="muted">Some skills were left out of this list to keep it small.</Text> : null}
      {editor.session > 0 ? (
        <SkillEditor
          key={editor.session}
          open={editor.open}
          initial={editor.initial}
          driveNames={driveNames}
          onClose={closeEditor}
          onSaved={(saved) => {
            closeEditor();
            setFilter("all");
            setQuery("");
            onSelect(saved.id);
          }}
        />
      ) : null}
    </Stack>
  );
}
