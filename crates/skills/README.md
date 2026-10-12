# aidex-skills

Portable agent skills: directories holding a `SKILL.md` (Claude Code, Codex, and t3code compatible), discovered from the workspace, the drive, and enabled plugins, with persisted enablement and drive skill authoring. Skill text is untrusted: everything this crate returns is credential-redacted, and callers must wrap skill bodies as untrusted data before they reach a model.

## Format

A skill is a directory `<root>/<skill-dir>/` with `SKILL.md` (or `skill.md`, any letter case) that starts with YAML front matter:

```markdown
---
name: release-notes
description: Write release notes from the git log.
version: 1.2.0
allowed-tools: [Read, Grep, "Bash(git log:*)"]
---

# Steps
1. ...
```

| Key | Rule |
| --- | --- |
| `name` | Required slug `[a-z0-9][a-z0-9-]{0,63}`; when missing, the directory name if it is a valid slug. |
| `description` | Required, at most 1024 characters, no control characters except line breaks and tabs. |
| `version` | Optional, at most 64 characters on one line. |
| `display-name` / `display_name` | Optional; defaults to the title-cased name (`release-notes` → `Release Notes`). |
| `user-invocable` / `user_invocable` | Optional boolean (YAML 1.1 spellings accepted), default `true`. |
| `disable-model-invocation` | Optional boolean, default `false`; `model_invocable` is its negation. |
| `allowed-tools` / `allowed_tools` | Optional list (or comma-separated string), informational only: AiDEX policy still decides every tool call. |

Unknown keys are ignored. The front matter parser (`FrontMatter`, `SkillDocument`) is a small dependency-free YAML subset: top-level `key: value` pairs with plain, single-quoted, and double-quoted scalars (escapes, multi-line folding), booleans, null, `|`/`>` block scalars with chomping and indentation indicators, inline `[a, "b"]` lists, and `- item` block lists. Nested mappings, flow mappings, and lists of mappings are skipped; comments, a UTF-8 BOM, CRLF line endings, and a `...` terminator are accepted. A file without front matter is a skill without a description (listed with an error). Parse errors name the line, never the value.

Limits: the body (after the front matter) is at most 64 KiB, the front matter 16 KiB, and the whole file `MAX_SKILL_FILE_BYTES`; larger or non-UTF-8 files are listed with an error rather than read.

## Roots, scopes, and precedence

`SkillCatalog::discover(paths, workspace, extra_roots)` scans the roots below in order; for each name only the first skill found is kept (a higher root shadows a lower one silently; two directories of the same root declaring one name are reported as an issue).

1. Workspace `.aidex/skills/` — scope `project`.
2. Workspace `.agents/skills/`, `.claude/skills/`, `.codex/skills/` — scope `repo` (skills other agents use; read-only here).
3. Drive `DrivePaths::skills()` — scope `drive` (the user's portable skills, authored from AiDEX).
4. `extra_roots`: enabled, trusted plugins' `skills/` directories — scope `plugin` (`ExtraSkillRoot { scope: SkillScope::Plugin, root, owner }`).

Ids are `<scope>:<name>` (`drive:release-notes`). Hidden entries are skipped. Symbolic links and junctions are never followed: a linked skill directory or file is listed with an error, and a linked root (or a linked directory on the way to it below the workspace or drive root) is reported in `SkillCatalog::issues()` and skipped. Each root yields at most 256 skills (and at most 512 skill files are read from it), a workspace's project and repo roots together at most 256 (so they never crowd out drive and plugin skills), and a catalog at most 512. Skills are rescanned on every call; nothing is cached.

`SkillSummary` carries `{id, name, display_name, description, scope, enabled, user_invocable, model_invocable, version?, location, size_bytes, plugin?, error?}`; `location` is drive- or workspace-relative. `SkillCatalog::inspect` adds the redacted body and the front matter entries, and `body_redacted: true` (omitted when false) when credential-like text was hidden in the body, description, or version, so an editor knows saving them back would lose that text; `SkillCatalog::load_body(id)` returns the redacted body, name, and description of an enabled, valid skill (re-read and re-validated from disk) for injection, and `SkillError::Unavailable` (`skill_unavailable`) otherwise.

## Enablement

`private_config/skills.toml` (closed schema, version 1, a file of its own so older daemons never read it) stores only explicit decisions:

```toml
version = 1

[[skill]]
scope = "repo"
name = "deploy"
workspace_id = "wsp_..."
enabled = true
```

Without a decision, drive and plugin skills are enabled and project and repo skills are disabled until the user enables them (issue #24: untrusted extensions start disabled). Workspace decisions are kept per workspace id. `SkillService::set_enabled` serialises writes, refuses to enable a skill with an error, and emits `extension.enabled` / `extension.disabled` (`kind: skill`, `extension_id` = the skill id; workspace-scoped for project and repo skills) when the effective state changes.

## Drive skill authoring

`SkillService::save_drive_skill(SkillDraft { name, description, body, version? })` validates the draft, refuses text that looks like it holds a credential (`credential_detected`; skills are stored unencrypted on the drive), and writes `skills/<name>/SKILL.md` atomically (a drive skill that declares that name in another directory is replaced in place; a directory holding a different skill is a conflict). Replacing a skill keeps every top-level front matter key the draft does not set (`name`, `description`, `version`) exactly as written, such as `user-invocable`, `disable-model-invocation`, `allowed-tools`, `display-name`, nested mappings, and comments; when the kept keys would not read back unchanged (or would make the skill invalid) the save is refused, and a file whose front matter cannot be parsed is replaced whole. `delete_drive_skill(name)` removes the skill's directory and forgets its enablement decision. Project, repo, and plugin skills are never written.

## Errors

`SkillError::code()` maps to `aidex_core::ServiceErrorCode` and `reason()` to a stable `data.reason`: `invalid_skill_id`, `skill_not_found`, `invalid_skill`, `credential_detected`, `skill_unavailable`, `workspace_required`, `skill_conflict`, `invalid_state_file`, `unsupported_state_version`, `unsafe_path`, `io`, `event_not_recorded`. Messages never quote skill text or state-file values.

## Daemon and CLI

`aidexd` serves `skill.list`, `skill.show`, `skill.set_enabled`, `skill.save`, and `skill.delete` (`crates/aidexd/src/methods/skills.rs`, see `protocol/commands/README.md`); the CLI exposes `aidex skills list|show|enable|disable [--workspace ID|NAME]`. The daemon passes enabled, trusted plugins' `skills/` directories as `extra_roots` on every call. Invocation (`message.send`/`agent.start` `skills`) and the model-invocable catalog with its `skill_load` tool are served by the native harness (`crates/harness/README.md`).
