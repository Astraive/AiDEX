//! `plugin.*` methods: install portable plugin bundles (skills and MCP servers) from a
//! local folder or `.zip`, review them, trust their exact tree hash, enable, disable,
//! and uninstall them (`aidex-plugins`).
//!
//! Plugins install DISABLED. Enabling requires trusting the plugin's exact SHA-256
//! tree hash (the hash `plugin.show` reports); a changed tree revokes that trust and
//! disables the plugin. Failures carry `data.reason` from the plugin store
//! (`plugin_not_found`, `not_trusted`, `hash_mismatch`, `unsafe_content`, ...).

use crate::{
    errors::RpcError,
    methods::native_view::WIRE_BUDGET,
    router::{Router, RouterError},
    rpc::{expect_no_params, parse_params, to_result},
    services::Services,
};
use aidex_plugins::{PluginDetail, PluginError};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{path::Path, sync::Arc};

pub const LIST: &str = "plugin.list";
pub const SHOW: &str = "plugin.show";
pub const INSTALL: &str = "plugin.install";
pub const TRUST: &str = "plugin.trust";
pub const SET_ENABLED: &str = "plugin.set_enabled";
pub const UNINSTALL: &str = "plugin.uninstall";

/// Params of `plugin.show` and `plugin.uninstall`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NameParams {
    pub name: String,
}

/// Params of `plugin.install`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InstallParams {
    /// Absolute path of a plugin folder or `.zip` archive on this computer.
    pub source_path: String,
    /// Replace an installed plugin of the same name (trust carries over only when
    /// the new tree hash is identical; the plugin is installed disabled either way).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replace: bool,
}

/// Params of `plugin.trust`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TrustParams {
    pub name: String,
    /// The exact tree hash the user reviewed (from `plugin.show`).
    pub tree_sha256: String,
    /// Also enable the plugin ("Trust and enable").
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub enable: bool,
}

/// Params of `plugin.set_enabled`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SetEnabledParams {
    pub name: String,
    pub enabled: bool,
}

/// Map a plugin error to its stable wire code with `data.reason`.
pub fn plugin_error(error: PluginError) -> RpcError {
    let data = match &error {
        PluginError::EventNotRecorded(_) => {
            json!({"reason": error.reason(), "committed": true})
        }
        _ => json!({"reason": error.reason()}),
    };
    RpcError::service(error.code(), error.to_string()).with_data(data)
}

fn encoded_len<T: Serialize>(value: &T) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |bytes| bytes.len())
}

/// Fit a plugin review into `budget` bytes of JSON. The README goes first, then
/// skills from the end (`skills_truncated`), and as a last resort the contents
/// (with an error saying so). The summary (name, counts, tree hash, trust) is always
/// kept, so a committed install is never reported as a failure and the plugin can
/// still be reviewed and trusted by its hash.
fn bounded_detail(detail: &mut PluginDetail, budget: usize) {
    let mut total = encoded_len(detail);
    if total <= budget {
        return;
    }
    let Some(contents) = detail.contents.as_mut() else {
        return;
    };
    if contents.readme.take().is_some() {
        contents.readme_truncated = true;
        total = encoded_len(detail);
        if total <= budget {
            return;
        }
    }
    let Some(contents) = detail.contents.as_mut() else {
        return;
    };
    while total > budget {
        let Some(skill) = contents.skills.pop() else {
            break;
        };
        contents.skills_truncated = true;
        total = total.saturating_sub(encoded_len(&skill).saturating_add(1));
    }
    if encoded_len(detail) > budget {
        detail.contents = None;
        detail
            .summary
            .error
            .get_or_insert_with(|| "the plugin's contents are too large to list for review".into());
    }
}

