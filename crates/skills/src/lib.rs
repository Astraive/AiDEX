//! Portable agent skills: `SKILL.md` directories (Claude Code / Codex / t3code
//! compatible) discovered from the workspace, the drive, and enabled plugins.
//!
//! - [`FrontMatter`]: a small dependency-free parser for the YAML front matter
//!   subset skills use (scalars, quoted strings, booleans, lists, block scalars).
//! - [`SkillCatalog::discover`]: every root in precedence order (project, repo,
//!   drive, plugin; the first skill with a name wins), refusing symbolic links and
//!   junctions and capping file sizes; [`SkillCatalog::load_body`] returns the
//!   redacted instructions of an enabled skill for injection.
//! - [`SkillService`]: enablement in `private_config/skills.toml` (drive and plugin
//!   skills enabled by default, project and repo skills disabled until the user
//!   enables them) with `extension.enabled`/`extension.disabled` events, and
//!   authoring of drive skills.
//!
//! Skill text is untrusted: everything leaving this crate is credential-redacted,
//! and callers must wrap bodies as untrusted data before they reach a model.

mod authoring;
mod catalog;
mod discover;
mod error;
mod front_matter;
mod fsio;
mod manifest;
mod name;
mod service;
mod state;

pub use authoring::{SkillDraft, render_skill_file};
pub use catalog::{FrontMatterEntry, SkillBody, SkillCatalog, SkillDetail, SkillSummary};
pub use discover::{
    ExtraSkillRoot, MAX_ROOT_ENTRIES, MAX_SKILLS, MAX_SKILLS_PER_ROOT, MAX_WORKSPACE_SKILLS,
    PROJECT_SKILLS_DIR, REPO_SKILLS_DIRS, SkillIssue, SkillWorkspace,
};
pub use error::SkillError;
pub use front_matter::{FrontMatter, FrontMatterError, FrontMatterValue, SkillDocument};
pub use manifest::{
    MAX_ALLOWED_TOOLS, MAX_BODY_BYTES, MAX_DESCRIPTION_CHARS, MAX_DISPLAY_NAME_CHARS,
    MAX_FRONT_MATTER_BYTES, MAX_SKILL_FILE_BYTES, MAX_VERSION_CHARS, SkillManifest,
};
pub use name::{MAX_SKILL_NAME_BYTES, SkillId, SkillScope, display_name, is_skill_name};
pub use service::SkillService;
pub use state::{SKILLS_STATE_FILE, STATE_VERSION, SkillStateEntry, SkillStateFile};
