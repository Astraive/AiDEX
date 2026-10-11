use aidex_adapters::*;
use aidex_auth::AuthService;
use aidex_core::CancelToken;
use aidex_native_sessions::*;
use aidex_sandbox::Sandbox;
use aidex_vault::{SecretString, VaultService, VaultServiceConfig};
use std::{path::PathBuf, sync::Arc, time::Duration};
fn setup(driver: Driver, scenario: &str) -> (tempfile::TempDir, RuntimeManager, RuntimeRequest) {
    let (dir, manager, request, _vault) = setup_with_vault(driver, scenario);
    (dir, manager, request)
}
fn setup_with_vault(
    driver: Driver,
    scenario: &str,
) -> (
    tempfile::TempDir,
    RuntimeManager,
    RuntimeRequest,
    Arc<VaultService>,
) {
    let dir = tempfile::tempdir().unwrap();
    let vault = Arc::new(
        VaultService::open(
            dir.path().join("vault"),
            VaultServiceConfig {
                kdf: aidex_vault::KdfParams::new(8, 1, 1).unwrap(),
                ..Default::default()
            },
        )
        .unwrap(),
    );
    vault
        .init(
            &SecretString::from("protocol-fixture-strong-password".to_owned()),
            false,
        )
        .unwrap();
    let name = aidex_vault::SecretName::new("runtime-fixture-key").unwrap();
    let consumer = aidex_vault::ConsumerScope::process("runtime-fixture").unwrap();
    vault
        .put_secret(
            &name,
            aidex_vault::SecretKind::ApiKey,
            aidex_vault::SecretBytes::from("opaqueMarkerWithoutTokenPattern_8412".to_owned()),
            aidex_vault::SecretPolicy::new([consumer]).unwrap(),
        )
        .unwrap();
    let auth = Arc::new(AuthService::new(vault.clone()).unwrap());
    let mut helper = std::env::current_exe().unwrap();
    helper.pop();
    helper.pop();
    helper.push(format!(
        "aidex-sandbox-exec{}",
        std::env::consts::EXE_SUFFIX
    ));
    let manager = RuntimeManager::open(
        dir.path().to_path_buf(),
        vault.clone(),
        auth,
        Sandbox::new(helper),
        Arc::new(|| true),
    )
    .unwrap();
    let python = std::env::var("AIDEX_TEST_PYTHON").unwrap_or_else(|_| {
        if cfg!(windows) {
            "C:/Users/gskne/AppData/Local/Programs/Python/Python312/python.exe".into()
        } else {
            "/usr/bin/python3".into()
        }
    });
    let name = serde_json::to_value(driver)
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned();
    manager
        .save(RuntimeConfig {
            id: "fixture".into(),
            driver,
            executable: python,
            argv: vec![
                "-u".into(),
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/peer.py")
                    .to_string_lossy()
                    .into_owned(),
                name,
                scenario.into(),
            ],
            profile: None,
            enabled: true,
            auth: RuntimeAuth::None,
        })
        .unwrap();
    let request = RuntimeRequest {
        session_id: "session-1".into(),
        run_id: "run-1".into(),
        parent_run_id: None,
        operation: RuntimeOperation::Start,
        mode: Mode::Agent,
        workspace: Some(RuntimeWorkspace {
            workspace_id: "fixture".into(),
            root: dir.path().to_string_lossy().into_owned(),
        }),
        permission_mode: PermissionMode::FullAccess,
        instructions: String::new(),
        model: None,
        messages: vec![aidex_providers::ChatMessage::user("hello")],
        max_steps: 16,
        max_tokens: 4096,
        runtime_ref: None,
        approvals: vec![],
        bridge: None,
    };
    (dir, manager, request, vault)
}
#[test]
fn every_named_driver_streams_real_process_protocol_and_ignores_child_completion() {
    for driver in Driver::ALL {
        let (_dir, manager, request) = setup(driver, "normal");
        let executor = manager.executors().unwrap().remove(0);
        let mut text = String::new();
        let mut tools = 0;
        let response = executor
            .execute(
                &request,
                &mut |e| {
                    if let RuntimeEvent::TextDelta { text: t } = e {
                        text.push_str(t);
                    }
                    if matches!(e, RuntimeEvent::ToolProgress { .. }) {
                        tools += 1;
                    }
                    RuntimeControl::Continue
                },
                &CancelToken::new().with_timeout(Duration::from_secs(10)),
            )
            .unwrap_or_else(|e| panic!("{driver:?}: {e}"));
        assert_eq!(response.status, RuntimeStatus::Completed);
        assert!(text.starts_with("hello"), "{driver:?}: {text}");
        assert!(!text.contains("child"));
        assert!(tools >= 2, "{driver:?}: missing real wire tool activity");
    }
}
#[test]
fn codex_approval_pauses_before_effect_and_requires_exact_live_native_request() {
    let (dir, manager, mut request) = setup(Driver::Codex, "approval");
    let executor = manager.executors().unwrap().remove(0);
    let mut approval = None;
    let paused=executor.execute(&request,&mut |e| {if let RuntimeEvent::ApprovalRequest{request_id,runtime_ref,tool_name,arguments,..}=e {let fingerprint=aidex_native_sessions::digest(serde_json::json!({"runtime":"fixture","session":request.session_id,"run":request.run_id,"workspace":request.workspace,"permission":request.permission_mode,"request":request_id,"tool":tool_name,"arguments":arguments,"reference":runtime_ref}).to_string().as_bytes());approval=Some((request_id.clone(),runtime_ref.clone(),fingerprint));return RuntimeControl::Pause;}RuntimeControl::Continue},&CancelToken::new()).unwrap();
    assert_eq!(paused.status, RuntimeStatus::WaitingApproval);
    assert!(!dir.path().join("effect.txt").exists());
    let (id, reference, fingerprint) = approval.unwrap();
    request.operation = RuntimeOperation::Approval;
    request.runtime_ref = paused.runtime_ref;
    request.approvals = vec![RuntimeApprovalDecision {
        request_id: "wrong".into(),
        approved: true,
        fingerprint: fingerprint.clone(),
        runtime_ref: reference.clone(),
    }];
    assert_eq!(
        executor
            .execute(
                &request,
                &mut |_| RuntimeControl::Continue,
                &CancelToken::new()
            )
            .unwrap_err()
            .code,
        "approval_mismatch"
    );
    assert!(!dir.path().join("effect.txt").exists());
    request.approvals[0].request_id = id;
    let done = executor
        .execute(
            &request,
            &mut |_| RuntimeControl::Continue,
            &CancelToken::new(),
        )
        .unwrap();
    assert_eq!(done.status, RuntimeStatus::Completed);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("effect.txt")).unwrap(),
        "once\n"
    );
    assert!(
        executor
            .execute(
                &request,
                &mut |_| RuntimeControl::Continue,
                &CancelToken::new()
            )
            .is_err()
    );
}
#[test]
fn malformed_truncated_bounded_frames_and_cancellation_fail_closed() {
    for scenario in ["malformed", "truncated", "oversized", "cancel"] {
        let (_dir, manager, request) = setup(Driver::Codex, scenario);
        let executor = manager.executors().unwrap().remove(0);
        let result = executor.execute(
            &request,
            &mut |_| RuntimeControl::Continue,
            &CancelToken::new().with_timeout(Duration::from_secs(1)),
        );
        assert!(result.is_err(), "{scenario}");
        assert!(manager.process_registry().is_empty());
    }
}
#[test]
fn config_is_encrypted_and_unsupported_scope_is_refused() {
    let (dir, manager, mut request) = setup(Driver::Pi, "normal");
    request.permission_mode = PermissionMode::Supervised;
    assert!(manager.executors().unwrap()[0].validate(&request).is_err());
    let private = std::fs::read(dir.path().join("private/runtime-config.sealed")).unwrap();
    assert!(!String::from_utf8_lossy(&private).contains("peer.py"));
    assert_eq!(manager.list().unwrap().len(), 1);
}

