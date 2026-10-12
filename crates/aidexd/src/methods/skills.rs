//! `skill.*` methods: list, inspect, enable, and disable skills, and author the
//! user's drive skills (`aidex-skills`).
//!
//! `workspace_id` (id or name) adds that workspace's project (`.aidex/skills/`)
//! and repo (`.agents/`, `.claude/`, `.codex/` `skills/`) skills; without it only
//! drive and plugin skills are visible. Skills are rescanned on every call.
//! Every text field is credential-redacted by the crate, and skill text is still
//! untrusted: nothing here sends it to a model.

use crate::{
    errors::RpcError,
    methods::{native_view::WIRE_BUDGET, workspace::workspace_error},
    router::{Router, RouterError},
    rpc::{parse_params, to_result},
    services::Services,
};
use aidex_skills::{
    ExtraSkillRoot, SkillCatalog, SkillDraft, SkillError, SkillId, SkillIssue, SkillSummary,
    SkillWorkspace,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

pub const LIST: &str = "skill.list";
pub const SHOW: &str = "skill.show";
pub const SET_ENABLED: &str = "skill.set_enabled";
pub const SAVE: &str = "skill.save";
pub const DELETE: &str = "skill.delete";

/// Most issues `skill.list` reports.
const MAX_LISTED_ISSUES: usize = 64;

/// Params of `skill.list`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ListParams {
    /// Workspace id or name whose project and repo skills are included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
}

/// Params of `skill.show`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ShowParams {
    /// `<scope>:<name>`.
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
}

/// Params of `skill.set_enabled`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SetEnabledParams {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    pub enabled: bool,
}

/// Params of `skill.delete` (drive skills only).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DeleteParams {
    pub name: String,
}

/// Result of `skill.list`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SkillList {
    /// Visible skills sorted by name (the first of each name in precedence order).
    pub skills: Vec<SkillSummary>,
    /// Roots and entries that could not be considered (links, unreadable
    /// directories, duplicate names within one root).
    pub issues: Vec<SkillIssue>,
    /// The resolved workspace id, when one was included.
    pub workspace_id: Option<String>,
    /// `true` when skills or issues were left out to fit one message.
    pub truncated: bool,
}

/// Result of `skill.delete`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SkillDeleted {
    pub deleted: SkillSummary,
}

/// Map a skill error to its wire code with `data.reason`. An unavailable skill
/// (unknown, disabled, or invalid when invoked) is `invalid_params` with
/// `data.reason = "skill_unavailable"`; a change whose event could not be
/// recorded also carries `committed: true`.
pub fn skill_error(error: SkillError) -> RpcError {
    let reason = error.reason();
    match error {
        SkillError::Unavailable { .. } => {
            RpcError::invalid_params(error.to_string()).with_data(json!({ "reason": reason }))
        }
        SkillError::EventNotRecorded(_) => RpcError::service(error.code(), error.to_string())
            .with_data(json!({ "reason": reason, "committed": true })),
        _ => RpcError::service(error.code(), error.to_string())
            .with_data(json!({ "reason": reason })),
    }
}

/// The workspace named by `reference` (id or name), with its confined root;
/// `None` when no workspace was given.
pub fn skill_workspace(
    services: &Services,
    reference: Option<&str>,
) -> Result<Option<SkillWorkspace>, RpcError> {
    let Some(reference) = reference
        .map(str::trim)
        .filter(|reference| !reference.is_empty())
    else {
        return Ok(None);
    };
    let workspace = services
        .workspaces()
        .find(reference)
        .map_err(workspace_error)?;
    let handle = services
        .workspaces()
        .handle(&workspace.id)
        .map_err(workspace_error)?;
    Ok(Some(SkillWorkspace {
        id: handle.id().clone(),
        root: handle.root().root().to_path_buf(),
    }))
}

/// Skill roots contributed by enabled, trusted plugins (scope `plugin`),
/// re-verified against each plugin's trusted tree hash on every call.
pub fn plugin_skill_roots(services: &Services) -> Vec<ExtraSkillRoot> {
    crate::services::contributions::plugin_skill_roots(services.plugins())
}

fn skill_id(id: &str) -> Result<SkillId, RpcError> {
    SkillId::parse(id).map_err(skill_error)
}

