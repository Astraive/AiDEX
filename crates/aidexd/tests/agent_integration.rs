//! Exercise composed RPC services, rather than registering isolated handlers.
mod common;
use aidex_vault::{KdfParams, SecretBytes, SecretKind, SecretName, SecretPolicy};
use aidexd::{Client, ErrorCode};
use serde_json::json;
#[cfg(windows)]
use std::time::{Duration, Instant};

fn unlocked(root: &std::path::Path) -> aidexd::DaemonHandle {
    common::start(root, common::options(), |d| {
        d.services()
            .vault()
            .set_kdf_params(KdfParams::new(8, 1, 1).unwrap());
    })
}
fn init(c: &mut Client) {
    c.call(
        "vault.init",
        json!({"passphrase":"fixture passphrase is never a real credential"}),
    )
    .unwrap();
}

#[test]
fn composed_extensions_and_runtime_registry_survive_reopening() {
    let root = common::temp_drive();
    let project = tempfile::tempdir().unwrap();
    let d = unlocked(root.path());
    let mut c = Client::connect(root.path(), "agent-integration").unwrap();
    init(&mut c);
    let workspace = c
        .call(
            "workspace.add",
            json!({"path":project.path(),"name":"fixture"}),
        )
        .unwrap();
    let runtime_catalog = c.call("runtime.catalog", json!({})).unwrap();
    assert_eq!(runtime_catalog.as_array().unwrap().len(), 10);
    let opaque = "opaque-credential-invalid-params-fixture";
    let error = c
        .call(
            "web.configure",
            json!({"provider":"brave","enabled":opaque,"api_key":"another-synthetic-key"}),
        )
        .unwrap_err();
    assert!(!error.to_string().contains(opaque));
    assert!(!error.to_string().contains("another-synthetic-key"));
    c.call(
        "runtime.add",
        json!({"id":"fixture-runtime","driver":"codex","executable":"codex.exe"}),
    )
    .unwrap();
    let catalog = c.call("extensions.marketplace.list", json!({})).unwrap();
    assert_eq!(catalog["schema_version"], 1);
    let entry = &catalog["entries"][0];
    c.call("extensions.marketplace.install",json!({"id":entry["id"],"version":entry["version"],"package_digest":entry["package_digest"],"trusted":true,"workspace_ids":[workspace["id"]],"profiles":["supervised"]})).unwrap();
    let skills = c.call("extensions.skills.list", json!({})).unwrap();
    assert!(!skills.as_array().unwrap().is_empty());
    c.call(
        "web.configure",
        json!({"provider":"brave","enabled":true,"api_key":"opaque-synthetic-search-fixture"}),
    )
    .unwrap();
    let web = c.call("web.status", json!({})).unwrap();
    assert_eq!(web["credential_present"], true);
    drop(c);
    d.stop();
    d.wait().unwrap();

    let d = unlocked(root.path());
    let mut c = Client::connect(root.path(), "agent-integration").unwrap();
    c.call(
        "vault.unlock",
        json!({"passphrase":"fixture passphrase is never a real credential"}),
    )
    .unwrap();
    let status = c.call("execution.status", json!({})).unwrap();
    assert_eq!(
        status["runtime_descriptors"][0]["id"], "fixture-runtime",
        "restoration must require no unrelated mutation or message"
    );
    assert_eq!(
        c.call("extensions.skills.list", json!({})).unwrap()[0]["id"],
        skills[0]["id"]
    );
    assert_eq!(
        c.call("web.status", json!({})).unwrap()["settings"]["enabled"],
        true
    );
    d.stop();
    d.wait().unwrap();
}

