//! Discovery: precedence across roots, scopes, default enablement, size caps,
//! link refusal, and body loading.

mod common;

use std::fs;

use aidex_skills::{
    ExtraSkillRoot, MAX_BODY_BYTES, MAX_SKILL_FILE_BYTES, MAX_SKILLS_PER_ROOT,
    MAX_WORKSPACE_SKILLS, SkillCatalog, SkillError, SkillId, SkillScope, SkillSummary,
};
use common::{dir_link, drive, fake_token, file_link, skill, skill_text, workspace, write};

fn by_name<'a>(skills: &'a [SkillSummary], name: &str) -> &'a SkillSummary {
    skills
        .iter()
        .find(|skill| skill.name == name)
        .unwrap_or_else(|| panic!("no skill `{name}` in {skills:#?}"))
}

fn id(text: &str) -> SkillId {
    SkillId::parse(text).unwrap()
}

#[test]
fn roots_apply_in_precedence_order_with_scope_defaults() {
    let drive = drive();
    let (ws_dir, ws) = workspace("wsp_one");
    let root = ws_dir.path();
    skill(&root.join(".aidex/skills"), "review", "Project review");
    skill(
        &root.join(".claude/skills"),
        "review",
        "Shadowed by project",
    );
    skill(&root.join(".agents/skills"), "lint", "Agents lint");
    write(
        &root.join(".codex/skills"),
        "lint",
        "skill.md",
        &skill_text("lint", "Shadowed by .agents", "x"),
    );
    write(
        &root.join(".codex/skills"),
        "fmt",
        "skill.md",
        &skill_text("fmt", "Codex fmt", "x"),
    );
    let skills_dir = drive.paths.skills();
    skill(&skills_dir, "review", "Shadowed drive review");
    skill(&skills_dir, "notes", "Drive notes");
    let plugin_skills = drive.paths.plugins().join("acme").join("skills");
    skill(&plugin_skills, "notes", "Shadowed plugin notes");
    skill(&plugin_skills, "deploy", "Plugin deploy");
    let extra = [ExtraSkillRoot {
        scope: SkillScope::Plugin,
        root: plugin_skills.clone(),
        owner: "acme".into(),
    }];

    let catalog = SkillCatalog::discover(&drive.paths, Some(&ws), &extra).unwrap();
    assert!(catalog.issues().is_empty(), "{:?}", catalog.issues());
    let skills = catalog.summaries();
    let names: Vec<&str> = skills.iter().map(|skill| skill.name.as_str()).collect();
    assert_eq!(names, ["deploy", "fmt", "lint", "notes", "review"]);

    let review = by_name(&skills, "review");
    assert_eq!(review.id, "project:review");
    assert_eq!(review.description, "Project review");
    assert_eq!(review.location, ".aidex/skills/review/SKILL.md");
    assert!(!review.enabled, "project skills start disabled");

    let lint = by_name(&skills, "lint");
    assert_eq!(
        (lint.scope, lint.description.as_str()),
        (SkillScope::Repo, "Agents lint")
    );
    assert_eq!(lint.location, ".agents/skills/lint/SKILL.md");
    assert!(!lint.enabled, "repo skills start disabled");
    let fmt = by_name(&skills, "fmt");
    assert_eq!(fmt.location, ".codex/skills/fmt/skill.md");

    let notes = by_name(&skills, "notes");
    assert_eq!((notes.id.as_str(), notes.enabled), ("drive:notes", true));
    assert_eq!(notes.location, "skills/notes/SKILL.md");
    assert_eq!(notes.display_name, "Notes");
    assert!(notes.user_invocable && notes.model_invocable);
    assert!(notes.plugin.is_none());

    let deploy = by_name(&skills, "deploy");
    assert_eq!(
        (deploy.id.as_str(), deploy.enabled),
        ("plugin:deploy", true)
    );
    assert_eq!(deploy.plugin.as_deref(), Some("acme"));
    assert_eq!(deploy.location, "plugins/acme/skills/deploy/SKILL.md");
    assert!(deploy.size_bytes > 0);

    let enabled: Vec<&str> = catalog.enabled().map(|skill| skill.id.as_str()).collect();
    assert_eq!(enabled, ["plugin:deploy", "drive:notes"]);

    // Without a workspace only the drive and plugins remain.
    let catalog = SkillCatalog::discover(&drive.paths, None, &extra).unwrap();
    let skills = catalog.summaries();
    assert_eq!(by_name(&skills, "review").id, "drive:review");
    assert_eq!(skills.len(), 3);
    assert_eq!(
        catalog.summary(&id("project:review")).unwrap_err(),
        SkillError::WorkspaceRequired
    );
    assert!(matches!(
        catalog.summary(&id("plugin:notes")),
        Err(SkillError::NotFound(_))
    ));
}