#[test]
fn encrypted_native_state_survives_restart_and_resume_does_not_repeat_initial_effect() {
    let (dir, manager, mut request) = setup(Driver::Codex, "restart");
    request.messages = vec![aidex_providers::ChatMessage::user("initial effect")];
    let executor = manager.executors().unwrap().remove(0);
    let done = executor
        .execute(
            &request,
            &mut |_| RuntimeControl::Continue,
            &CancelToken::new(),
        )
        .unwrap();
    request.runtime_ref = done.runtime_ref;
    request.operation = RuntimeOperation::Resume;
    request.run_id = "run-2".into();
    request
        .messages
        .push(aidex_providers::ChatMessage::user("follow-up"));
    let private = dir.path().join("private");
    for file in std::fs::read_dir(&private).unwrap() {
        let bytes = std::fs::read(file.unwrap().path()).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("ENCRYPTED-NATIVE-HISTORY"));
    }
    drop(executor);
    drop(manager);
    let vault = Arc::new(
        VaultService::open(dir.path().join("vault"), VaultServiceConfig::default()).unwrap(),
    );
    vault
        .unlock(&SecretString::from(
            "protocol-fixture-strong-password".to_owned(),
        ))
        .unwrap();
    let auth = Arc::new(AuthService::new(vault.clone()).unwrap());
    let manager = RuntimeManager::open(
        dir.path().to_path_buf(),
        vault,
        auth,
        Sandbox::locate(),
        Arc::new(|| true),
    )
    .unwrap();
    assert_eq!(
        manager.executors().unwrap()[0]
            .execute(
                &request,
                &mut |_| RuntimeControl::Continue,
                &CancelToken::new()
            )
            .unwrap()
            .status,
        RuntimeStatus::Completed
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("effect.txt")).unwrap(),
        "once\n"
    );
}
#[test]
fn exact_vault_secret_never_appears_in_runtime_text_or_resume_reference() {
    let (dir, manager, request) = setup(Driver::Codex, "secret");
    let _keep_dir = dir;
    let mut config = manager.get("fixture").unwrap();
    config.auth = RuntimeAuth::Vault {
        secret_name: "runtime-fixture-key".into(),
        env_name: "OPENAI_API_KEY".into(),
    };
    manager.save(config).unwrap();
    let mut text = String::new();
    let done = manager.executors().unwrap()[0]
        .execute(
            &request,
            &mut |e| {
                if let RuntimeEvent::TextDelta { text: t } = e {
                    text.push_str(t);
                }
                RuntimeControl::Continue
            },
            &CancelToken::new(),
        )
        .unwrap();
    assert!(!text.contains("opaqueMarker"));
    assert!(
        !serde_json::to_string(&done.runtime_ref)
            .unwrap()
            .contains("opaqueMarker")
    );
}

