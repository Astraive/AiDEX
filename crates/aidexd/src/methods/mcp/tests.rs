//! `mcp.*` handler tests. Stdio servers run the `aidex-mcp` Python fixture
//! (`AIDEX_TEST_PYTHON`, as in CI) through the broker's sandbox.

use super::*;
use crate::{
    lifecycle::ShutdownSignal,
    router::{CallContext, ClientInfo},
};
use aidex_vault::SecretString;
use serde_json::Value;
use std::time::{Duration, Instant};

const PASSPHRASE: &str = "fixture passphrase only";

/// Fields drop in order: the router's handlers, then the services (which stop
/// every server), then the drive.
struct Fixture {
    router: Router,
    context: CallContext,
    services: Arc<Services>,
    _drive: tempfile::TempDir,
}

impl Fixture {
    fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        self.router.call(&self.context, method, params)
    }

    fn ok(&self, method: &str, params: Value) -> Value {
        self.call(method, params)
            .unwrap_or_else(|error| panic!("{method}: {error:?}"))
    }

    fn refused(&self, method: &str, params: Value) -> (Option<ErrorCode>, String) {
        let error = self.call(method, params).unwrap_err();
        let reason = error
            .data
            .as_ref()
            .and_then(|data| data["reason"].as_str())
            .unwrap_or_default()
            .to_owned();
        (error.kind(), reason)
    }

    fn status(&self, id: &str) -> String {
        let listed = self.ok(LIST, json!({}));
        listed["servers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|server| server["id"] == id)
            .map(|server| server["status"].as_str().unwrap().to_owned())
            .unwrap_or_default()
    }

    fn wait_for_status(&self, id: &str, wanted: &str) {
        let until = Instant::now() + Duration::from_secs(10);
        while self.status(id) != wanted {
            assert!(Instant::now() < until, "{id} stayed {}", self.status(id));
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn event_names(&self) -> Vec<String> {
        self.services
            .events()
            .replay(
                &aidex_events::EventFilter::all(),
                aidex_events::Cursor::ZERO,
                256,
            )
            .unwrap()
            .events
            .iter()
            .map(|event| event.event.name().to_owned())
            .collect()
    }
}

fn fixture() -> Fixture {
    let drive = crate::services::testing::temp_drive();
    let shutdown = ShutdownSignal::new();
    let services = Arc::new(Services::open(drive.path(), shutdown.clone()).unwrap());
    let context = CallContext::detached(ClientInfo::new("mcp-test", "1"), shutdown);
    let mut router = Router::new();
    register(&mut router, &services).unwrap();
    Fixture {
        router,
        context,
        services,
        _drive: drive,
    }
}

fn python() -> String {
    std::env::var("AIDEX_TEST_PYTHON").unwrap_or_else(|_| {
        if cfg!(windows) {
            "C:/Users/gskne/AppData/Local/Programs/Python/Python312/python.exe".into()
        } else {
            "/usr/bin/python3".into()
        }
    })
}

/// The `aidex-mcp` fixture server script.
fn fixture_script() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../mcp/tests/fixtures/mcp_server.py")
}

/// A stdio server running the `aidex-mcp` fixture.
fn stdio_spec(id: &str, enabled: bool) -> Value {
    let script = fixture_script();
    json!({
        "id": id,
        "name": format!("Fixture {id}"),
        "transport": "stdio",
        "command": python(),
        "args": ["-u", script.to_string_lossy(), "normal"],
        "enabled": enabled,
    })
}

#[test]
fn servers_are_configured_started_and_stopped_over_rpc() {
    let f = fixture();
    assert_eq!(
        f.ok(LIST, json!({})),
        json!({"servers": [], "invalid": [], "tools_omitted": false})
    );
    // Closed schema: no field can carry a secret value.
    let (code, _) = f.refused(
        ADD,
        json!({"id": "x", "transport": "stdio", "command": "x", "env": {"TOKEN": "v"}}),
    );
    assert_eq!(code, Some(ErrorCode::InvalidParams));
    let mut leaky = stdio_spec("leaky", false);
    leaky["args"]
        .as_array_mut()
        .unwrap()
        .push(json!("--api-key=sk-proj-abcdefghijklmnopqrstuvwxyz0123"));
    assert_eq!(
        f.refused(ADD, leaky),
        (Some(ErrorCode::InvalidArgument), "invalid_config".into())
    );

    let added = f.ok(ADD, stdio_spec("fixture", false));
    assert_eq!(added["status"], "stopped");
    assert_eq!(added["enabled"], false);
    assert_eq!(added["source"], "drive");
    assert_eq!(added["tool_count"], 0);
    assert!(
        added["endpoint"]
            .as_str()
            .unwrap()
            .contains("mcp_server.py")
    );
    assert_eq!(
        f.refused(ADD, stdio_spec("fixture", false)),
        (Some(ErrorCode::Conflict), "conflict".into())
    );
    assert_eq!(
        f.refused(START, json!({"id": "fixture"})),
        (Some(ErrorCode::Conflict), "disabled".into())
    );
    assert_eq!(
        f.refused(START, json!({"id": "absent"})),
        (Some(ErrorCode::NotFound), "not_found".into())
    );

    let enabled = f.ok(SET_ENABLED, json!({"id": "fixture", "enabled": true}));
    assert_eq!(enabled["enabled"], true);
    let started = f.ok(START, json!({"id": "fixture"}));
    assert_eq!(started["status"], "running");
    assert_eq!(started["tool_count"], 7);
    assert_eq!(started["tools"][0]["name"], "echo");
    assert_eq!(started["server"]["name"], "fixture");
    let tools = f.ok(TOOLS, json!({"id": "fixture"}));
    assert_eq!(tools["total"], 7);
    assert_eq!(tools["truncated"], false);
    assert_eq!(
        tools["tools"][0]["input_schema"]["required"],
        json!(["text"])
    );
    assert_eq!(f.status("fixture"), "running");

    let logs = f.ok(LOGS, json!({"id": "fixture"}));
    let lines: Vec<&str> = logs["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line["text"].as_str().unwrap())
        .collect();
    assert!(lines.contains(&"running with 7 tools"), "{lines:?}");
    assert_eq!(
        f.refused(LOGS, json!({"id": "fixture", "limit": 0})).0,
        Some(ErrorCode::InvalidParams)
    );

    assert_eq!(f.ok(STOP, json!({"id": "fixture"}))["status"], "stopped");
    assert_eq!(f.ok(RESTART, json!({"id": "fixture"}))["status"], "running");
    let mut renamed = stdio_spec("fixture", true);
    renamed["name"] = json!("Renamed fixture");
    let updated = f.ok(UPDATE, renamed);
    assert_eq!(updated["name"], "Renamed fixture");
    assert_eq!(updated["status"], "stopped");
    assert_eq!(
        f.refused(UPDATE, stdio_spec("absent", true)).0,
        Some(ErrorCode::NotFound)
    );

    // `mcp.tools` starts the server on demand.
    assert_eq!(f.ok(TOOLS, json!({"id": "fixture"}))["total"], 7);
    let disabled = f.ok(SET_ENABLED, json!({"id": "fixture", "enabled": false}));
    assert_eq!(disabled["status"], "stopped");
    let events = f.event_names();
    assert!(
        events.contains(&"extension.enabled".to_owned()),
        "{events:?}"
    );
    assert!(
        events.contains(&"extension.disabled".to_owned()),
        "{events:?}"
    );
    assert!(events.contains(&"process.started".to_owned()), "{events:?}");

    // Removing a server also deletes its scratch home.
    let home = f.services.paths().tmp().join("mcp").join("fixture");
    assert!(home.is_dir());
    assert_eq!(
        f.ok(REMOVE, json!({"id": "fixture"})),
        json!({"removed": true})
    );
    assert!(!home.exists());
    assert_eq!(
        f.ok(REMOVE, json!({"id": "fixture"})),
        json!({"removed": false})
    );
    assert_eq!(f.ok(LIST, json!({}))["servers"], json!([]));
    assert_eq!(
        f.refused(LOGS, json!({"id": "fixture"})).0,
        Some(ErrorCode::NotFound)
    );
}

#[test]
fn remote_servers_pass_the_offline_switch_and_need_vault_secrets() {
    let f = fixture();
    f.ok(
        ADD,
        json!({
            "id": "remote",
            "transport": "http",
            "url": "https://mcp.example.test/mcp",
            "headers_from_secrets": [{"header": "Authorization", "secret_name": "remote-token"}],
            "enabled": true,
        }),
    );
    let listed = f.ok(LIST, json!({}));
    assert_eq!(listed["servers"][0]["endpoint"], "https://mcp.example.test");
    assert_eq!(
        listed["servers"][0]["secret_names"],
        json!(["remote-token"])
    );

    f.services.models().offline().set_offline(true).unwrap();
    assert_eq!(
        f.refused(START, json!({"id": "remote"})),
        (Some(ErrorCode::Unavailable), "offline_refused".into())
    );
    let listed = f.ok(LIST, json!({}));
    assert_eq!(listed["servers"][0]["status"], "failed");
    assert_eq!(listed["servers"][0]["error"]["reason"], "offline_refused");

    f.services.models().offline().set_offline(false).unwrap();
    assert_eq!(
        f.refused(START, json!({"id": "remote"})),
        (Some(ErrorCode::Unavailable), "needs_auth".into())
    );
    assert_eq!(f.status("remote"), "needs_auth");
    f.services
        .vault()
        .init(&SecretString::new(PASSPHRASE.into()), false)
        .unwrap();
    assert_eq!(
        f.refused(START, json!({"id": "remote"})).1,
        "needs_auth".to_owned()
    );
    f.services.vault().lock();
    assert_eq!(
        f.refused(START, json!({"id": "remote"})),
        (Some(ErrorCode::VaultLocked), "vault_locked".into())
    );
    assert_eq!(f.status("remote"), "needs_auth");
    let names = f.event_names();
    assert!(names.contains(&"policy.decision".to_owned()), "{names:?}");
}

/// The daemon's supervisor resolves imported servers' paths: a trusted
/// plugin's `${CLAUDE_PLUGIN_ROOT}` points into its installed folder, and a
/// workspace server runs in its workspace root.
#[test]
fn plugin_and_workspace_servers_start_from_their_own_folders() {
    let f = fixture();
    let source = tempfile::tempdir().unwrap();
    let plugin = source.path().join("toolkit");
    std::fs::create_dir_all(plugin.join(".claude-plugin")).unwrap();
    std::fs::create_dir_all(plugin.join("servers")).unwrap();
    std::fs::write(
        plugin.join(".claude-plugin/plugin.json"),
        json!({"name": "toolkit", "version": "1.0.0", "description": "Bundles a server."})
            .to_string(),
    )
    .unwrap();
    std::fs::copy(fixture_script(), plugin.join("servers/echo.py")).unwrap();
    std::fs::write(
        plugin.join(".mcp.json"),
        json!({"mcpServers": {"echo": {
            "command": python(),
            "args": ["-u", "${CLAUDE_PLUGIN_ROOT}/servers/echo.py", "normal"],
        }}})
        .to_string(),
    )
    .unwrap();
    let plugins = f.services.plugins();
    let installed = plugins.install(&plugin, false).unwrap();
    let hash = installed.plugin.summary.tree_sha256.clone().unwrap();
    plugins.trust("toolkit", &hash, true).unwrap();

    // Listing brings in the plugin's server, disabled; once enabled it starts
    // from the plugin's folder.
    assert_eq!(f.status("toolkit-echo"), "stopped");
    f.ok(SET_ENABLED, json!({"id": "toolkit-echo", "enabled": true}));
    let started = f.ok(START, json!({"id": "toolkit-echo"}));
    assert_eq!(started["status"], "running", "{started}");
    assert_eq!(started["tool_count"], 7);
    assert_eq!(f.ok(TOOLS, json!({"id": "toolkit-echo"}))["total"], 7);
    assert_eq!(
        f.ok(STOP, json!({"id": "toolkit-echo"}))["status"],
        "stopped"
    );
    // Its files are as trusted, so the plugin still contributes the server.
    let summary = plugins.show("toolkit").unwrap().summary;
    assert!(summary.trusted && summary.enabled, "{summary:?}");
    assert_eq!(f.status("toolkit-echo"), "stopped");

    // A bundled script changed after the review never runs: starting the
    // server re-verifies the plugin first, which withdraws its servers.
    let bundled = f.services.paths().plugins().join("toolkit/servers/echo.py");
    let mut script = std::fs::read(&bundled).unwrap();
    script.extend_from_slice(b"\n# changed after review\n");
    std::fs::write(&bundled, script).unwrap();
    assert_eq!(
        f.refused(START, json!({"id": "toolkit-echo"})),
        (Some(ErrorCode::NotFound), "not_found".into())
    );
    let summary = plugins.show("toolkit").unwrap().summary;
    assert!(!summary.trusted && !summary.enabled, "{summary:?}");

    // A project's server names its script relative to the project.
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join("scripts")).unwrap();
    std::fs::copy(fixture_script(), project.path().join("scripts/mcp.py")).unwrap();
    std::fs::write(
        project.path().join(".mcp.json"),
        json!({"mcpServers": {"local": {
            "command": python(),
            "args": ["-u", "scripts/mcp.py", "normal"],
        }}})
        .to_string(),
    )
    .unwrap();
    f.services
        .workspaces()
        .add_host(project.path(), Some("proj"))
        .unwrap();
    f.ok(IMPORT, json!({"workspace_id": "proj"}));
    let listed = f.ok(LIST, json!({"workspace_id": "proj"}));
    let id = listed["servers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|server| server["source"] == "workspace")
        .map(|server| server["id"].as_str().unwrap().to_owned())
        .unwrap();
    f.ok(SET_ENABLED, json!({"id": id, "enabled": true}));
    let started = f.ok(START, json!({"id": id}));
    assert_eq!(started["status"], "running", "{started}");
    assert_eq!(f.ok(STOP, json!({"id": id}))["status"], "stopped");
}

#[test]
fn workspace_mcp_json_files_import_disabled_servers() {
    let f = fixture();
    let project = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    std::fs::write(
        project.path().join(".mcp.json"),
        json!({"mcpServers": {
            "files": {"command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "."]},
            "remote": {"type": "http", "url": "https://mcp.example.test/mcp",
                       "headers": {"Authorization": "Bearer opaque-import-value-123"}},
            "bad": {"type": "websocket", "url": "wss://x.example.test"}
        }})
        .to_string(),
    )
    .unwrap();
    let workspace = f
        .services
        .workspaces()
        .add_host(project.path(), Some("proj"))
        .unwrap();
    f.services
        .workspaces()
        .add_host(other.path(), Some("other"))
        .unwrap();

    let preview = f.ok(IMPORT, json!({"workspace_id": "proj", "dry_run": true}));
    let statuses = |result: &Value| -> Vec<(String, String)> {
        result["servers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|server| {
                (
                    server["original_name"].as_str().unwrap().to_owned(),
                    server["status"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    };
    let mut seen = statuses(&preview);
    seen.sort();
    assert_eq!(
        seen,
        [
            ("bad".to_owned(), "invalid".to_owned()),
            ("files".to_owned(), "new".to_owned()),
            ("remote".to_owned(), "new".to_owned()),
        ]
    );
    assert_eq!(f.ok(LIST, json!({}))["servers"], json!([]));

    let imported = f.ok(IMPORT, json!({"workspace_id": "proj"}));
    assert!(!imported.to_string().contains("opaque-import-value-123"));
    assert_eq!(imported["path"], ".mcp.json");
    assert_eq!(
        statuses(&imported)
            .iter()
            .filter(|(_, status)| status == "added")
            .count(),
        2
    );
    let again = f.ok(IMPORT, json!({"workspace_id": "proj"}));
    assert_eq!(
        statuses(&again)
            .iter()
            .filter(|(_, status)| status == "exists")
            .count(),
        2
    );

    let listed = f.ok(LIST, json!({"workspace_id": "proj"}));
    let servers = listed["servers"].as_array().unwrap();
    assert_eq!(servers.len(), 2);
    for server in servers {
        assert_eq!(server["enabled"], false);
        assert_eq!(server["source"], "workspace");
        assert_eq!(server["source_owner"], workspace.id.as_str());
        assert_eq!(
            server["config"]["workspaces"],
            json!([workspace.id.as_str()])
        );
    }
    assert_eq!(
        f.ok(LIST, json!({"workspace_id": "other"}))["servers"],
        json!([])
    );
    assert_eq!(
        f.refused(IMPORT, json!({"workspace_id": "other"})).0,
        Some(ErrorCode::NotFound)
    );
    assert_eq!(
        f.refused(IMPORT, json!({"workspace_id": "proj", "extra": 1}))
            .0,
        Some(ErrorCode::InvalidParams)
    );
}

/// The vault's lock observer stops servers as the session ends, so a lock
/// followed at once by an unlock (well within one watchdog interval) still
/// stops them.
#[test]
fn a_vault_lock_stops_running_servers_before_it_returns() {
    let f = fixture();
    let vault = f.services.vault();
    vault.set_kdf_params(aidex_vault::KdfParams::new(8, 1, 1).unwrap());
    vault
        .init(&SecretString::new(PASSPHRASE.into()), false)
        .unwrap();
    f.ok(ADD, stdio_spec("fixture", true));
    assert_eq!(f.ok(START, json!({"id": "fixture"}))["status"], "running");
    assert!(vault.lock().was_unlocked);
    assert_eq!(f.status("fixture"), "stopped");
    vault.unlock(&SecretString::new(PASSPHRASE.into())).unwrap();
    assert_eq!(f.status("fixture"), "stopped");
    let logs = f.ok(LOGS, json!({"id": "fixture"}));
    let lines: Vec<&str> = logs["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line["text"].as_str().unwrap())
        .collect();
    assert!(
        lines
            .iter()
            .any(|line| line.contains("stopped because the vault was locked")),
        "{lines:?}"
    );
}

#[test]
fn vault_lock_and_emergency_stop_stop_running_servers() {
    let f = fixture();
    f.services
        .vault()
        .init(&SecretString::new(PASSPHRASE.into()), false)
        .unwrap();
    f.ok(ADD, stdio_spec("fixture", true));
    assert_eq!(f.ok(START, json!({"id": "fixture"}))["status"], "running");
    // Let the watchdog see the unlocked vault, then lock it.
    std::thread::sleep(Duration::from_millis(1200));
    f.services.vault().lock();
    f.wait_for_status("fixture", "stopped");

    // A server without secrets runs with a locked vault: the watchdog leaves
    // it alone, since the lock observer already handled that lock.
    assert_eq!(f.ok(START, json!({"id": "fixture"}))["status"], "running");
    std::thread::sleep(Duration::from_millis(1200));
    assert_eq!(f.status("fixture"), "running");
    f.services
        .broker()
        .emergency_stop("test emergency", &aidex_broker::Principal::LocalUser)
        .unwrap();
    f.wait_for_status("fixture", "stopped");
    assert_eq!(
        f.refused(START, json!({"id": "fixture"})),
        (Some(ErrorCode::PolicyDenied), "policy_denied".into())
    );
    f.services
        .broker()
        .resume_operations(&aidex_broker::Principal::LocalUser)
        .unwrap();
    assert_eq!(f.ok(START, json!({"id": "fixture"}))["status"], "running");
    assert_eq!(f.ok(STOP, json!({"id": "fixture"}))["status"], "stopped");
}