#[test]
fn extra_roots_must_be_named_plugin_roots() {
    let drive = drive();
    for (scope, owner) in [(SkillScope::Drive, "acme"), (SkillScope::Plugin, " ")] {
        let extra = [ExtraSkillRoot {
            scope,
            root: drive.paths.plugins(),
            owner: owner.into(),
        }];
        let error = SkillCatalog::discover(&drive.paths, None, &extra).unwrap_err();
        assert_eq!(error.reason(), "invalid_skill", "{error}");
    }
}

#[test]
fn front_matter_problems_are_listed_with_errors() {
    let drive = drive();
    let root = drive.paths.skills();
    // The directory name stands in for a missing `name`.
    write(
        &root,
        "dir-named",
        "SKILL.md",
        "---\ndescription: 'Named by its directory'\nuser-invocable: false\ndisable-model-invocation: true\nallowed-tools: [Read, Grep]\nversion: 2.0.1\n---\nBody\n",
    );
    write(
        &root,
        "plain",
        "SKILL.md",
        "# No front matter\n\nJust text.\n",
    );
    write(
        &root,
        "nodesc",
        "SKILL.md",
        "---\nname: nodesc\n---\nBody\n",
    );
    write(
        &root,
        "badbool",
        "SKILL.md",
        "---\nname: badbool\ndescription: d\nuser-invocable: maybe\n---\n",
    );
    write(
        &root,
        "Bad_Dir",
        "SKILL.md",
        "---\nname: Not A Slug\ndescription: d\n---\n",
    );
    write(
        &root,
        "unclosed",
        "SKILL.md",
        "---\nname: unclosed\ndescription: d\n",
    );
    fs::create_dir_all(root.join("empty-dir")).unwrap();
    skill(&root.join(".hidden"), "secret", "Hidden roots are skipped");
    write(
        &root,
        "dup-a",
        "SKILL.md",
        &skill_text("same", "First", "a"),
    );
    write(
        &root,
        "dup-b",
        "SKILL.md",
        &skill_text("same", "Second", "b"),
    );

    let catalog = SkillCatalog::discover(&drive.paths, None, &[]).unwrap();
    let skills = catalog.summaries();
    let names: Vec<&str> = skills.iter().map(|skill| skill.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "bad-dir",
            "badbool",
            "dir-named",
            "nodesc",
            "plain",
            "same",
            "unclosed"
        ]
    );

    let named = by_name(&skills, "dir-named");
    assert_eq!(named.error, None);
    assert_eq!(named.description, "Named by its directory");
    assert_eq!(named.version.as_deref(), Some("2.0.1"));
    assert!(!named.user_invocable && !named.model_invocable);

    for (name, fragment) in [
        ("plain", "description"),
        ("nodesc", "description"),
        ("badbool", "true or false"),
        ("bad-dir", "`name`"),
        ("unclosed", "not closed"),
    ] {
        let summary = by_name(&skills, name);
        let error = summary
            .error
            .as_deref()
            .unwrap_or_else(|| panic!("{summary:?}"));
        assert!(error.contains(fragment), "{name}: {error}");
        assert!(summary.description.is_empty());
        assert!(!summary.user_invocable && !summary.model_invocable);
        assert!(catalog.enabled().all(|enabled| enabled.name != name));
    }
    assert_eq!(by_name(&skills, "same").description, "First");
    assert_eq!(catalog.issues().len(), 1, "{:?}", catalog.issues());
    assert!(catalog.issues()[0].error.contains("already defines"));
    assert_eq!(catalog.issues()[0].location, "skills/dup-b/SKILL.md");

    // Invalid skills cannot be loaded; their detail has no body.
    let detail = catalog.inspect(&id("drive:nodesc")).unwrap();
    assert!(detail.body.is_none() && detail.front_matter.is_empty());
    let error = catalog.load_body(&id("drive:nodesc")).unwrap_err();
    assert_eq!(error.reason(), "skill_unavailable");
}

