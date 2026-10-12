// Mirrors the `skill.*` methods in crates/aidexd/src/methods/skills.rs and the
// `aidex_skills` wire types (crates/skills/src/catalog.rs, discover.rs, authoring.rs).
// Skill text is credential-redacted by the daemon but still untrusted: render it,
// never execute or inject it.

/** Where a skill was found, in precedence order (the first skill with a name wins). */
export type SkillScope = "project" | "repo" | "drive" | "plugin";

/** `<scope>:<name>`, for example `drive:review`. */
export type SkillId = string;

/** One skill as `skill.list` reports it. */
export type SkillSummary = {
  id: SkillId;
  name: string;
  display_name: string;
  description: string;
  scope: SkillScope;
  /** The user's decision or the scope default; a skill with `error` is never invoked. */
  enabled: boolean;
  user_invocable: boolean;
  model_invocable: boolean;
  version?: string;
  /** Drive- or workspace-relative path of the skill file (or directory). */
  location: string;
  size_bytes: number;
  /** The plugin contributing a `plugin`-scope skill. */
  plugin?: string;
  /** Why the skill cannot be used (too large, malformed, a link, ...). */
  error?: string;
};

/** A root or entry that could not be considered at all. */
export type SkillIssue = {
  scope: SkillScope;
  location: string;
  error: string;
};

/** Result of `skill.list`. */
export type SkillList = {
  skills: SkillSummary[];
  issues: SkillIssue[];
  /** The resolved workspace id when one was included. */
  workspace_id: string | null;
  /** Skills or issues were left out to fit one message. */
  truncated: boolean;
};

/** A front matter value: `aidex_skills::FrontMatterValue` (untagged). */
export type FrontMatterValue = boolean | string | string[];

export type FrontMatterEntry = { key: string; value: FrontMatterValue };

/** Result of `skill.show`: the summary plus the redacted body and front matter. */
export type SkillDetail = SkillSummary & {
  /** Absent when the skill has an error. */
  body?: string;
  front_matter: FrontMatterEntry[];
  /**
   * True when the daemon hid credential-like text in what it returned. Saving that text back with
   * `skill.save` would overwrite the original, so such a skill is edited in its SKILL.md file.
   */
  body_redacted?: boolean;
};

/** Params of `skill.list`; without `workspace_id` only drive and plugin skills are visible. */
export type SkillListParams = { workspace_id?: string };

export type SkillShowParams = { id: SkillId; workspace_id?: string };

export type SkillSetEnabledParams = {
  id: SkillId;
  workspace_id?: string;
  enabled: boolean;
};

/** Params of `skill.save`: creates or replaces a drive skill (`skills/<name>/SKILL.md`). */
export type SkillDraft = {
  name: string;
  description: string;
  /** The Markdown instructions. */
  body: string;
  version?: string | null;
};

/** Result of `skill.delete` (drive skills only). */
export type SkillDeleted = { deleted: SkillSummary };

/** `aidex_harness::MAX_INVOKED_SKILLS`: most skills one `message.send`/`agent.start` may invoke. */
export const MAX_INVOKED_SKILLS = 8;
