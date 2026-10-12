import { useState, type FormEvent } from "react";
import { Dialog, InlineCode, Markdown, Notice } from "@aidex/ui/components";
import { Button, Card, Grid, Stack, Text, TextArea, TextField } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import type { SkillDetail, SkillSummary } from "@aidex/client/dto/skills";
import { useAction } from "../data";
import { MAX_BODY_BYTES, MAX_DESCRIPTION_CHARS, SKILL_NAME_PATTERN } from "./skillScopes";

type Props = {
  open: boolean;
  /** The drive skill being edited; omit to write a new one. */
  initial?: SkillDetail;
  /** Names of the drive skills that already exist, to warn before one is replaced. */
  driveNames: ReadonlySet<string>;
  onClose: () => void;
  onSaved: (skill: SkillSummary) => void;
};

type Errors = { name?: string; description?: string; version?: string; body?: string };

const VERSION_PATTERN = /^[A-Za-z0-9.+_-]{1,64}$/;
/** The front matter keys the editor writes; `skill.save` keeps the rest of a replaced skill's keys. */
const DRAFT_KEYS: ReadonlySet<string> = new Set(["name", "description", "version"]);
const BODY_ROWS = 18;

const STARTER_BODY = "# What this skill does\n\nDescribe when to use it, then the steps to follow.\n\n1. First step\n2. Second step\n";

function validate(name: string, description: string, version: string, body: string): Errors {
  const errors: Errors = {};
  if (!SKILL_NAME_PATTERN.test(name)) errors.name = "Use 1-64 lowercase letters, digits, and hyphens, starting with a letter or digit.";
  if (!description.trim()) errors.description = "Say in one line when the skill should be used.";
  else if (description.trim().length > MAX_DESCRIPTION_CHARS) errors.description = `Keep it under ${MAX_DESCRIPTION_CHARS} characters.`;
  if (version.trim() && !VERSION_PATTERN.test(version.trim()))
    errors.version = "Use letters, digits, `.`, `-`, `+`, or `_` (at most 64).";
  if (!body.trim()) errors.body = "Write the instructions the agent follows.";
  else if (new TextEncoder().encode(body).length > MAX_BODY_BYTES) errors.body = "The instructions must stay under 64 KB.";
  return errors;
}

/**
 * Writes or rewrites a drive skill (`skills/<name>/SKILL.md`) with `skill.save`: name, one-line
 * description, optional version, and Markdown instructions beside a live preview. Give it a new `key`
 * for each opening so the draft starts from `initial`.
 */
export function SkillEditor({ open, initial, driveNames, onClose, onSaved }: Props) {
  const editing = initial !== undefined;
  const [name, setName] = useState(initial?.name ?? "");
  const [description, setDescription] = useState(initial?.description ?? "");
  const [version, setVersion] = useState(initial?.version ?? "");
  const [body, setBody] = useState(initial?.body ?? STARTER_BODY);
  const [errors, setErrors] = useState<Errors>({});
  const action = useAction();
  const replaces = !editing && driveNames.has(name.trim());
  // Credential-like text was replaced by a marker in `initial`; saving would write the marker over it.
  const redacted = initial?.body_redacted === true;
  const keptKeys = initial?.front_matter.map((entry) => entry.key).filter((key) => !DRAFT_KEYS.has(key)) ?? [];

  const save = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (redacted) return;
    const nextErrors = validate(name.trim(), description, version, body);
    setErrors(nextErrors);
    if (Object.keys(nextErrors).length > 0) return;
    const saved = await action.run("save", () =>
      call("skill.save", {
        name: name.trim(),
        description: description.trim(),
        body,
        version: version.trim() || null,
      }),
    );
    if (saved) onSaved(saved);
  };

  const clear = (field: keyof Errors) => setErrors((current) => ({ ...current, [field]: undefined }));

  return (
    <Dialog
      open={open}
      size="xl"
      title={editing ? `Edit ${initial.display_name}` : "New skill"}
      description="Skills are Markdown instructions an agent follows when you invoke them with $name. AiDEX saves this one on the drive, so it travels with you."
      onClose={onClose}
    >
      <form onSubmit={(event) => void save(event)} noValidate aria-label={editing ? "Edit skill" : "New skill"}>
        <Stack gap={5}>
          <Grid columns={2} gap={4}>
            <TextField
              label="Name"
              size="md"
              placeholder="review-pr"
              autoComplete="off"
              spellCheck={false}
              readOnly={editing}
              value={name}
              error={errors.name}
              hint={editing ? "The name is the skill's folder, so it stays the same." : "Lowercase letters, digits, and hyphens."}
              onChange={(event) => {
                setName(event.target.value);
                clear("name");
              }}
            />
            <TextField
              label="Version (optional)"
              size="md"
              placeholder="1.0.0"
              autoComplete="off"
              spellCheck={false}
              value={version}
              error={errors.version}
              onChange={(event) => {
                setVersion(event.target.value);
                clear("version");
              }}
            />
          </Grid>
          <TextField
            label="Description"
            size="md"
            placeholder="Review a pull request for bugs, risky changes, and missing tests"
            value={description}
            error={errors.description}
            hint="One line. The model also reads it to decide when the skill fits a request."
            onChange={(event) => {
              setDescription(event.target.value);
              clear("description");
            }}
          />
          <Grid columns={2} gap={4}>
            <TextArea
              label="Instructions (Markdown)"
              monospace
              rows={BODY_ROWS}
              spellCheck={false}
              value={body}
              error={errors.body}
              onChange={(event) => {
                setBody(event.target.value);
                clear("body");
              }}
            />
            <Stack gap={2}>
              <Text as="span" variant="label" tone="secondary">
                Preview
              </Text>
              <Card padding="md">
                {body.trim() ? (
                  <Markdown source={body} headingLevelOffset={2} />
                ) : (
                  <Text tone="muted">The rendered instructions appear here.</Text>
                )}
              </Card>
            </Stack>
          </Grid>
          {keptKeys.length > 0 ? (
            <Text tone="muted">Saving keeps the skill's other SKILL.md settings: {keptKeys.join(", ")}.</Text>
          ) : null}
          {redacted ? (
            <Notice tone="warning">
              This skill contains text that looks like a credential, which AiDEX hides, so saving here would replace
              it. Edit <InlineCode>{initial?.location}</InlineCode> on the drive instead.
            </Notice>
          ) : null}
          {replaces ? (
            <Notice tone="warning">A drive skill named {name.trim()} already exists. Saving replaces its SKILL.md.</Notice>
          ) : null}
          {action.error ? (
            <Notice tone="danger" role="alert">
              {action.error}
            </Notice>
          ) : null}
          <Stack direction="row" gap={2} justify="end">
            <Button variant="ghost" onClick={onClose}>
              Cancel
            </Button>
            <Button type="submit" variant="primary" disabled={action.busy !== null || redacted}>
              {action.busy ? "Saving…" : editing ? "Save changes" : "Create skill"}
            </Button>
          </Stack>
        </Stack>
      </form>
    </Dialog>
  );
}