#[test]
fn rotated_vault_credential_refuses_recovery_before_starting_a_process() {
    let (dir, manager, mut request, vault) = setup_with_vault(Driver::Codex, "vault_binding");
    let mut config = manager.get("fixture").unwrap();
    config.auth = RuntimeAuth::Vault {
        secret_name: "runtime-fixture-key".into(),
        env_name: "OPENAI_API_KEY".into(),
    };
    manager.save(config).unwrap();
    let executor = manager.executors().unwrap().remove(0);
    let done = executor
        .execute(
            &request,
            &mut |_| RuntimeControl::Continue,
            &CancelToken::new(),
        )
        .unwrap();
    request.operation = RuntimeOperation::Resume;
    request.runtime_ref = done.runtime_ref;
    vault
        .put_secret(
            &aidex_vault::SecretName::new("runtime-fixture-key").unwrap(),
            aidex_vault::SecretKind::ApiKey,
            aidex_vault::SecretBytes::from("different-account-key".to_owned()),
            aidex_vault::SecretPolicy::new([aidex_vault::ConsumerScope::process(
                "runtime-fixture",
            )
            .unwrap()])
            .unwrap(),
        )
        .unwrap();
    assert_eq!(
        executor
            .execute(
                &request,
                &mut |_| RuntimeControl::Continue,
                &CancelToken::new()
            )
            .unwrap_err()
            .code,
        "resume_unavailable"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("launches.txt")).unwrap(),
        "launch\n"
    );
}

#[test]
fn shared_oauth_account_change_refuses_recovery_before_starting_a_process() {
    let (dir, manager, mut request, vault) = setup_with_vault(Driver::Codex, "oauth_binding");
    let write_identity = |subject: &str| {
        // The fixture models the authenticated encrypted auth profile, not parsed bearer-token claims.
        let profile = serde_json::json!({"client_id":"verified-client","subject":subject,"issuer":"https://auth.openai.com","email":null,"tokens":{"access_token":"fixture-access","refresh_token":null,"id_token":null,"expires_at":u64::MAX,"earliest_refresh_at":null,"scope":"resource.invoke chatgpt.tokens.use.direct"},"needs_reauth":false});
        vault
            .put_secret(
                &aidex_vault::SecretName::new("oauth.openai.profile").unwrap(),
                aidex_vault::SecretKind::Generic,
                aidex_vault::SecretBytes::new(serde_json::to_vec(&profile).unwrap()),
                aidex_vault::SecretPolicy::new([
                    aidex_vault::ConsumerScope::process("aidex-auth").unwrap()
                ])
                .unwrap(),
            )
            .unwrap();
    };
    write_identity("verified-account-a");
    let mut config = manager.get("fixture").unwrap();
    config.auth = RuntimeAuth::CodexOauth {
        provider_id: "openai".into(),
    };
    manager.save(config).unwrap();
    let executor = manager.executors().unwrap().remove(0);
    let done = executor
        .execute(
            &request,
            &mut |_| RuntimeControl::Continue,
            &CancelToken::new(),
        )
        .unwrap();
    request.operation = RuntimeOperation::Resume;
    request.runtime_ref = done.runtime_ref;
    write_identity("verified-account-b");
    assert_eq!(
        executor
            .execute(
                &request,
                &mut |_| RuntimeControl::Continue,
                &CancelToken::new()
            )
            .unwrap_err()
            .code,
        "resume_unavailable"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("launches.txt")).unwrap(),
        "launch\n"
    );
}

#[test]
fn actual_vendor_approval_scrubs_reflected_credential_keys_and_numbers() {
    let (_dir, manager, request, vault) = setup_with_vault(Driver::Codex, "reflection");
    vault
        .put_secret(
            &aidex_vault::SecretName::new("runtime-fixture-key").unwrap(),
            aidex_vault::SecretKind::ApiKey,
            aidex_vault::SecretBytes::from("48199273".to_owned()),
            aidex_vault::SecretPolicy::new([aidex_vault::ConsumerScope::process(
                "runtime-fixture",
            )
            .unwrap()])
            .unwrap(),
        )
        .unwrap();
    let mut config = manager.get("fixture").unwrap();
    config.auth = RuntimeAuth::Vault {
        secret_name: "runtime-fixture-key".into(),
        env_name: "OPENAI_API_KEY".into(),
    };
    manager.save(config).unwrap();
    let mut observed = None;
    let cancel = CancelToken::new();
    let response = manager.executors().unwrap()[0]
        .execute(
            &request,
            &mut |event| {
                if let RuntimeEvent::ApprovalRequest { arguments, .. } = event {
                    observed = Some(arguments.clone());
                    return RuntimeControl::Pause;
                }
                RuntimeControl::Continue
            },
            &cancel,
        )
        .unwrap();
    assert_eq!(response.status, RuntimeStatus::WaitingApproval);
    let arguments = observed.unwrap();
    assert!(!arguments.to_string().contains("48199273"));
    assert_eq!(arguments["[REDACTED]"], "[REDACTED]");
    cancel.cancel();
    manager.shutdown().unwrap();
}