#[test]
fn generic_vault_edits_protect_system_records_and_audit_only_opaque_identity() {
    let root = common::temp_drive();
    let mut vault = None;
    let d = common::start(root.path(), common::options(), |d| {
        let v = d.services().vault().clone();
        v.set_kdf_params(KdfParams::new(8, 1, 1).unwrap());
        vault = Some(v);
    });
    let vault = vault.unwrap();
    let mut c = Client::connect(root.path(), "agent-vault").unwrap();
    init(&mut c);
    for name in [
        "native-record-encryption-key",
        "external-runtime-encryption-key",
        "aidex-preferences",
        "oauth.fixture.profile",
    ] {
        vault
            .put_secret(
                &SecretName::new(name).unwrap(),
                SecretKind::Generic,
                SecretBytes::from("system synthetic record".to_owned()),
                SecretPolicy::default(),
            )
            .unwrap();
        for (method, params) in [
            (
                "vault.secret.set",
                json!({"name":name,"value":"replacement"}),
            ),
            ("vault.secret.delete", json!({"name":name})),
        ] {
            assert_eq!(
                c.call(method, params).unwrap_err().rpc_kind(),
                Some(ErrorCode::PolicyDenied)
            );
        }
    }
    let secret = "opaque-user-credential-fixture";
    let meta = c
        .call(
            "vault.secret.set",
            json!({"name":"user-key-name","value":secret,"consumers":["mcp:fixture"]}),
        )
        .unwrap();
    c.call("vault.secret.delete", json!({"name":"user-key-name"}))
        .unwrap();
    let page = c
        .call("events.replay", json!({"after":0,"limit":100}))
        .unwrap();
    let wire = page.to_string();
    assert!(!wire.contains(secret));
    assert!(!wire.contains("user-key-name"));
    for event_type in ["secret.stored", "secret.deleted"] {
        let events = page["events"].as_array().unwrap();
        let event = events
            .iter()
            .find(|e| e["event"]["type"] == event_type)
            .unwrap();
        assert_eq!(event["event"]["data"]["secret_ref"], meta["secret_ref"]);
    }
    d.stop();
    d.wait().unwrap();
}

#[test]
fn unlock_reconciles_an_orphaned_wait_without_starting_a_new_actor() {
    let root = common::temp_drive();
    let project = tempfile::tempdir().unwrap();
    let mut services = None;
    let d = common::start(root.path(), common::options(), |d| {
        d.services()
            .vault()
            .set_kdf_params(KdfParams::new(8, 1, 1).unwrap());
        services = Some(d.services().clone());
    });
    let services = services.unwrap();
    let mut c = Client::connect(root.path(), "orphan-recovery").unwrap();
    init(&mut c);
    let workspace = c
        .call("workspace.add", json!({"path":project.path()}))
        .unwrap();
    let session=c.call("conversation.create",json!({"mode":"agent","runtime_id":"native","workspace_id":workspace["id"],"provider_id":"unconfigured","model_id":"fixture"})).unwrap();
    let session = session["id"].as_str().unwrap();
    let run = services
        .native()
        .store()
        .begin(
            session,
            "inspect before performing any effect",
            "orphan-fixture",
            None,
        )
        .unwrap();
    services.native().store().update(session,|s|{s.runs[0].status=aidex_native_sessions::RunStatus::WaitingApproval;s.approvals.push(serde_json::from_value(json!({"id":"approval_orphan","run_id":run.id,"tool_name":"web_search","arguments":{},"fingerprint":"opaque-orphan-plan","status":"pending","bridge_call_id":"bridge_orphan"})).unwrap());Ok(())}).unwrap();
    services.vault().lock();
    c.call(
        "vault.unlock",
        json!({"passphrase":"fixture passphrase is never a real credential"}),
    )
    .unwrap();
    let restored = c.call("run.get", json!({"id":run.id})).unwrap();
    assert_eq!(restored["status"], "interrupted");
    let approvals = c.call("approval.list", json!({"run_id":run.id})).unwrap();
    assert_eq!(approvals["approvals"][0]["status"], "expired");
    assert!(services.native().runtime_descriptors().is_empty());
    assert!(!project.path().join("effect.txt").exists());
    d.stop();
    d.wait().unwrap();
}