#[test]
fn oversized_and_binary_skill_files_are_refused() {
    let drive = drive();
    let root = drive.paths.skills();
    let huge = "x".repeat(usize::try_from(MAX_SKILL_FILE_BYTES).unwrap() + 1);
    write(&root, "huge", "SKILL.md", &skill_text("huge", "d", &huge));
    let long_body = "y".repeat(MAX_BODY_BYTES + 1);
    write(
        &root,
        "long",
        "SKILL.md",
        &skill_text("long", "d", &long_body),
    );
    let fits = "z".repeat(MAX_BODY_BYTES);
    write(&root, "fits", "SKILL.md", &skill_text("fits", "d", &fits));
    let dir = root.join("binary");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("SKILL.md"), [0xff, 0xfe, 0x00]).unwrap();

    let catalog = SkillCatalog::discover(&drive.paths, None, &[]).unwrap();
    let skills = catalog.summaries();
    let huge = by_name(&skills, "huge");
    assert!(
        huge.error.as_deref().unwrap().contains("larger than"),
        "{huge:?}"
    );
    assert!(huge.size_bytes > MAX_SKILL_FILE_BYTES);
    let long = by_name(&skills, "long");
    assert!(
        long.error
            .as_deref()
            .unwrap()
            .contains("instructions are larger"),
        "{long:?}"
    );
    assert!(
        by_name(&skills, "binary")
            .error
            .as_deref()
            .unwrap()
            .contains("UTF-8")
    );
    let fits = catalog.load_body(&id("drive:fits")).unwrap();
    assert_eq!(fits.body.len(), MAX_BODY_BYTES);
}

#[test]
fn links_are_never_followed() {
    let drive = drive();
    let outside = tempfile::tempdir().unwrap();
    skill(outside.path(), "elsewhere", "Outside the drive");
    let root = drive.paths.skills();
    fs::create_dir_all(&root).unwrap();
    skill(&root, "real", "A real skill");

    // A linked skill directory is listed with an error, never read.
    if dir_link(&outside.path().join("elsewhere"), &root.join("linked")) {
        let catalog = SkillCatalog::discover(&drive.paths, None, &[]).unwrap();
        let skills = catalog.summaries();
        let linked = by_name(&skills, "linked");
        assert!(
            linked.error.as_deref().unwrap().contains("links"),
            "{linked:?}"
        );
        assert!(linked.description.is_empty());
        assert_eq!(
            catalog.load_body(&id("drive:linked")).unwrap_err().reason(),
            "skill_unavailable"
        );
        assert!(by_name(&skills, "real").error.is_none());
    }

    // A linked skill file is refused.
    let target = outside.path().join("elsewhere").join("SKILL.md");
    let dir = root.join("file-link");
    fs::create_dir_all(&dir).unwrap();
    if file_link(&target, &dir.join("SKILL.md")) {
        let catalog = SkillCatalog::discover(&drive.paths, None, &[]).unwrap();
        let skills = catalog.summaries();
        let linked = by_name(&skills, "file-link");
        assert!(
            linked.error.as_deref().unwrap().contains("link"),
            "{linked:?}"
        );
    }

    // A linked workspace root is reported as an issue and skipped.
    let (ws_dir, ws) = workspace("wsp_links");
    fs::create_dir_all(ws_dir.path().join(".aidex")).unwrap();
    if dir_link(outside.path(), &ws_dir.path().join(".aidex").join("skills")) {
        let catalog = SkillCatalog::discover(&drive.paths, Some(&ws), &[]).unwrap();
        assert!(
            catalog
                .summaries()
                .iter()
                .all(|skill| skill.name != "elsewhere")
        );
        let issue = &catalog.issues()[0];
        assert_eq!(issue.scope, SkillScope::Project);
        assert_eq!(issue.location, ".aidex/skills");
        assert!(issue.error.contains("symbolic link"), "{issue:?}");
    }

    // So is a linked drive skills directory.
    let other = common::drive();
    fs::remove_dir_all(other.paths.skills()).ok();
    if dir_link(outside.path(), &other.paths.skills()) {
        let catalog = SkillCatalog::discover(&other.paths, None, &[]).unwrap();
        assert!(catalog.summaries().is_empty());
        assert_eq!(catalog.issues()[0].scope, SkillScope::Drive);
    }
}