struct BridgeFixture {
    calls: std::sync::atomic::AtomicUsize,
}
fn bridge_preflight(connection: &aidex_runtime_bridge::BridgeConnection) {
    use std::io::{Read, Write};
    let (authority, path) = connection
        .endpoint()
        .strip_prefix("http://")
        .unwrap()
        .split_once('/')
        .unwrap();
    let body=serde_json::json!({"jsonrpc":"2.0","id":"preflight","method":"initialize","params":{"protocolVersion":"2025-06-18","clientInfo":{"name":"fixture","version":"1"},"capabilities":{}}}).to_string();
    let mut stream = std::net::TcpStream::connect(authority).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let request = format!(
        "POST /{path} HTTP/1.1\r\nHost: {authority}\r\nAuthorization: {}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        connection.authorization_header(),
        body.len()
    );
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    assert!(response.starts_with(b"HTTP/1.1 200"));
}
impl aidex_runtime_bridge::BridgeDispatcher for BridgeFixture {
    fn active(&self, scope: &aidex_runtime_bridge::BridgeScope) -> bool {
        scope.session_id == "session-1" && scope.run_id == "run-1"
    }
    fn tools(
        &self,
        scope: &aidex_runtime_bridge::BridgeScope,
    ) -> Result<Vec<aidex_runtime_bridge::BridgeTool>, aidex_runtime_bridge::BridgeError> {
        assert!(self.active(scope));
        Ok(vec![aidex_runtime_bridge::BridgeTool {
            name: "fixture_echo".into(),
            description: "Fixture".into(),
            input_schema: serde_json::json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}),
            annotations: serde_json::json!({"readOnlyHint":true}),
        }])
    }
    fn call(
        &self,
        scope: &aidex_runtime_bridge::BridgeScope,
        _: &str,
        name: &str,
        args: serde_json::Value,
        cancel: &CancelToken,
    ) -> Result<serde_json::Value, aidex_runtime_bridge::BridgeError> {
        assert!(self.active(scope));
        assert_eq!(name, "fixture_echo");
        assert!(!cancel.is_stopped());
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(serde_json::json!({"content":[{"type":"text","text":args["text"]}]}))
    }
}