fn catalog(services: &Services, workspace: Option<&str>) -> Result<SkillCatalog, RpcError> {
    let workspace = skill_workspace(services, workspace)?;
    services
        .skills()
        .catalog(workspace.as_ref(), &plugin_skill_roots(services))
        .map_err(skill_error)
}

fn encoded_len<T: Serialize>(value: &T) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |bytes| bytes.len())
}

/// The listing, cut to fit `budget` bytes of JSON (issues first, at most
/// [`MAX_LISTED_ISSUES`], then skills in name order).
fn bounded_list(catalog: &SkillCatalog, budget: usize) -> SkillList {
    let workspace_id = catalog.workspace_id().map(ToString::to_string);
    // The envelope and the result's own keys.
    let mut used = 256 + workspace_id.as_ref().map_or(0, String::len);
    let mut truncated = catalog.issues().len() > MAX_LISTED_ISSUES;
    let mut issues = Vec::new();
    for issue in catalog.issues().iter().take(MAX_LISTED_ISSUES) {
        let size = encoded_len(issue).saturating_add(1);
        if used.saturating_add(size) > budget {
            truncated = true;
            break;
        }
        used += size;
        issues.push(issue.clone());
    }
    let mut skills = Vec::new();
    for summary in catalog.summaries() {
        let size = encoded_len(&summary).saturating_add(1);
        if used.saturating_add(size) > budget {
            truncated = true;
            break;
        }
        used += size;
        skills.push(summary);
    }
    SkillList {
        skills,
        issues,
        workspace_id,
        truncated,
    }
}