pub(crate) fn register(router: &mut Router, services: &Arc<Services>) -> Result<(), RouterError> {
    let shared = Arc::clone(services);
    router.register(LIST, move |_context, params| {
        expect_no_params(&params)?;
        to_result(&shared.plugins().list().map_err(plugin_error)?)
    })?;
    let shared = Arc::clone(services);
    router.register(SHOW, move |_context, params| {
        let params: NameParams = parse_params(params)?;
        let mut detail = shared.plugins().show(&params.name).map_err(plugin_error)?;
        bounded_detail(&mut detail, WIRE_BUDGET);
        to_result(&detail)
    })?;
    let shared = Arc::clone(services);
    router.register(INSTALL, move |_context, params| {
        let params: InstallParams = parse_params(params)?;
        let mut outcome = shared
            .plugins()
            .install(Path::new(params.source_path.trim()), params.replace)
            .map_err(plugin_error)?;
        // A replaced plugin installs disabled: drop the servers it contributed.
        super::mcp::sync_plugins(&shared);
        // The install is committed: never let an oversized review turn it into an error.
        bounded_detail(&mut outcome.plugin, WIRE_BUDGET);
        to_result(&outcome)
    })?;
    let shared = Arc::clone(services);
    router.register(TRUST, move |_context, params| {
        let params: TrustParams = parse_params(params)?;
        let summary = shared
            .plugins()
            .trust(&params.name, &params.tree_sha256, params.enable)
            .map_err(plugin_error)?;
        super::mcp::sync_plugins(&shared);
        to_result(&summary)
    })?;
    let shared = Arc::clone(services);
    router.register(SET_ENABLED, move |_context, params| {
        let params: SetEnabledParams = parse_params(params)?;
        let summary = shared
            .plugins()
            .set_enabled(&params.name, params.enabled)
            .map_err(plugin_error)?;
        // Enabled plugins contribute DISABLED MCP entries; disabled ones none.
        super::mcp::sync_plugins(&shared);
        to_result(&summary)
    })?;
    let shared = Arc::clone(services);
    router.register(UNINSTALL, move |_context, params| {
        let params: NameParams = parse_params(params)?;
        let removed = shared
            .plugins()
            .uninstall(&params.name)
            .map_err(plugin_error)?;
        super::mcp::sync_plugins(&shared);
        to_result(&removed)
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
    use aidex_events::{Cursor, EventFilter};
    use aidex_plugins::{InstallOutcome, PluginDetail, PluginList, PluginSummary, Uninstalled};
    use serde_json::Value;
    use std::fs;

    const SECRET: &str = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";

    struct Fixture {
        _root: tempfile::TempDir,
        source: tempfile::TempDir,
        services: Arc<Services>,
        router: Router,
        context: CallContext,
    }

    fn fixture() -> Fixture {
        let root = crate::services::testing::temp_drive();
        let shutdown = ShutdownSignal::new();
        let services = Arc::new(Services::open(root.path(), shutdown.clone()).unwrap());
        let mut router = Router::new();
        register(&mut router, &services).unwrap();
        let context = CallContext::detached(ClientInfo::new("test", "1"), shutdown);
        let source = tempfile::tempdir().unwrap();
        let plugin = source.path().join("review");
        for (path, content) in [
            (
                ".claude-plugin/plugin.json",
                r#"{"name": "review", "version": "1.0.0", "description": "Reviews code.", "permissions": {"network": ["api.github.com"]}}"#
                    .to_owned(),
            ),
            (
                "skills/review/SKILL.md",
                "---\nname: review\ndescription: Checks a diff\n---\nBody.\n".to_owned(),
            ),
            (
                ".mcp.json",
                format!(
                    r#"{{"mcpServers": {{"github": {{"command": "npx", "env": {{"GITHUB_TOKEN": "{SECRET}"}}}}}}}}"#
                ),
            ),
        ] {
            let path = plugin.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }
        Fixture {
            _root: root,
            source,
            services,
            router,
            context,
        }
    }

    impl Fixture {
        fn plugin_path(&self) -> String {
            self.source.path().join("review").display().to_string()
        }

        fn call<T: serde::de::DeserializeOwned>(&self, method: &str, params: Value) -> T {
            serde_json::from_value(self.router.call(&self.context, method, params).unwrap())
                .unwrap()
        }

        fn fail(&self, method: &str, params: Value) -> (Option<ErrorCode>, Value) {
            let error = self.router.call(&self.context, method, params).unwrap_err();
            (error.kind(), error.data.unwrap_or(Value::Null))
        }

        fn event_names(&self) -> Vec<&'static str> {
            let page = self
                .services
                .events()
                .replay(&EventFilter::all(), Cursor::ZERO, 100)
                .unwrap();
            page.events
                .iter()
                .map(|event| event.event.name())
                .filter(|name| name.starts_with("extension."))
                .collect()
        }
    }

    #[test]
    fn install_review_trust_enable_and_uninstall() {
        let fixture = fixture();
        let empty: PluginList = fixture.call(LIST, json!({}));
        assert!(empty.plugins.is_empty() && empty.unregistered.is_empty());

        let raw = fixture
            .router
            .call(
                &fixture.context,
                INSTALL,
                json!({"source_path": fixture.plugin_path()}),
            )
            .unwrap();
        assert!(!raw.to_string().contains(SECRET), "{raw}");
        let installed: InstallOutcome = serde_json::from_value(raw).unwrap();
        let summary = &installed.plugin.summary;
        assert!(!summary.enabled && !summary.trusted && !installed.replaced);
        let hash = summary.tree_sha256.clone().unwrap();

        let shown = fixture
            .router
            .call(&fixture.context, SHOW, json!({"name": "review"}))
            .unwrap();
        assert_eq!(shown["tree_sha256"], hash.as_str());
        assert_eq!(
            shown["manifest"]["permissions"]["network"][0],
            "api.github.com"
        );
        assert_eq!(shown["contents"]["skills"][0]["name"], "review");
        assert_eq!(
            shown["contents"]["mcp_servers"][0]["env_keys"][0],
            "GITHUB_TOKEN"
        );
        assert!(!shown.to_string().contains(SECRET), "{shown}");
        let detail: PluginDetail = serde_json::from_value(shown).unwrap();
        assert_eq!(detail, installed.plugin);

        let (code, data) = fixture.fail(SET_ENABLED, json!({"name": "review", "enabled": true}));
        assert_eq!(code, Some(ErrorCode::PermissionDenied));
        assert_eq!(data["reason"], "not_trusted");
        let (code, data) = fixture.fail(
            TRUST,
            json!({"name": "review", "tree_sha256": "0".repeat(64), "enable": true}),
        );
        assert_eq!(code, Some(ErrorCode::Conflict));
        assert_eq!(data["reason"], "hash_mismatch");

        let trusted: PluginSummary = fixture.call(
            TRUST,
            json!({"name": "review", "tree_sha256": hash, "enable": true}),
        );
        assert!(trusted.trusted && trusted.enabled);
        assert_eq!(
            fixture
                .services
                .plugins()
                .enabled_contributions()
                .unwrap()
                .len(),
            1
        );
        let disabled: PluginSummary =
            fixture.call(SET_ENABLED, json!({"name": "review", "enabled": false}));
        assert!(!disabled.enabled && disabled.trusted);
        let enabled: PluginSummary =
            fixture.call(SET_ENABLED, json!({"name": "review", "enabled": true}));
        assert!(enabled.enabled);
        assert_eq!(
            fixture.event_names(),
            [
                "extension.enabled",
                "extension.disabled",
                "extension.enabled"
            ]
        );

        // Editing the installed files revokes trust and disables the plugin.
        let skill = fixture
            .services
            .paths()
            .plugins()
            .join("review/skills/review/SKILL.md");
        fs::write(skill, "---\nname: review\n---\nChanged.\n").unwrap();
        let listed: PluginList = fixture.call(LIST, json!({}));
        assert!(!listed.plugins[0].enabled && !listed.plugins[0].trusted);
        assert!(
            fixture
                .services
                .plugins()
                .enabled_contributions()
                .unwrap()
                .is_empty()
        );
        assert_eq!(fixture.event_names().last(), Some(&"extension.disabled"));

        let (code, data) = fixture.fail(INSTALL, json!({"source_path": fixture.plugin_path()}));
        assert_eq!(code, Some(ErrorCode::Conflict));
        assert_eq!(data["reason"], "plugin_exists");
        let replaced: InstallOutcome = fixture.call(
            INSTALL,
            json!({"source_path": fixture.plugin_path(), "replace": true}),
        );
        assert!(replaced.replaced);
        assert_eq!(
            replaced.plugin.summary.tree_sha256.as_deref(),
            Some(hash.as_str())
        );

        let removed: Uninstalled = fixture.call(UNINSTALL, json!({"name": "review"}));
        assert!(removed.removed_files && !removed.was_enabled);
        let (code, data) = fixture.fail(SHOW, json!({"name": "review"}));
        assert_eq!(code, Some(ErrorCode::NotFound));
        assert_eq!(data["reason"], "plugin_not_found");
    }

    #[test]
    fn params_are_closed_and_unsafe_sources_refused() {
        let fixture = fixture();
        for (method, params) in [
            (LIST, json!({"all": true})),
            (SHOW, json!({})),
            (
                INSTALL,
                json!({"source_path": fixture.plugin_path(), "trust": true}),
            ),
            (TRUST, json!({"name": "review"})),
            (SET_ENABLED, json!({"name": "review"})),
            (UNINSTALL, json!({"name": "review", "force": true})),
        ] {
            let (code, _) = fixture.fail(method, params);
            assert_eq!(code, Some(ErrorCode::InvalidParams), "{method}");
        }
        let (code, data) = fixture.fail(INSTALL, json!({"source_path": "relative/plugin"}));
        assert_eq!(code, Some(ErrorCode::InvalidArgument));
        assert_eq!(data["reason"], "invalid_request");
        let (_, data) = fixture.fail(SHOW, json!({"name": "../vault"}));
        assert_eq!(data["reason"], "invalid_request");

        let executable = fixture.source.path().join("review/bin/tool.exe");
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(&executable, b"MZ\x90\x00").unwrap();
        let (code, data) = fixture.fail(INSTALL, json!({"source_path": fixture.plugin_path()}));
        assert_eq!(code, Some(ErrorCode::InvalidArgument));
        assert_eq!(data["reason"], "executable_refused");
        let listed: PluginList = fixture.call(LIST, Value::Null);
        assert!(listed.plugins.is_empty());
    }

    #[test]
    fn enabled_plugins_contribute_skills_and_disabled_mcp_servers() {
        let fixture = fixture();
        let mut router = Router::new();
        register(&mut router, &fixture.services).unwrap();
        crate::methods::skills::register(&mut router, &fixture.services).unwrap();
        crate::methods::mcp::register(&mut router, &fixture.services).unwrap();
        let call =
            |method: &str, params: Value| router.call(&fixture.context, method, params).unwrap();
        let plugin_servers = |listing: &Value| -> Vec<Value> {
            listing["servers"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|server| server["source"] == "plugin")
                .cloned()
                .collect()
        };
        let skill_ids = |listing: &Value| -> Vec<String> {
            listing["skills"]
                .as_array()
                .unwrap()
                .iter()
                .map(|skill| skill["id"].as_str().unwrap().to_owned())
                .collect()
        };

        let installed: InstallOutcome =
            serde_json::from_value(call(INSTALL, json!({"source_path": fixture.plugin_path()})))
                .unwrap();
        let hash = installed.plugin.summary.tree_sha256.unwrap();
        // Installed but untrusted: nothing is contributed.
        assert!(skill_ids(&call("skill.list", json!({}))).is_empty());
        assert!(plugin_servers(&call("mcp.list", json!({}))).is_empty());

        call(
            TRUST,
            json!({"name": "review", "tree_sha256": hash, "enable": true}),
        );
        let skills = call("skill.list", json!({}));
        assert_eq!(skill_ids(&skills), ["plugin:review"]);
        assert_eq!(skills["skills"][0]["plugin"], "review");
        assert_eq!(skills["skills"][0]["enabled"], true);
        let servers = plugin_servers(&call("mcp.list", json!({})));
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0]["id"], "review-github");
        assert_eq!(servers[0]["enabled"], false);
        assert_eq!(servers[0]["source_owner"], "review");
        assert!(!servers.iter().any(|s| s.to_string().contains(SECRET)));

        // Disabling the plugin withdraws both; enabling brings the server back disabled.
        call(SET_ENABLED, json!({"name": "review", "enabled": false}));
        assert!(skill_ids(&call("skill.list", json!({}))).is_empty());
        assert!(plugin_servers(&call("mcp.list", json!({}))).is_empty());
        call(SET_ENABLED, json!({"name": "review", "enabled": true}));
        let servers = plugin_servers(&call("mcp.list", json!({})));
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0]["enabled"], false);

        call(UNINSTALL, json!({"name": "review"}));
        assert!(plugin_servers(&call("mcp.list", json!({}))).is_empty());
        assert!(skill_ids(&call("skill.list", json!({}))).is_empty());
    }

    /// Keeps one file unreadable while it is alive.
    struct Unreadable {
        #[cfg(windows)]
        _handle: fs::File,
        #[cfg(unix)]
        path: std::path::PathBuf,
    }

    impl Unreadable {
        fn new(path: &Path) -> Self {
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                // No sharing: every other open of the file fails until this handle closes.
                let handle = fs::OpenOptions::new()
                    .read(true)
                    .share_mode(0)
                    .open(path)
                    .unwrap();
                Self { _handle: handle }
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(0o000)).unwrap();
                Self {
                    path: path.to_path_buf(),
                }
            }
        }
    }

    #[cfg(unix)]
    impl Drop for Unreadable {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&self.path, fs::Permissions::from_mode(0o644));
        }
    }

    #[test]
    fn unreadable_plugins_contribute_nothing_and_lose_their_servers() {
        let fixture = fixture();
        let mut router = Router::new();
        register(&mut router, &fixture.services).unwrap();
        crate::methods::skills::register(&mut router, &fixture.services).unwrap();
        crate::methods::mcp::register(&mut router, &fixture.services).unwrap();
        let call =
            |method: &str, params: Value| router.call(&fixture.context, method, params).unwrap();
        let plugin_servers = || -> Vec<Value> {
            let listing = call("mcp.list", json!({}));
            listing["servers"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|server| server["source"] == "plugin")
                .cloned()
                .collect()
        };
        let skill_ids = || -> Vec<String> {
            let listing = call("skill.list", json!({}));
            listing["skills"]
                .as_array()
                .unwrap()
                .iter()
                .map(|skill| skill["id"].as_str().unwrap().to_owned())
                .collect()
        };
        let installed: InstallOutcome =
            serde_json::from_value(call(INSTALL, json!({"source_path": fixture.plugin_path()})))
                .unwrap();
        let hash = installed.plugin.summary.tree_sha256.unwrap();
        call(
            TRUST,
            json!({"name": "review", "tree_sha256": hash, "enable": true}),
        );
        assert_eq!(plugin_servers().len(), 1);
        // The user enabled the server the plugin contributed.
        let store = fixture.services.mcp().unwrap().store();
        store.set_enabled("review-github", true).unwrap();

        let skill = fixture
            .services
            .paths()
            .plugins()
            .join("review/skills/review/SKILL.md");
        let locked = Unreadable::new(&skill);
        if fs::read(&skill).is_ok() {
            // Running with privileges that ignore file permissions (e.g. root).
            return;
        }
        // Files that cannot be verified contribute no skills, and the servers they
        // contributed are stopped and removed so nothing of the plugin can run.
        assert!(skill_ids().is_empty());
        assert!(plugin_servers().is_empty());
        assert!(!store.exists("review-github"));
        let listed: PluginList = fixture.call(LIST, json!({}));
        assert!(!listed.plugins[0].enabled);
        assert_eq!(
            listed.plugins[0].trusted_sha256.as_deref(),
            Some(hash.as_str())
        );

        // Readable again, the plugin contributes again (its server disabled, as on enable).
        drop(locked);
        assert_eq!(skill_ids(), ["plugin:review"]);
        let servers = plugin_servers();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0]["enabled"], false);
        let listed: PluginList = fixture.call(LIST, json!({}));
        assert!(listed.plugins[0].enabled && listed.plugins[0].trusted);
    }

    #[test]
    fn reviewed_mcp_servers_are_the_ones_imported() {
        let fixture = fixture();
        let mut router = Router::new();
        register(&mut router, &fixture.services).unwrap();
        crate::methods::mcp::register(&mut router, &fixture.services).unwrap();
        let call =
            |method: &str, params: Value| router.call(&fixture.context, method, params).unwrap();
        // Claude Code plugins may ship the server map without the `mcpServers` wrapper.
        let plugin = fixture.source.path().join("bare");
        for (path, content) in [
            (
                "aidex-plugin.json",
                r#"{"name": "bare", "version": "1", "description": "Bare map."}"#,
            ),
            (
                ".mcp.json",
                "\u{feff}{\"github\": {\"command\": \"npx\", \"args\": [\"-y\", \"server-github\"]}}",
            ),
        ] {
            let path = plugin.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }
        let installed: InstallOutcome = serde_json::from_value(call(
            INSTALL,
            json!({"source_path": plugin.display().to_string()}),
        ))
        .unwrap();
        let reviewed = &installed.plugin.contents.as_ref().unwrap().mcp_servers;
        assert_eq!(installed.plugin.summary.mcp_server_count, 1);
        assert_eq!(reviewed[0].name, "github");
        let hash = installed.plugin.summary.tree_sha256.unwrap();
        call(
            TRUST,
            json!({"name": "bare", "tree_sha256": hash, "enable": true}),
        );
        let listing = call("mcp.list", json!({}));
        let imported: Vec<&Value> = listing["servers"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|server| server["source_owner"] == "bare")
            .collect();
        assert_eq!(imported.len(), 1, "{listing}");
        assert_eq!(imported[0]["id"], "bare-github");
        assert_eq!(imported[0]["enabled"], false);
    }

    #[test]
    fn oversized_reviews_fit_the_wire_budget() {
        let fixture = fixture();
        let plugin = fixture.source.path().join("noisy");
        let write = |path: &str, content: &str| {
            let path = plugin.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        };
        write(
            "aidex-plugin.json",
            r#"{"name": "noisy", "version": "1", "description": "Noisy."}"#,
        );
        write("README.md", &"# Noisy\n".repeat(2048));
        // Quotes double in JSON: 256 skills of 1 KiB names and descriptions exceed 1 MiB.
        let quotes = "\"".repeat(1026);
        for index in 0..256 {
            write(
                &format!("skills/s{index:03}/SKILL.md"),
                &format!("---\nname: {quotes}\ndescription: {quotes}\n---\nBody.\n"),
            );
        }
        let source = plugin.display().to_string();
        let raw = fixture
            .router
            .call(&fixture.context, INSTALL, json!({ "source_path": source }))
            .unwrap();
        assert!(raw.to_string().len() <= WIRE_BUDGET);
        let installed: InstallOutcome = serde_json::from_value(raw).unwrap();
        assert_eq!(installed.plugin.summary.skill_count, 256);
        let contents = installed.plugin.contents.as_ref().unwrap();
        assert!(contents.skills_truncated && contents.readme_truncated);
        assert!(contents.readme.is_none());
        assert!(!contents.skills.is_empty() && contents.skills.len() < 256);

        let shown = fixture
            .router
            .call(&fixture.context, SHOW, json!({"name": "noisy"}))
            .unwrap();
        assert!(shown.to_string().len() <= WIRE_BUDGET);
        assert_eq!(shown["contents"]["skills_truncated"], true);
        assert!(shown["tree_sha256"].is_string());

        // With no room for any contents, only the summary is kept.
        let mut detail: PluginDetail = serde_json::from_value(shown).unwrap();
        let mut summary_only = detail.clone();
        summary_only.contents = None;
        summary_only.summary.error =
            Some("the plugin's contents are too large to list for review".into());
        let budget = encoded_len(&summary_only);
        bounded_detail(&mut detail, budget);
        assert_eq!(detail, summary_only);
        assert!(encoded_len(&detail) <= budget);
        assert_eq!(detail.summary.skill_count, 256);
    }

    #[test]
    fn event_failures_report_a_committed_change() {
        let error = plugin_error(PluginError::EventNotRecorded("log closed".into()));
        assert_eq!(error.kind(), Some(ErrorCode::InternalError));
        assert_eq!(
            error.data.unwrap(),
            json!({"reason": "event_not_recorded", "committed": true})
        );
    }
}