#[test]
fn codex_and_claude_native_configuration_reaches_the_authenticated_scoped_mcp_bridge() {
    for driver in [Driver::Codex, Driver::ClaudeCode] {
        let (_dir, manager, mut request) = setup(driver, "bridge");
        let server = aidex_runtime_bridge::BridgeServer::bind().unwrap();
        let dispatch = Arc::new(BridgeFixture {
            calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let lease = server
            .lease(
                aidex_runtime_bridge::BridgeScope {
                    session_id: request.session_id.clone(),
                    run_id: request.run_id.clone(),
                },
                dispatch.clone(),
                CancelToken::new(),
                Duration::from_secs(30),
            )
            .unwrap();
        let connection = lease.connection();
        bridge_preflight(&connection);
        let authorization = connection.authorization_header();
        let endpoint = connection.endpoint().to_owned();
        request.bridge = Some(connection);
        assert!(
            serde_json::to_value(&request)
                .unwrap()
                .get("bridge")
                .is_none()
        );
        let executor = manager.executors().unwrap().remove(0);
        assert!(executor.supports_mcp_bridge());
        let done = executor
            .execute(
                &request,
                &mut |_| RuntimeControl::Continue,
                &CancelToken::new(),
            )
            .unwrap_or_else(|error| {
                panic!(
                    "driver={driver:?}, failure={}, fixture={}",
                    error.message,
                    std::fs::read_to_string(_dir.path().join("fixture-error.txt"))
                        .unwrap_or_default()
                )
            });
        assert_eq!(done.status, RuntimeStatus::Completed);
        assert_eq!(done.message.unwrap().content, "bridge result");
        assert_eq!(dispatch.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        let reference = serde_json::to_string(&done.runtime_ref).unwrap();
        assert!(!reference.contains(&authorization));
        assert!(!reference.contains(&endpoint));
        drop(lease);
        server.shutdown();
    }
}

#[test]
fn bridge_material_copied_by_a_vendor_cannot_be_published_as_recovery_state() {
    let (_dir, manager, mut request) = setup(Driver::Codex, "bridge_persist");
    let server = aidex_runtime_bridge::BridgeServer::bind().unwrap();
    let lease = server
        .lease(
            aidex_runtime_bridge::BridgeScope {
                session_id: request.session_id.clone(),
                run_id: request.run_id.clone(),
            },
            Arc::new(BridgeFixture {
                calls: std::sync::atomic::AtomicUsize::new(0),
            }),
            CancelToken::new(),
            Duration::from_secs(30),
        )
        .unwrap();
    request.bridge = Some(lease.connection());
    bridge_preflight(request.bridge.as_ref().unwrap());
    let error = manager.executors().unwrap()[0]
        .execute(
            &request,
            &mut |_| RuntimeControl::Continue,
            &CancelToken::new(),
        )
        .unwrap_err();
    assert_eq!(
        error.code,
        "bridge_state_unsupported",
        "{}; fixture={}",
        error.message,
        std::fs::read_to_string(_dir.path().join("fixture-error.txt")).unwrap_or_default()
    );
    assert_eq!(
        std::fs::read_dir(_dir.path().join("private"))
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(
                |entry| entry.file_name().to_string_lossy().starts_with("runtime-")
                    && entry.file_name() != "runtime-config.sealed"
            )
            .count(),
        0
    );
}

#[test]
fn unsupported_native_adapter_refuses_a_bridge_before_starting_a_process() {
    let (_dir, manager, mut request) = setup(Driver::Cursor, "bridge");
    let server = aidex_runtime_bridge::BridgeServer::bind().unwrap();
    let lease = server
        .lease(
            aidex_runtime_bridge::BridgeScope {
                session_id: request.session_id.clone(),
                run_id: request.run_id.clone(),
            },
            Arc::new(BridgeFixture {
                calls: std::sync::atomic::AtomicUsize::new(0),
            }),
            CancelToken::new(),
            Duration::from_secs(30),
        )
        .unwrap();
    request.bridge = Some(lease.connection());
    let executor = manager.executors().unwrap().remove(0);
    assert!(!executor.supports_mcp_bridge());
    assert_eq!(
        executor
            .execute(
                &request,
                &mut |_| RuntimeControl::Continue,
                &CancelToken::new()
            )
            .unwrap_err()
            .code,
        "bridge_unsupported"
    );
    assert!(manager.process_registry().is_empty());
}

#[test]
fn every_acp_family_denies_exact_live_permission_before_file_effect() {
    for driver in [
        Driver::Grok,
        Driver::Opencode,
        Driver::Antigravity,
        Driver::Acp,
    ] {
        let (dir, manager, mut request) = setup(driver, "approval");
        let executor = manager.executors().unwrap().remove(0);
        let mut decision = None;
        let cancel = CancelToken::new();
        let paused=executor.execute(&request,&mut|e|{if let RuntimeEvent::ApprovalRequest{request_id,runtime_ref,tool_name,arguments,..}=e {let fingerprint=digest(serde_json::json!({"runtime":"fixture","session":request.session_id,"run":request.run_id,"workspace":request.workspace,"permission":request.permission_mode,"request":request_id,"tool":tool_name,"arguments":arguments,"reference":runtime_ref}).to_string().as_bytes());decision=Some(RuntimeApprovalDecision{request_id:request_id.clone(),approved:false,fingerprint,runtime_ref:runtime_ref.clone()});return RuntimeControl::Pause;}RuntimeControl::Continue},&cancel).unwrap();
        assert!(!dir.path().join("effect.txt").exists());
        assert_eq!(manager.remove("fixture").unwrap_err().code, "runtime_busy");
        request.operation = RuntimeOperation::Approval;
        request.runtime_ref = paused.runtime_ref;
        request.approvals = vec![decision.unwrap()];
        assert_eq!(
            executor
                .execute(&request, &mut |_| RuntimeControl::Continue, &cancel)
                .unwrap()
                .status,
            RuntimeStatus::Completed
        );
        assert!(!dir.path().join("effect.txt").exists());
    }
}
#[test]
fn cancellation_after_returning_waiting_approval_stops_native_tree() {
    let (dir, manager, request) = setup(Driver::Codex, "approval");
    let executor = manager.executors().unwrap().remove(0);
    let cancel = CancelToken::new();
    assert_eq!(
        executor
            .execute(
                &request,
                &mut |event| if matches!(event, RuntimeEvent::ApprovalRequest { .. }) {
                    RuntimeControl::Pause
                } else {
                    RuntimeControl::Continue
                },
                &cancel
            )
            .unwrap()
            .status,
        RuntimeStatus::WaitingApproval
    );
    cancel.cancel();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !manager.process_registry().is_empty() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(!dir.path().join("effect.txt").exists());
}
#[test]
fn auth_required_is_an_error_and_health_never_treats_binary_presence_as_login() {
    let (_dir, manager, request) = setup(Driver::Codex, "auth");
    assert_eq!(
        manager.executors().unwrap()[0]
            .execute(
                &request,
                &mut |_| RuntimeControl::Continue,
                &CancelToken::new()
            )
            .unwrap_err()
            .code,
        "auth_required"
    );
    let status = manager.health("fixture").unwrap();
    assert!(status.installed);
    assert!(!status.authenticated);
}

#[test]
fn pi_and_omp_models_round_trip_provider_qualified_identity() {
    for driver in [Driver::Pi, Driver::Omp] {
        let (_dir, manager, mut request) = setup(driver, "model");
        let models = manager.models("fixture").unwrap();
        assert_eq!(models[0].id, "fixture-provider/fixture");
        request.model = Some(models[0].id.clone());
        assert_eq!(
            manager.executors().unwrap()[0]
                .execute(
                    &request,
                    &mut |_| RuntimeControl::Continue,
                    &CancelToken::new()
                )
                .unwrap()
                .status,
            RuntimeStatus::Completed
        );
    }
}

#[test]
fn official_login_encrypts_credentials_and_cancel_cleans_the_owned_tree() {
    for pending in [false, true] {
        let (dir, manager, _) = setup(Driver::Codex, "normal");
        let mut config = manager.get("fixture").unwrap();
        config.argv = vec![
            "-u".into(),
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/account_peer.py")
                .to_string_lossy()
                .into_owned(),
        ];
        if pending {
            config.argv.push("pending".into());
        }
        manager.save(config).unwrap();
        let flow = manager.login_start("fixture").unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let final_status = loop {
            let status = manager.login_status("fixture", &flow.flow_id).unwrap();
            if pending && status.authorization_url.is_some() {
                break manager.login_cancel("fixture", &flow.flow_id).unwrap();
            }
            if status.state != "pending" {
                break status;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(
            final_status.state,
            if pending { "cancelled" } else { "connected" }
        );
        assert!(manager.process_registry().is_empty());
        assert!(
            !serde_json::to_string(&final_status)
                .unwrap()
                .contains("arbitrary-secret")
        );
        let status = manager.account_status("fixture").unwrap();
        assert_eq!(status.authenticated, !pending);
        for entry in std::fs::read_dir(dir.path().join("private")).unwrap() {
            assert!(
                !String::from_utf8_lossy(&std::fs::read(entry.unwrap().path()).unwrap())
                    .contains("fixture-opaque-secret")
            );
        }
        manager.logout("fixture").unwrap();
        assert!(!manager.account_status("fixture").unwrap().authenticated);
    }
}

#[test]
fn explicitly_selected_native_credential_import_is_validated_and_encrypts_only_auth_file() {
    let (dir, manager, _) = setup(Driver::Codex, "normal");
    let mut config = manager.get("fixture").unwrap();
    config.argv = vec![
        "-u".into(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/account_peer.py")
            .to_string_lossy()
            .into_owned(),
    ];
    manager.save(config).unwrap();
    assert!(!manager.account_status("fixture").unwrap().authenticated);
    assert!(
        manager
            .import_profile("fixture", PathBuf::from("relative/auth.json").as_path())
            .is_err()
    );
    assert!(
        manager.get("fixture").unwrap().profile.is_none(),
        "Failed import must preserve the configured account mode"
    );
    let source = dir.path().join("selected-profile");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(
        source.join("auth.json"),
        br#"{"tokens":{"access_token":"explicit-import-secret"}}"#,
    )
    .unwrap();
    std::fs::write(source.join("config.toml"), "untrusted_config_not_imported").unwrap();
    assert!(
        manager
            .import_profile("fixture", &source)
            .unwrap()
            .authenticated
    );
    let status = manager.account_status("fixture").unwrap();
    assert!(status.authenticated && status.credential_present);
    assert_eq!(status.evidence, "native_credential_and_cli_exit_status");
    assert!(source.join("auth.json").is_file());
    for entry in std::fs::read_dir(dir.path().join("private")).unwrap() {
        let bytes = std::fs::read(entry.unwrap().path()).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("explicit-import-secret"));
    }
    manager.logout("fixture").unwrap();
    assert!(!manager.account_status("fixture").unwrap().authenticated);
    assert!(
        manager
            .import_profile("fixture", PathBuf::from("relative/auth.json").as_path())
            .is_err()
    );
}

#[test]
fn first_turn_native_checkpoint_survives_incomplete_response_without_replaying_effect() {
    let (dir, manager, mut request) = setup(Driver::Codex, "restart_cut");
    request.messages = vec![aidex_providers::ChatMessage::user("initial effect")];
    let executor = manager.executors().unwrap().remove(0);
    let mut checkpoint = None;
    assert!(
        executor
            .execute(
                &request,
                &mut |e| {
                    if let RuntimeEvent::SessionStarted { runtime_ref } = e {
                        checkpoint = Some(runtime_ref.clone());
                    }
                    RuntimeControl::Continue
                },
                &CancelToken::new()
            )
            .is_err()
    );
    request.runtime_ref =
        Some(checkpoint.expect("native identity must be checkpointed before turn effects"));
    request.operation = RuntimeOperation::Resume;
    request.run_id = "run-2".into();
    request
        .messages
        .push(aidex_providers::ChatMessage::user("follow-up"));
    assert_eq!(
        executor
            .execute(
                &request,
                &mut |_| RuntimeControl::Continue,
                &CancelToken::new()
            )
            .unwrap()
            .status,
        RuntimeStatus::Completed
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("effect.txt")).unwrap(),
        "once\n"
    );
}

#[test]
fn resuming_same_interrupted_run_inspects_evidence_without_reissuing_original_effect() {
    let (dir, manager, mut request) = setup(Driver::Codex, "restart_cut");
    request.messages = vec![aidex_providers::ChatMessage::user("initial effect")];
    let executor = manager.executors().unwrap().remove(0);
    let mut checkpoint = None;
    assert!(
        executor
            .execute(
                &request,
                &mut |event| {
                    if let RuntimeEvent::SessionStarted { runtime_ref } = event {
                        checkpoint = Some(runtime_ref.clone());
                    }
                    RuntimeControl::Continue
                },
                &CancelToken::new()
            )
            .is_err()
    );
    request.operation = RuntimeOperation::Resume;
    request.runtime_ref = checkpoint;
    let done = executor
        .execute(
            &request,
            &mut |_| RuntimeControl::Continue,
            &CancelToken::new(),
        )
        .unwrap();
    assert_eq!(done.status, RuntimeStatus::Completed);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("effect.txt")).unwrap(),
        "once\n"
    );
}

#[test]
fn missing_or_changed_native_snapshot_evidence_refuses_resume() {
    let (dir, manager, mut request) = setup(Driver::Codex, "restart");
    let executor = manager.executors().unwrap().remove(0);
    request.runtime_ref = executor
        .execute(
            &request,
            &mut |_| RuntimeControl::Continue,
            &CancelToken::new(),
        )
        .unwrap()
        .runtime_ref;
    request.operation = RuntimeOperation::Resume;
    let name = request.runtime_ref.as_ref().unwrap()["snapshot_id"]
        .as_str()
        .unwrap();
    std::fs::remove_file(dir.path().join("private").join(name)).unwrap();
    assert_eq!(
        executor
            .execute(
                &request,
                &mut |_| RuntimeControl::Continue,
                &CancelToken::new()
            )
            .unwrap_err()
            .code,
        "resume_unavailable"
    );
}

#[test]
fn imported_native_account_secret_is_scrubbed_from_runtime_protocol() {
    let (dir, manager, request) = setup(Driver::Codex, "native_secret");
    let mut config = manager.get("fixture").unwrap();
    let protocol_argv = config.argv.clone();
    config.argv = vec![
        "-u".into(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/account_peer.py")
            .to_string_lossy()
            .into_owned(),
    ];
    manager.save(config).unwrap();
    let source = dir.path().join("explicit-auth.json");
    std::fs::write(
        &source,
        br#"{"tokens":{"access_token":"OpaqueNativeAccountSecret_5159"}}"#,
    )
    .unwrap();
    manager.import_profile("fixture", &source).unwrap();
    let mut config = manager.get("fixture").unwrap();
    config.argv = protocol_argv;
    manager.save(config).unwrap();
    let mut observed = String::new();
    let result = manager.executors().unwrap()[0]
        .execute(
            &request,
            &mut |e| {
                observed.push_str(&serde_json::to_string(e).unwrap());
                RuntimeControl::Continue
            },
            &CancelToken::new(),
        )
        .unwrap();
    assert!(!observed.contains("OpaqueNativeAccountSecret"));
    assert!(
        !serde_json::to_string(&result)
            .unwrap()
            .contains("OpaqueNativeAccountSecret")
    );
}

#[test]
fn runtime_factory_migrates_only_owned_ciphertext_and_uses_declared_private_directory() {
    let (dir, manager, _) = setup(Driver::Codex, "normal");
    let old = std::fs::read(dir.path().join("private/runtime-config.sealed")).unwrap();
    std::fs::write(
        dir.path().join("private/unrelated-vault.sealed"),
        b"unrelated",
    )
    .unwrap();
    drop(manager);
    let vault = Arc::new(
        VaultService::open(dir.path().join("vault"), VaultServiceConfig::default()).unwrap(),
    );
    vault
        .unlock(&SecretString::from(
            "protocol-fixture-strong-password".to_owned(),
        ))
        .unwrap();
    let auth = Arc::new(AuthService::new(vault.clone()).unwrap());
    let private = dir.path().join("config/external-runtimes");
    let manager = RuntimeManager::open_with_private(
        dir.path().to_path_buf(),
        private.clone(),
        vault,
        auth,
        Sandbox::locate(),
        Arc::new(|| true),
    )
    .unwrap();
    assert_eq!(manager.get("fixture").unwrap().driver, Driver::Codex);
    assert_eq!(
        std::fs::read(private.join("runtime-config.sealed")).unwrap(),
        old
    );
    assert!(!private.join("unrelated-vault.sealed").exists());
    assert!(manager.models("fixture").is_ok());
}

#[test]
fn changing_native_account_refuses_old_session_identity_without_replaying_effects() {
    let (dir, manager, mut request) = setup(Driver::Codex, "restart");
    let mut config = manager.get("fixture").unwrap();
    let protocol_argv = config.argv.clone();
    let account_argv = vec![
        "-u".into(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/account_peer.py")
            .to_string_lossy()
            .into_owned(),
    ];
    config.argv = account_argv.clone();
    manager.save(config).unwrap();
    let source = dir.path().join("explicit-auth.json");
    std::fs::write(
        &source,
        br#"{"tokens":{"access_token":"OpaqueAccountA_1064"}}"#,
    )
    .unwrap();
    manager.import_profile("fixture", &source).unwrap();
    let mut config = manager.get("fixture").unwrap();
    config.argv = protocol_argv.clone();
    manager.save(config).unwrap();
    request.messages = vec![aidex_providers::ChatMessage::user("initial effect")];
    let executor = manager.executors().unwrap().remove(0);
    let initial = executor
        .execute(
            &request,
            &mut |_| RuntimeControl::Continue,
            &CancelToken::new(),
        )
        .unwrap();
    request.runtime_ref = initial.runtime_ref;
    assert!(request.runtime_ref.as_ref().unwrap()["account_revision"].is_string());
    let mut config = manager.get("fixture").unwrap();
    config.argv = account_argv;
    manager.save(config).unwrap();
    std::fs::write(
        &source,
        br#"{"tokens":{"access_token":"OpaqueAccountB_2085"}}"#,
    )
    .unwrap();
    manager.import_profile("fixture", &source).unwrap();
    let mut config = manager.get("fixture").unwrap();
    config.argv = protocol_argv;
    manager.save(config).unwrap();
    request.operation = RuntimeOperation::Resume;
    request.run_id = "run-2".into();
    request
        .messages
        .push(aidex_providers::ChatMessage::user("follow-up"));
    assert_eq!(
        executor
            .execute(
                &request,
                &mut |_| RuntimeControl::Continue,
                &CancelToken::new()
            )
            .unwrap_err()
            .code,
        "resume_unavailable"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("effect.txt")).unwrap(),
        "once\n"
    );
}

#[test]
fn shutdown_cancels_pending_official_login_and_refuses_native_turns_during_login() {
    let (_dir, manager, request) = setup(Driver::Codex, "normal");
    let mut config = manager.get("fixture").unwrap();
    config.argv = vec![
        "-u".into(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/account_peer.py")
            .to_string_lossy()
            .into_owned(),
        "pending".into(),
    ];
    manager.save(config).unwrap();
    let flow = manager.login_start("fixture").unwrap();
    assert_eq!(
        manager.executors().unwrap()[0]
            .validate(&request)
            .unwrap_err()
            .code,
        "auth_pending"
    );
    assert!(!manager.account_status("fixture").unwrap().authenticated);
    manager.shutdown().unwrap();
    assert_eq!(
        manager
            .login_status("fixture", &flow.flow_id)
            .unwrap()
            .state,
        "cancelled"
    );
    assert!(manager.process_registry().is_empty());
    assert!(!manager.process_registry().is_accepting());
}

#[test]
fn root_and_child_native_branches_have_independent_encrypted_resume_snapshots() {
    let (dir, manager, mut root) = setup(Driver::Codex, "restart");
    root.messages = vec![aidex_providers::ChatMessage::user("initial effect")];
    let executor = manager.executors().unwrap().remove(0);
    let root_result = executor
        .execute(
            &root,
            &mut |_| RuntimeControl::Continue,
            &CancelToken::new(),
        )
        .unwrap();
    let mut child = root.clone();
    child.run_id = "child-1".into();
    child.parent_run_id = Some(root.run_id.clone());
    let child_result = executor
        .execute(
            &child,
            &mut |_| RuntimeControl::Continue,
            &CancelToken::new(),
        )
        .unwrap();
    let root_ref = root_result.runtime_ref.unwrap();
    let child_ref = child_result.runtime_ref.unwrap();
    assert_ne!(root_ref["snapshot_id"], child_ref["snapshot_id"]);
    for (mut request, reference) in [(root, root_ref), (child, child_ref)] {
        request.operation = RuntimeOperation::Resume;
        request.runtime_ref = Some(reference);
        request.run_id.push_str("-next");
        request
            .messages
            .push(aidex_providers::ChatMessage::user("follow-up"));
        assert_eq!(
            executor
                .execute(
                    &request,
                    &mut |_| RuntimeControl::Continue,
                    &CancelToken::new()
                )
                .unwrap()
                .status,
            RuntimeStatus::Completed
        );
    }
    assert_eq!(
        std::fs::read_to_string(dir.path().join("effect.txt")).unwrap(),
        "once\nonce\n"
    );
}
#[test]
fn installer_refuses_missing_manifest_and_hash_mismatch_then_copies_verified_component() {
    let (dir, manager, _) = setup(Driver::Codex, "normal");
    assert!(manager.install(Driver::Codex, None).is_err());
    std::fs::write(
        dir.path().join("standalone.exe"),
        b"verified package component",
    )
    .unwrap();
    let manifest = |hash: &str| {
        serde_json::json!({"components":[{"driver":"codex","path":"standalone.exe","sha256":hash}]})
            .to_string()
    };
    std::fs::write(
        dir.path().join("runtime-components.json"),
        manifest("wrong"),
    )
    .unwrap();
    assert_eq!(
        manager.install(Driver::Codex, None).unwrap_err().code,
        "integrity_failed"
    );
    std::fs::write(
        dir.path().join("runtime-components.json"),
        manifest(&digest(b"verified package component")),
    )
    .unwrap();
    let installed = manager.install(Driver::Codex, None).unwrap();
    assert_eq!(
        std::fs::read(dir.path().join(installed.executable)).unwrap(),
        b"verified package component"
    );
}