pub(crate) fn register(router: &mut Router, services: &Arc<Services>) -> Result<(), RouterError> {
    let shared = Arc::clone(services);
    router.register(LIST, move |_context, params| {
        let params: ListParams = parse_params(params)?;
        let catalog = catalog(&shared, params.workspace_id.as_deref())?;
        to_result(&bounded_list(&catalog, WIRE_BUDGET))
    })?;
    let shared = Arc::clone(services);
    router.register(SHOW, move |_context, params| {
        let params: ShowParams = parse_params(params)?;
        let id = skill_id(&params.id)?;
        let catalog = catalog(&shared, params.workspace_id.as_deref())?;
        to_result(&catalog.inspect(&id).map_err(skill_error)?)
    })?;
    let shared = Arc::clone(services);
    router.register(SET_ENABLED, move |_context, params| {
        let params: SetEnabledParams = parse_params(params)?;
        let id = skill_id(&params.id)?;
        let workspace = skill_workspace(&shared, params.workspace_id.as_deref())?;
        let summary = shared
            .skills()
            .set_enabled(
                workspace.as_ref(),
                &plugin_skill_roots(&shared),
                &id,
                params.enabled,
            )
            .map_err(skill_error)?;
        to_result(&summary)
    })?;
    let shared = Arc::clone(services);
    router.register(SAVE, move |_context, params| {
        let draft: SkillDraft = parse_params(params)?;
        to_result(
            &shared
                .skills()
                .save_drive_skill(&draft)
                .map_err(skill_error)?,
        )
    })?;
    let shared = Arc::clone(services);
    router.register(DELETE, move |_context, params| {
        let params: DeleteParams = parse_params(params)?;
        let deleted = shared
            .skills()
            .delete_drive_skill(params.name.trim())
            .map_err(skill_error)?;
        to_result(&SkillDeleted { deleted })
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        errors::ErrorCode,
        lifecycle::ShutdownSignal,
        router::{CallContext, ClientInfo},
    };
    use aidex_events::{Cursor, EventFilter, EventKind, ExtensionKind};
    use aidex_skills::SkillDetail;
    use serde::de::DeserializeOwned;
    use serde_json::Value;
    use std::{fs, path::Path};

    struct Fixture {
        _drive: tempfile::TempDir,
        services: Arc<Services>,
        router: Router,
        context: CallContext,
    }

    fn fixture() -> Fixture {
        let drive = crate::services::testing::temp_drive();
        let shutdown = ShutdownSignal::new();
        let services = Arc::new(Services::open(drive.path(), shutdown.clone()).unwrap());
        let mut router = Router::new();
        register(&mut router, &services).unwrap();
        let context = CallContext::detached(ClientInfo::new("skills-test", "1"), shutdown);
        Fixture {
            _drive: drive,
            services,
            router,
            context,
        }
    }

    impl Fixture {
        fn call<T: DeserializeOwned>(&self, method: &str, params: Value) -> T {
            let value = self
                .router
                .call(&self.context, method, params.clone())
                .unwrap_or_else(|error| panic!("{method} {params}: {error}"));
            serde_json::from_value(value).unwrap()
        }

        fn fail(&self, method: &str, params: Value) -> RpcError {
            self.router
                .call(&self.context, method, params.clone())
                .expect_err(&format!("{method} {params} should fail"))
        }

        fn extension_events(&self) -> Vec<(String, String, Option<String>)> {
            let page = self
                .services
                .events()
                .replay(&EventFilter::all(), Cursor::ZERO, 100)
                .unwrap();
            page.events
                .iter()
                .filter_map(|envelope| match &envelope.event {
                    EventKind::ExtensionEnabled { extension_id, kind }
                    | EventKind::ExtensionDisabled { extension_id, kind } => {
                        assert_eq!(*kind, ExtensionKind::Skill);
                        Some((
                            envelope.event.name().to_owned(),
                            extension_id.to_string(),
                            envelope
                                .scope
                                .workspace_id
                                .as_ref()
                                .map(ToString::to_string),
                        ))
                    }
                    _ => None,
                })
                .collect()
        }
    }

    fn reason(error: &RpcError) -> &str {
        error
            .data
            .as_ref()
            .and_then(|data| data["reason"].as_str())
            .unwrap_or("")
    }

    fn write_skill(root: &Path, dir: &str, text: &str) {
        let dir = root.join(dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), text).unwrap();
    }

    fn token() -> String {
        format!("{}{}", "ghp_", "FAKE0fake1FAKE2fake3FAKE4fake5")
    }

    #[test]
    fn drive_skills_are_saved_listed_shown_toggled_and_deleted() {
        let fixture = fixture();
        let empty: SkillList = fixture.call(LIST, Value::Null);
        assert!(empty.skills.is_empty() && empty.issues.is_empty() && !empty.truncated);
        assert_eq!(empty.workspace_id, None);

        let saved: SkillSummary = fixture.call(
            SAVE,
            json!({
                "name": "release-notes",
                "description": "Write release notes",
                "body": "# Steps\n\n1. Read the log",
                "version": "1.0.0"
            }),
        );
        assert_eq!(saved.id, "drive:release-notes");
        assert_eq!(saved.location, "skills/release-notes/SKILL.md");
        assert!(saved.enabled, "drive skills are enabled by default");

        let list: SkillList = fixture.call(LIST, json!({}));
        assert_eq!(list.skills, std::slice::from_ref(&saved));
        let detail: SkillDetail = fixture.call(SHOW, json!({"id": "drive:release-notes"}));
        assert_eq!(detail.summary, saved);
        assert_eq!(detail.body.as_deref(), Some("# Steps\n\n1. Read the log"));
        let keys: Vec<&str> = detail
            .front_matter
            .iter()
            .map(|entry| entry.key.as_str())
            .collect();
        assert_eq!(keys, ["name", "description", "version"]);

        let disabled: SkillSummary = fixture.call(
            SET_ENABLED,
            json!({"id": "drive:release-notes", "enabled": false}),
        );
        assert!(!disabled.enabled);
        let enabled: SkillSummary = fixture.call(
            SET_ENABLED,
            json!({"id": "drive:release-notes", "enabled": true}),
        );
        assert!(enabled.enabled);
        assert_eq!(
            fixture.extension_events(),
            [
                (
                    "extension.disabled".into(),
                    "drive:release-notes".into(),
                    None
                ),
                (
                    "extension.enabled".into(),
                    "drive:release-notes".into(),
                    None
                ),
            ]
        );

        let deleted: SkillDeleted = fixture.call(DELETE, json!({"name": "release-notes"}));
        assert_eq!(deleted.deleted.id, "drive:release-notes");
        let list: SkillList = fixture.call(LIST, Value::Null);
        assert!(list.skills.is_empty());
        let missing = fixture.fail(DELETE, json!({"name": "release-notes"}));
        assert_eq!(missing.kind(), Some(ErrorCode::NotFound));
        assert_eq!(reason(&missing), "skill_not_found");
    }

    #[test]
    fn editing_a_drive_skill_keeps_its_settings_and_flags_hidden_text() {
        let fixture = fixture();
        let skills = fixture.services.paths().skills();
        write_skill(
            &skills,
            "deploy",
            "---\nname: deploy\ndescription: Deploy\ndisable-model-invocation: true\nuser-invocable: false\nallowed-tools: [Bash]\n---\nShip it.\n",
        );
        let shown = fixture
            .router
            .call(&fixture.context, SHOW, json!({"id": "drive:deploy"}))
            .unwrap();
        assert!(shown.get("body_redacted").is_none(), "{shown}");

        let saved: SkillSummary = fixture.call(
            SAVE,
            json!({"name": "deploy", "description": "Deploy the app", "body": "Ship it carefully."}),
        );
        assert_eq!(saved.description, "Deploy the app");
        assert!(!saved.model_invocable && !saved.user_invocable, "{saved:?}");
        let detail: SkillDetail = fixture.call(SHOW, json!({"id": "drive:deploy"}));
        assert_eq!(detail.body.as_deref(), Some("Ship it carefully."));
        assert!(
            detail
                .front_matter
                .iter()
                .any(|entry| entry.key == "allowed-tools")
        );

        // A body shown with credential-like text hidden is flagged for editors.
        write_skill(
            &skills,
            "db",
            &format!(
                "---\nname: db\ndescription: Database\n---\nRun with {}.\n",
                token()
            ),
        );
        let shown = fixture
            .router
            .call(&fixture.context, SHOW, json!({"id": "drive:db"}))
            .unwrap();
        assert_eq!(shown["body_redacted"], true);
        assert!(!shown.to_string().contains(&token()), "{shown}");
    }

    #[test]
    fn workspace_skills_resolve_through_the_registry_and_start_disabled() {
        let fixture = fixture();
        let project = tempfile::tempdir().unwrap();
        write_skill(
            &project.path().join(".aidex").join("skills"),
            "deploy",
            "---\nname: deploy\ndescription: Project deploy\n---\nShip it.\n",
        );
        write_skill(
            &project.path().join(".claude").join("skills"),
            "review",
            "---\nname: review\ndescription: Repo review\n---\nReview it.\n",
        );
        let workspace = fixture
            .services
            .workspaces()
            .add_host(project.path(), Some("app"))
            .unwrap();

        let without: SkillList = fixture.call(LIST, json!({}));
        assert!(without.skills.is_empty(), "no workspace: no project skills");
        let list: SkillList = fixture.call(LIST, json!({"workspace_id": "app"}));
        assert_eq!(list.workspace_id.as_deref(), Some(workspace.id.as_str()));
        let ids: Vec<(&str, bool)> = list
            .skills
            .iter()
            .map(|skill| (skill.id.as_str(), skill.enabled))
            .collect();
        assert_eq!(ids, [("project:deploy", false), ("repo:review", false)]);
        assert_eq!(list.skills[1].location, ".claude/skills/review/SKILL.md");

        let needs_workspace = fixture.fail(
            SET_ENABLED,
            json!({"id": "project:deploy", "enabled": true}),
        );
        assert_eq!(reason(&needs_workspace), "workspace_required");
        let enabled: SkillSummary = fixture.call(
            SET_ENABLED,
            json!({"id": "project:deploy", "workspace_id": workspace.id.as_str(), "enabled": true}),
        );
        assert!(enabled.enabled);
        assert_eq!(
            fixture.extension_events(),
            [(
                "extension.enabled".into(),
                "project:deploy".into(),
                Some(workspace.id.to_string())
            )]
        );
        let detail: SkillDetail =
            fixture.call(SHOW, json!({"id": "project:deploy", "workspace_id": "app"}));
        assert_eq!(detail.body.as_deref(), Some("Ship it."));
        assert!(detail.summary.enabled);

        let unknown = fixture.fail(LIST, json!({"workspace_id": "nope"}));
        assert_eq!(reason(&unknown), "workspace_not_found");
        // Workspace skills are never saved or deleted through `skill.*`.
        let refused = fixture.fail(DELETE, json!({"name": "deploy"}));
        assert_eq!(reason(&refused), "skill_not_found");
        assert!(
            project
                .path()
                .join(".aidex/skills/deploy/SKILL.md")
                .is_file()
        );
    }

    #[test]
    fn params_are_closed_and_errors_carry_reasons_without_secrets() {
        let fixture = fixture();
        for (method, params) in [
            (LIST, json!({"workspace": "x"})),
            (SHOW, json!({})),
            (SET_ENABLED, json!({"id": "drive:x"})),
            (SAVE, json!({"name": "x", "description": "d"})),
            (
                SAVE,
                json!({"name": "x", "description": "d", "body": "b", "scope": "project"}),
            ),
            (DELETE, json!({"name": "x", "force": true})),
        ] {
            let error = fixture.fail(method, params.clone());
            assert_eq!(
                error.kind(),
                Some(ErrorCode::InvalidParams),
                "{method} {params}"
            );
        }
        for (method, params, expected) in [
            (SHOW, json!({"id": "global:x"}), "invalid_skill_id"),
            (SHOW, json!({"id": "drive:Bad"}), "invalid_skill_id"),
            (SHOW, json!({"id": "drive:missing"}), "skill_not_found"),
            (SHOW, json!({"id": "repo:missing"}), "workspace_required"),
            (DELETE, json!({"name": "../escape"}), "invalid_skill_id"),
            (
                SAVE,
                json!({"name": "Bad Name", "description": "d", "body": "b"}),
                "invalid_skill",
            ),
        ] {
            let error = fixture.fail(method, params.clone());
            assert_eq!(reason(&error), expected, "{method} {params}: {error}");
        }

        let token = token();
        let leaked = fixture.fail(
            SAVE,
            json!({"name": "leaky", "description": "d", "body": format!("use {token}")}),
        );
        assert_eq!(reason(&leaked), "credential_detected");
        assert!(!leaked.to_string().contains(&token));
        assert!(!fixture.services.paths().skills().join("leaky").exists());

        // A broken skill can be shown (with its error) and disabled, not enabled.
        write_skill(
            &fixture.services.paths().skills(),
            "broken",
            &format!("---\nname: broken\nnote: {token}\n---\n"),
        );
        let detail: SkillDetail = fixture.call(SHOW, json!({"id": "drive:broken"}));
        assert!(detail.summary.error.is_some() && detail.body.is_none());
        let refused = fixture.fail(SET_ENABLED, json!({"id": "drive:broken", "enabled": true}));
        assert_eq!(reason(&refused), "invalid_skill");
        let _: SkillSummary =
            fixture.call(SET_ENABLED, json!({"id": "drive:broken", "enabled": false}));
        let listed = fixture
            .router
            .call(&fixture.context, LIST, Value::Null)
            .unwrap();
        assert!(!listed.to_string().contains(&token), "{listed}");
    }

    #[test]
    fn unavailable_skills_map_to_invalid_params() {
        let error = skill_error(SkillError::Unavailable {
            id: "drive:x".into(),
            reason: "it is disabled".into(),
        });
        assert_eq!(error.kind(), Some(ErrorCode::InvalidParams));
        assert_eq!(reason(&error), "skill_unavailable");
        let error = skill_error(SkillError::EventNotRecorded("full".into()));
        assert_eq!(error.data.unwrap()["committed"], true);
    }

    #[test]
    fn listings_are_cut_to_the_wire_budget() {
        let fixture = fixture();
        let skills = fixture.services.paths().skills();
        for index in 0..12 {
            let name = format!("skill-{index:02}");
            write_skill(
                &skills,
                &name,
                &format!(
                    "---\nname: {name}\ndescription: {}\n---\nBody\n",
                    "d".repeat(900)
                ),
            );
        }
        let catalog = fixture.services.skills().catalog(None, &[]).unwrap();
        let full = bounded_list(&catalog, WIRE_BUDGET);
        assert_eq!((full.skills.len(), full.truncated), (12, false));
        let cut = bounded_list(&catalog, 4 * 1024);
        assert!(cut.truncated);
        assert!(!cut.skills.is_empty() && cut.skills.len() < 12);
        assert!(serde_json::to_vec(&cut).unwrap().len() < 4 * 1024);
        assert_eq!(cut.skills[0].name, "skill-00");
    }
}