#[cfg(windows)]
#[test]
fn pending_mcp_connect_does_not_block_cancel_or_offline_rpc() {
    let root = common::temp_drive();
    let project = tempfile::tempdir().unwrap();
    let d = unlocked(root.path());
    let mut c = Client::connect(root.path(), "agent-mcp").unwrap();
    init(&mut c);
    let workspace = c
        .call("workspace.add", json!({"path":project.path()}))
        .unwrap();
    c.call("extensions.mcp.upsert",json!({"config":{"id":"slow-fixture","name":"Slow fixture","enabled":true,"trusted":true,"workspace_ids":[workspace["id"]],"profiles":["full_access"],"transport":{"kind":"stdio","command":"powershell.exe","args":["-NoLogo","-NoProfile","-NonInteractive","-Command","while($null -ne [Console]::In.ReadLine()){Start-Sleep -Seconds 30}" ]},"timeout_ms":30000,"max_output_bytes":65536}})).unwrap();
    let before = Instant::now();
    let setup=c.call("extensions.mcp.connect",json!({"id":"slow-fixture","workspace_id":workspace["id"],"profile":"full_access","full_access":true})).unwrap();
    assert_eq!(setup["connecting"], true);
    assert!(
        before.elapsed() < Duration::from_secs(2),
        "setup must return before the fixture responds"
    );
    c.call("network.offline", json!({"enabled":true})).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let servers = c.call("extensions.mcp.list", json!({})).unwrap();
        if servers[0]["connecting"] == false {
            assert_eq!(servers[0]["connected"], false);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "offline entry must cancel pending setup"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        c.call("web.status", json!({})).unwrap()["network_allowed"],
        false
    );
    d.stop();
    d.wait().unwrap();
}

#[cfg(windows)]
#[test]
fn idle_vault_lock_disconnects_an_already_connected_stdio_server() {
    let root = common::temp_drive();
    let project = tempfile::tempdir().unwrap();
    let mut vault = None;
    let d = common::start(root.path(), common::options(), |d| {
        let v = d.services().vault().clone();
        v.set_kdf_params(KdfParams::new(8, 1, 1).unwrap());
        v.set_idle_timeout(None);
        vault = Some(v);
    });
    let vault = vault.unwrap();
    let mut c = Client::connect(root.path(), "idle-mcp").unwrap();
    init(&mut c);
    let workspace = c
        .call("workspace.add", json!({"path":project.path()}))
        .unwrap();
    let script = r#"while($null -ne ($line=[Console]::In.ReadLine())){$m=$line|ConvertFrom-Json;if($null -ne $m.id){if($m.method -eq 'initialize'){$r=@{protocolVersion='2025-06-18';capabilities=@{tools=@{}};serverInfo=@{name='fixture';version='1'}}}else{$r=@{tools=@()}};[Console]::Out.WriteLine((@{jsonrpc='2.0';id=$m.id;result=$r}|ConvertTo-Json -Compress -Depth 15));[Console]::Out.Flush()}}"#;
    c.call("extensions.mcp.upsert",json!({"config":{"id":"idle-fixture","name":"Idle fixture","enabled":true,"trusted":true,"workspace_ids":[workspace["id"]],"profiles":["full_access"],"transport":{"kind":"stdio","command":"powershell.exe","args":["-NoLogo","-NoProfile","-NonInteractive","-Command",script]},"timeout_ms":10000,"max_output_bytes":65536}})).unwrap();
    c.call("extensions.mcp.connect",json!({"id":"idle-fixture","workspace_id":workspace["id"],"profile":"full_access","full_access":true})).unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let servers = c.call("extensions.mcp.list", json!({})).unwrap();
        if servers[0]["connected"] == true {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "stdio initialization should finish: {servers}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    // Ensure the guard observed the unlocked state before triggering an idle lock.
    std::thread::sleep(Duration::from_millis(150));
    vault.set_idle_timeout(Some(Duration::from_millis(10)));
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let servers = c.call("extensions.mcp.list", json!({})).unwrap();
        if servers[0]["connected"] == false {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "idle lock must terminate cached credential consumers"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(vault.status().state, aidex_vault::VaultState::Locked);
    d.stop();
    d.wait().unwrap();
}