#[test]
fn bodies_and_details_are_redacted() {
    let drive = drive();
    let token = fake_token();
    write(
        &drive.paths.skills(),
        "deploy",
        "SKILL.md",
        &format!(
            "---\r\nname: deploy\r\ndescription: \"Deploy with {token}\"\r\nallowed-tools:\r\n  - Bash\r\n  - Read\r\nuser_invocable: true\r\n---\r\n\r\n# Deploy\r\n\r\nUse {token} to push.\r\n"
        ),
    );
    let catalog = SkillCatalog::discover(&drive.paths, None, &[]).unwrap();
    let summary = catalog.summary(&id("drive:deploy")).unwrap();
    assert!(summary.error.is_none(), "{summary:?}");
    assert!(!summary.description.contains(&token), "{summary:?}");
    assert!(summary.description.starts_with("Deploy with "));

    let body = catalog.load_body(&id("drive:deploy")).unwrap();
    assert_eq!(body.name, "deploy");
    assert!(body.body.starts_with("# Deploy\n\nUse "), "{}", body.body);
    assert!(!body.body.contains(&token) && !body.description.contains(&token));
    assert!(!body.body.contains('\r'));

    let detail = catalog.inspect(&id("drive:deploy")).unwrap();
    assert!(!detail.body.as_deref().unwrap().contains(&token));
    let keys: Vec<&str> = detail
        .front_matter
        .iter()
        .map(|entry| entry.key.as_str())
        .collect();
    assert_eq!(
        keys,
        ["name", "description", "allowed-tools", "user_invocable"]
    );
    let json = serde_json::to_value(&detail).unwrap();
    assert_eq!(json["id"], "drive:deploy");
    assert_eq!(
        json["front_matter"][2]["value"],
        serde_json::json!(["Bash", "Read"])
    );
    assert_eq!(json["front_matter"][3]["value"], true);
    assert!(!json.to_string().contains(&token));
    // Saving the shown text back would store the marker: the detail says so.
    assert!(detail.body_redacted);
    assert_eq!(json["body_redacted"], true);

    // A skill renamed on disk after listing is refused rather than injected.
    write(
        &drive.paths.skills(),
        "deploy",
        "SKILL.md",
        &skill_text("renamed", "d", "x"),
    );
    assert_eq!(
        catalog.load_body(&id("drive:deploy")).unwrap_err().reason(),
        "skill_unavailable"
    );
    assert!(
        catalog
            .inspect(&id("drive:deploy"))
            .unwrap()
            .summary
            .error
            .is_some()
    );
}

#[test]
fn details_say_when_editable_text_was_redacted() {
    let drive = drive();
    let token = fake_token();
    skill(&drive.paths.skills(), "clean", "Nothing hidden");
    write(
        &drive.paths.skills(),
        "described",
        "SKILL.md",
        &skill_text("described", &format!("Uses {token}"), "Plain body."),
    );
    write(
        &drive.paths.skills(),
        "tooled",
        "SKILL.md",
        &format!("---\nname: tooled\ndescription: d\nnote: \"{token}\"\n---\nPlain body.\n"),
    );
    let catalog = SkillCatalog::discover(&drive.paths, None, &[]).unwrap();

    let clean = catalog.inspect(&id("drive:clean")).unwrap();
    assert!(!clean.body_redacted);
    let json = serde_json::to_value(&clean).unwrap();
    assert!(json.get("body_redacted").is_none(), "{json}");
    // Older payloads without the field still parse.
    let parsed: aidex_skills::SkillDetail = serde_json::from_value(json).unwrap();
    assert_eq!(parsed, clean);

    // The description is saved back by an editor too.
    assert!(
        catalog
            .inspect(&id("drive:described"))
            .unwrap()
            .body_redacted
    );
    // Other front matter is kept from the file on save, so hiding it there is harmless.
    let tooled = catalog.inspect(&id("drive:tooled")).unwrap();
    assert!(!tooled.body_redacted);
    assert!(!serde_json::to_string(&tooled).unwrap().contains(&token));
}

#[test]
fn invalid_state_files_fail_discovery_without_echoing_values() {
    let drive = drive();
    skill(&drive.paths.skills(), "notes", "d");
    fs::create_dir_all(drive.paths.config()).unwrap();
    let token = fake_token();
    fs::write(
        drive.paths.config().join("skills.toml"),
        format!("version = 1\nleaked = \"{token}\"\n"),
    )
    .unwrap();
    let error = SkillCatalog::discover(&drive.paths, None, &[]).unwrap_err();
    assert_eq!(error.reason(), "invalid_state_file");
    assert!(!error.to_string().contains(&token), "{error}");
}

#[test]
fn workspace_skills_never_crowd_out_drive_and_plugin_skills() {
    let drive = drive();
    let (ws_dir, ws) = workspace("wsp_many");
    let root = ws_dir.path();
    for index in 0..MAX_SKILLS_PER_ROOT {
        skill(&root.join(".aidex/skills"), &format!("p{index:03}"), "d");
        skill(&root.join(".agents/skills"), &format!("a{index:03}"), "d");
    }
    skill(&drive.paths.skills(), "review", "Drive review");
    let plugin_skills = drive.paths.plugins().join("acme").join("skills");
    skill(&plugin_skills, "deploy", "Plugin deploy");
    let extra = [ExtraSkillRoot {
        scope: SkillScope::Plugin,
        root: plugin_skills,
        owner: "acme".into(),
    }];

    let catalog = SkillCatalog::discover(&drive.paths, Some(&ws), &extra).unwrap();
    let skills = catalog.summaries();
    assert_eq!(by_name(&skills, "review").id, "drive:review");
    assert_eq!(by_name(&skills, "deploy").id, "plugin:deploy");
    let workspace = skills
        .iter()
        .filter(|skill| skill.scope.is_workspace())
        .count();
    assert_eq!(workspace, MAX_WORKSPACE_SKILLS);
    assert_eq!(skills.len(), MAX_WORKSPACE_SKILLS + 2);
    assert!(
        catalog
            .issues()
            .iter()
            .any(|issue| issue.location == ".agents/skills"),
        "{:?}",
        catalog.issues()
    );
    let enabled: Vec<&str> = catalog.enabled().map(|skill| skill.id.as_str()).collect();
    assert_eq!(enabled, ["plugin:deploy", "drive:review"]);
}

#[test]
fn roots_of_unusable_entries_stop_being_parsed() {
    let drive = drive();
    let skills_dir = drive.paths.skills();
    // Directory names with no usable fallback (no ASCII letters or digits) and
    // skill files that declare no name.
    for index in 0..(2 * MAX_SKILLS_PER_ROOT + 8) {
        let unusable: String = format!("{index:03}")
            .chars()
            .map(|digit| char::from_u32(0x3b1 + digit.to_digit(10).unwrap()).unwrap())
            .collect();
        write(
            &skills_dir,
            &format!("_{unusable}"),
            "SKILL.md",
            "---\ndescription: d\n---\nx\n",
        );
    }
    let catalog = SkillCatalog::discover(&drive.paths, None, &[]).unwrap();
    assert!(catalog.summaries().is_empty());
    let issues = catalog.issues();
    assert_eq!(
        issues.len(),
        2 * MAX_SKILLS_PER_ROOT + 1,
        "one per parsed file"
    );
    assert!(
        issues
            .last()
            .unwrap()
            .error
            .contains("skill files; the rest are ignored"),
        "{:?}",
        issues.last()
    );
}
