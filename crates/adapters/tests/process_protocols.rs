use aidex_adapters::*;
use aidex_auth::AuthService;
use aidex_core::CancelToken;
use aidex_native_sessions::*;
use aidex_sandbox::Sandbox;
use aidex_vault::{SecretString, VaultService, VaultServiceConfig};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
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
    setup_with_gate(driver, scenario, Arc::new(AtomicBool::new(true)))
}
fn setup_with_gate(
    driver: Driver,
    scenario: &str,
    gate: Arc<AtomicBool>,
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
    let manager = RuntimeManager::open(
        dir.path().to_path_buf(),
        vault.clone(),
        auth,
        Sandbox::new(helper()),
        Arc::new(move || gate.load(Ordering::SeqCst)),
    )
    .unwrap();
    manager
        .save(RuntimeConfig {
            id: "fixture".into(),
            driver,
            executable: python(),
            argv: peer_argv(driver, scenario),
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
    };
    (dir, manager, request, vault)
}
fn helper() -> PathBuf {
    let mut helper = std::env::current_exe().unwrap();
    helper.pop();
    helper.pop();
    helper.push(format!(
        "aidex-sandbox-exec{}",
        std::env::consts::EXE_SUFFIX
    ));
    helper
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
fn fixture(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(name)
        .to_string_lossy()
        .into_owned()
}
fn peer_argv(driver: Driver, scenario: &str) -> Vec<String> {
    let name = serde_json::to_value(driver)
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned();
    vec!["-u".into(), fixture("peer.py"), name, scenario.into()]
}
fn account_argv(extra: &[&str]) -> Vec<String> {
    let mut argv = vec!["-u".into(), fixture("account_peer.py")];
    argv.extend(extra.iter().map(|s| s.to_string()));
    argv
}
fn set_argv(manager: &RuntimeManager, argv: Vec<String>) {
    let mut config = manager.get("fixture").unwrap();
    config.argv = argv;
    manager.save(config).unwrap();
}
fn launches(dir: &Path) -> usize {
    std::fs::read_to_string(dir.join("launches.txt"))
        .unwrap_or_default()
        .lines()
        .count()
}
fn runtime_snapshots(dir: &Path) -> Vec<String> {
    let prefix = format!("runtime-{}-", digest(b"fixture"));
    std::fs::read_dir(dir.join("private"))
        .unwrap()
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.starts_with(&prefix))
        .collect()
}
fn continue_all(_: &RuntimeEvent) -> RuntimeControl {
    RuntimeControl::Continue
}
fn pause_on_approval(event: &RuntimeEvent) -> RuntimeControl {
    if matches!(event, RuntimeEvent::ApprovalRequest { .. }) {
        RuntimeControl::Pause
    } else {
        RuntimeControl::Continue
    }
}
fn fingerprint(request: &RuntimeRequest, event: &RuntimeEvent) -> Option<RuntimeApprovalDecision> {
    let RuntimeEvent::ApprovalRequest {
        request_id,
        runtime_ref,
        tool_name,
        arguments,
        ..
    } = event
    else {
        return None;
    };
    let fingerprint = digest(serde_json::json!({"runtime":"fixture","session":request.session_id,"run":request.run_id,"workspace":request.workspace,"permission":request.permission_mode,"request":request_id,"tool":tool_name,"arguments":arguments,"reference":runtime_ref}).to_string().as_bytes());
    Some(RuntimeApprovalDecision {
        request_id: request_id.clone(),
        approved: true,
        fingerprint,
        runtime_ref: runtime_ref.clone(),
    })
}
fn wait_for_login(manager: &RuntimeManager, flow: &str) -> RuntimeLoginStatus {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let status = manager.login_status("fixture", flow).unwrap();
        if status.state != "pending" {
            return status;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn wait_for_authorization(manager: &RuntimeManager, flow: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while manager
        .login_status("fixture", flow)
        .unwrap()
        .authorization_url
        .is_none()
    {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
}
/// The harness keeps a reference only when `redact_text` leaves it unchanged
/// (`safe_reference`); a changed one fails the turn at its first checkpoint.
fn assert_redaction_stable(reference: &serde_json::Value, context: &str) {
    let text = reference.to_string();
    assert_eq!(aidex_events::redact_text(&text), text, "{context}");
}
/// The verified shared OAuth profile for `subject`, as `AuthService` stores it:
/// an authenticated encrypted profile, never parsed bearer-token claims.
fn oauth_identity(vault: &VaultService, subject: &str) {
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
}
fn import(manager: &RuntimeManager, dir: &Path, credential: &str) {
    let argv = manager.get("fixture").unwrap().argv;
    set_argv(manager, account_argv(&[]));
    let source = dir.join("explicit-auth.json");
    std::fs::write(
        &source,
        serde_json::json!({"tokens":{"access_token":credential}}).to_string(),
    )
    .unwrap();
    manager.import_profile("fixture", &source).unwrap();
    set_argv(manager, argv);
}
#[test]
fn every_named_driver_streams_real_process_protocol_and_ignores_child_completion() {
    for driver in Driver::ALL {
        let (_dir, manager, request) = setup(driver, "normal");
        let executor = manager.executors().unwrap().remove(0);
        let mut text = String::new();
        let mut tools = 0;
        let mut references = vec![];
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
                    if let RuntimeEvent::SessionStarted { runtime_ref } = e {
                        references.push(runtime_ref.clone());
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
        references.extend(response.runtime_ref);
        for reference in &references {
            assert_redaction_stable(reference, &format!("{driver:?}"));
        }
    }
}
#[test]
fn protocol_peers_accept_equivalent_workspace_directory_paths() {
    for driver in [Driver::Codex, Driver::Acp] {
        let (dir, manager, mut request) = setup(driver, "normal");
        // Sandbox negotiation canonicalizes cwd. The wire request can still use
        // an equivalent spelling (including Windows short-name temp paths).
        request.workspace.as_mut().unwrap().root =
            dir.path().join(".").to_string_lossy().into_owned();
        let response = manager.executors().unwrap()[0]
            .execute(
                &request,
                &mut |_| RuntimeControl::Continue,
                &CancelToken::new().with_timeout(Duration::from_secs(10)),
            )
            .unwrap_or_else(|error| panic!("{driver:?}: {error}"));
        assert_eq!(response.status, RuntimeStatus::Completed);
    }
}
#[test]
fn codex_approval_pauses_before_effect_and_requires_exact_live_native_request() {
    let (dir, manager, mut request) = setup(Driver::Codex, "approval");
    let executor = manager.executors().unwrap().remove(0);
    let mut approval = None;
    let paused = executor
        .execute(
            &request,
            &mut |e| {
                if let Some(decision) = fingerprint(&request, e) {
                    approval = Some(decision);
                    return RuntimeControl::Pause;
                }
                RuntimeControl::Continue
            },
            &CancelToken::new(),
        )
        .unwrap();
    assert_eq!(paused.status, RuntimeStatus::WaitingApproval);
    assert!(!dir.path().join("effect.txt").exists());
    let approval = approval.unwrap();
    // The paused reference is resumable: a snapshot was saved at the request.
    assert!(is_resumable_reference(paused.runtime_ref.as_ref().unwrap()));
    request.operation = RuntimeOperation::Approval;
    request.runtime_ref = paused.runtime_ref;
    request.approvals = vec![RuntimeApprovalDecision {
        request_id: "wrong".into(),
        ..approval.clone()
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
    request.approvals = vec![approval];
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
        assert!(
            manager.process_registry().is_empty(),
            "{scenario}: {result:?}"
        );
    }
}
#[test]
fn process_closed_reports_counts_and_a_closed_category_never_vendor_output() {
    let (_dir, manager, request) = setup(Driver::Codex, "truncated");
    let error = manager.executors().unwrap()[0]
        .execute(&request, &mut continue_all, &CancelToken::new())
        .unwrap_err();
    assert_eq!(error.code, "process_closed");
    assert!(error.message.contains("exit="), "{}", error.message);
    assert!(error.message.contains("stderr="), "{}", error.message);
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
    // A resumed native thread receives only what it has not seen: the
    // harness's `resume_messages`.
    request.messages = vec![aidex_providers::ChatMessage::user("follow-up")];
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
    assert_eq!(launches(dir.path()), 1);
}

#[test]
fn shared_oauth_account_change_refuses_recovery_before_starting_a_process() {
    let (dir, manager, mut request, vault) = setup_with_vault(Driver::Codex, "oauth_binding");
    oauth_identity(&vault, "verified-account-a");
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
    oauth_identity(&vault, "verified-account-b");
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
    assert_eq!(launches(dir.path()), 1);
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
        let paused = executor
            .execute(
                &request,
                &mut |e| {
                    if let Some(found) = fingerprint(&request, e) {
                        decision = Some(RuntimeApprovalDecision {
                            approved: false,
                            ..found
                        });
                        return RuntimeControl::Pause;
                    }
                    RuntimeControl::Continue
                },
                &cancel,
            )
            .unwrap();
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
    let (dir, manager, mut request) = setup(Driver::Codex, "approval");
    let executor = manager.executors().unwrap().remove(0);
    let cancel = CancelToken::new();
    let paused = executor
        .execute(&request, &mut pause_on_approval, &cancel)
        .unwrap();
    assert_eq!(paused.status, RuntimeStatus::WaitingApproval);
    cancel.cancel();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !manager.process_registry().is_empty() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(!dir.path().join("effect.txt").exists());
    // The parked launch went with its tree: its home (restored snapshot and
    // credential) is gone and the run can be resumed.
    wait_until("parked home removed", || manager.temporary_homes() == 0);
    assert_eq!(manager.parked_runs(), 0);
    request.operation = RuntimeOperation::Resume;
    request.runtime_ref = paused.runtime_ref;
    let resumed = executor
        .execute(&request, &mut pause_on_approval, &CancelToken::new())
        .unwrap();
    assert_eq!(resumed.status, RuntimeStatus::WaitingApproval);
    assert_eq!(manager.release_paused(), 1);
}
#[test]
fn closing_the_gate_mid_turn_kills_the_peer_and_a_later_launch_works() {
    let gate = Arc::new(AtomicBool::new(true));
    let (dir, manager, request, _vault) = setup_with_gate(Driver::Codex, "cancel", gate.clone());
    let executor = manager.executors().unwrap().remove(0);
    let turn = {
        let request = request.clone();
        std::thread::spawn(move || {
            executor.execute(&request, &mut continue_all, &CancelToken::new())
        })
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while launches(dir.path()) == 0 || manager.process_registry().is_empty() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    // The peer is inside its turn: closing the gate (vault lock, emergency
    // stop, offline) ends it without the turn's cancellation token.
    std::thread::sleep(Duration::from_millis(300));
    gate.store(false, Ordering::SeqCst);
    assert!(turn.join().unwrap().is_err());
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !manager.process_registry().is_empty() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(manager.process_registry().is_accepting());
    gate.store(true, Ordering::SeqCst);
    set_argv(&manager, peer_argv(Driver::Codex, "normal"));
    assert_eq!(
        manager.executors().unwrap()[0]
            .execute(&request, &mut continue_all, &CancelToken::new())
            .unwrap()
            .status,
        RuntimeStatus::Completed
    );
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
fn claude_subagent_error_result_does_not_fail_root() {
    for driver in [Driver::ClaudeCode, Driver::Cursor] {
        let (_dir, manager, request) = setup(driver, "normal");
        let mut started = vec![];
        let done = manager.executors().unwrap()[0]
            .execute(
                &request,
                &mut |event| {
                    if let RuntimeEvent::SessionStarted { runtime_ref } = event {
                        started.push(runtime_ref.clone());
                    }
                    RuntimeControl::Continue
                },
                &CancelToken::new(),
            )
            .unwrap_or_else(|e| panic!("{driver:?}: {e}"));
        assert_eq!(done.status, RuntimeStatus::Completed);
        assert_eq!(done.message.unwrap().content, "hello");
        // One checkpoint, on the root session, never on the subagent's.
        assert_eq!(started.len(), 1);
        assert_eq!(started[0]["native_session_id"], "native-session");
        assert_redaction_stable(&started[0], &format!("{driver:?} checkpoint"));
        let reference = done.runtime_ref.unwrap();
        assert_eq!(reference["native_session_id"], "native-session");
        assert_redaction_stable(&reference, &format!("{driver:?} completion"));
    }
}

#[test]
fn claude_root_session_id_change_is_refused() {
    let (_dir, manager, request) = setup(Driver::ClaudeCode, "session_change");
    assert_eq!(
        manager.executors().unwrap()[0]
            .execute(&request, &mut continue_all, &CancelToken::new())
            .unwrap_err()
            .code,
        "protocol_error"
    );
    assert!(manager.process_registry().is_empty());
}

#[test]
fn official_login_encrypts_credentials_and_cancel_cleans_the_owned_tree() {
    for pending in [false, true] {
        let (dir, manager, _) = setup(Driver::Codex, "normal");
        set_argv(
            &manager,
            account_argv(if pending { &["pending"] } else { &[] }),
        );
        let flow = manager.login_start("fixture").unwrap();
        let final_status = if pending {
            wait_for_authorization(&manager, &flow.flow_id);
            manager.login_cancel("fixture", &flow.flow_id).unwrap()
        } else {
            wait_for_login(&manager, &flow.flow_id)
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
        if pending {
            // The cancelled login bound no account, so there is none to sign out.
            assert_eq!(
                manager.logout("fixture").unwrap_err().code,
                "invalid_request"
            );
        } else {
            manager.logout("fixture").unwrap();
        }
        assert!(!manager.account_status("fixture").unwrap().authenticated);
    }
}

#[test]
fn official_login_runs_the_app_server_and_reports_its_authorization_page() {
    let (_dir, manager, _) = setup(Driver::Codex, "normal");
    // The account peer fails any run of the browser-opening `login` command.
    set_argv(&manager, account_argv(&["pending"]));
    let flow = manager.login_start("fixture").unwrap();
    assert!(flow.authorization_url.is_none());
    wait_for_authorization(&manager, &flow.flow_id);
    let pending = manager.login_status("fixture", &flow.flow_id).unwrap();
    assert_eq!(pending.state, "pending");
    let url = pending.authorization_url.unwrap();
    assert!(url.starts_with("https://auth.openai.com/oauth/authorize?"));
    assert!(url.ends_with("state=app-server-fixture"));
    assert_eq!(
        manager
            .login_cancel("fixture", &flow.flow_id)
            .unwrap()
            .state,
        "cancelled"
    );
    assert!(manager.process_registry().is_empty());
    // An app-server that cannot start the login (an older CLI) fails the
    // flow without its text, and the runtime keeps its authentication.
    set_argv(&manager, account_argv(&["unsupported"]));
    let flow = manager.login_start("fixture").unwrap();
    let failed = wait_for_login(&manager, &flow.flow_id);
    assert_eq!(failed.state, "failed");
    assert_eq!(failed.error_code.as_deref(), Some("login_failed"));
    assert!(failed.authorization_url.is_none());
    assert!(
        !serde_json::to_string(&failed)
            .unwrap()
            .contains("arbitrary-secret")
    );
    assert!(manager.get("fixture").unwrap().profile.is_none());
    assert!(!manager.has_pending_logins());
    assert!(manager.process_registry().is_empty());
}

#[test]
fn an_executor_loaded_before_a_login_bound_its_account_refuses_to_launch() {
    let (dir, manager, request) = setup(Driver::Codex, "normal");
    let mut config = manager.get("fixture").unwrap();
    config.auth = RuntimeAuth::Vault {
        secret_name: "runtime-fixture-key".into(),
        env_name: "OPENAI_API_KEY".into(),
    };
    manager.save(config).unwrap();
    // What an engine that could not reload its runtimes still holds.
    let stale = manager.executors().unwrap().remove(0);
    let argv = manager.get("fixture").unwrap().argv;
    set_argv(&manager, account_argv(&[]));
    let generation = manager.generation();
    // The login binds its account from its worker thread, outside any call.
    let flow = manager.login_start("fixture").unwrap();
    assert_eq!(wait_for_login(&manager, &flow.flow_id).state, "connected");
    assert!(manager.generation() > generation);
    set_argv(&manager, argv);
    let before = launches(dir.path());
    assert_eq!(
        stale
            .execute(&request, &mut continue_all, &CancelToken::new())
            .unwrap_err()
            .code,
        "account_changed"
    );
    assert_eq!(launches(dir.path()), before);
    // The reloaded runtime launches with the managed account.
    assert_eq!(
        manager.executors().unwrap()[0]
            .execute(&request, &mut continue_all, &CancelToken::new())
            .unwrap()
            .status,
        RuntimeStatus::Completed
    );
    // A removed runtime's executor launches nothing either.
    let current = manager.executors().unwrap().remove(0);
    set_argv(&manager, account_argv(&[]));
    manager.logout("fixture").unwrap();
    assert!(manager.remove("fixture").unwrap());
    let before = launches(dir.path());
    assert_eq!(
        current
            .execute(&request, &mut continue_all, &CancelToken::new())
            .unwrap_err()
            .code,
        "not_found"
    );
    assert_eq!(launches(dir.path()), before);
}

#[test]
fn a_failed_account_rotation_never_resumes_the_old_account_under_the_new_one() {
    let (dir, manager, mut request) = setup(Driver::Codex, "normal");
    import(&manager, dir.path(), "OpaqueRotationAccountA_7301");
    let first = manager.executors().unwrap()[0]
        .execute(&request, &mut continue_all, &CancelToken::new())
        .unwrap();
    let reference = first.runtime_ref.unwrap();
    assert!(is_resumable_reference(&reference));
    // The revision record cannot be replaced: the rotation fails after the
    // new credential was written.
    let profile = manager.get("fixture").unwrap().profile.unwrap();
    let record = dir
        .path()
        .join("private")
        .join(format!("account-{}.sealed", digest(profile.as_bytes())));
    let saved = std::fs::read(&record).unwrap();
    std::fs::remove_file(&record).unwrap();
    std::fs::create_dir(&record).unwrap();
    std::fs::write(record.join("blocker"), b"").unwrap();
    let argv = manager.get("fixture").unwrap().argv;
    set_argv(&manager, account_argv(&[]));
    let source = dir.path().join("account-b.json");
    std::fs::write(
        &source,
        serde_json::json!({"tokens":{"access_token":"OpaqueRotationAccountB_7302"}}).to_string(),
    )
    .unwrap();
    assert!(manager.import_profile("fixture", &source).is_err());
    set_argv(&manager, argv);
    std::fs::remove_dir_all(&record).unwrap();
    std::fs::write(&record, saved).unwrap();
    // Account A's session snapshot went before the new credential was
    // written: its reference is refused before any process starts.
    request.operation = RuntimeOperation::Resume;
    request.run_id = "run-2".into();
    request.runtime_ref = Some(reference);
    request.messages = vec![aidex_providers::ChatMessage::user("follow-up")];
    let before = launches(dir.path());
    assert_eq!(
        manager.executors().unwrap()[0]
            .execute(&request, &mut continue_all, &CancelToken::new())
            .unwrap_err()
            .code,
        "resume_unavailable"
    );
    assert_eq!(launches(dir.path()), before);
    // And the runtime still authenticates as account A.
    set_argv(&manager, peer_argv(Driver::Codex, "credential_digest"));
    request.operation = RuntimeOperation::Start;
    request.run_id = "run-3".into();
    request.runtime_ref = None;
    let done = manager.executors().unwrap()[0]
        .execute(&request, &mut continue_all, &CancelToken::new())
        .unwrap();
    assert_eq!(
        done.message.unwrap().content,
        digest(b"OpaqueRotationAccountA_7301")
    );
}

#[test]
fn sign_out_and_account_changes_remove_snapshots_an_older_daemon_saved() {
    let (dir, manager, _) = setup(Driver::Codex, "normal");
    let private = dir.path().join("private");
    let legacy = |runtime: &str, session: &str| {
        private.join(format!(
            "runtime-{}.sealed",
            digest(format!("{runtime}:{session}").as_bytes())
        ))
    };
    // Another runtime's branch snapshot is this daemon's own and stays.
    let other_branch = private.join(format!(
        "runtime-{}-{}.sealed",
        digest(b"other"),
        digest(b"s:b")
    ));
    std::fs::write(&other_branch, vec![0u8; 64]).unwrap();
    // A legacy name does not tell whose it is, so every one goes.
    let older = [legacy("fixture", "s1"), legacy("other", "s2")];
    for path in &older {
        std::fs::write(path, vec![0u8; 64]).unwrap();
    }
    import(&manager, dir.path(), "OpaqueLegacySweepAccount_1101");
    for path in &older {
        assert!(!path.exists(), "{}", path.display());
    }
    std::fs::write(legacy("fixture", "s3"), vec![0u8; 64]).unwrap();
    set_argv(&manager, account_argv(&[]));
    assert_eq!(manager.logout("fixture").unwrap().status, "signed_out");
    assert!(!legacy("fixture", "s3").exists());
    assert!(other_branch.exists());
}

#[test]
fn removing_a_runtime_removes_its_managed_account_and_snapshots() {
    let (dir, manager, request) = setup(Driver::Codex, "normal");
    import(&manager, dir.path(), "OpaqueRemovedAccount_6620");
    assert_eq!(
        manager.executors().unwrap()[0]
            .execute(&request, &mut continue_all, &CancelToken::new())
            .unwrap()
            .status,
        RuntimeStatus::Completed
    );
    assert!(!runtime_snapshots(dir.path()).is_empty());
    let config = manager.get("fixture").unwrap();
    let profile = config.profile.clone().unwrap();
    let private = dir.path().join("private");
    let credential = private.join(format!("profile-{}.sealed", digest(profile.as_bytes())));
    let revision = private.join(format!("account-{}.sealed", digest(profile.as_bytes())));
    let legacy = private.join(format!("runtime-{}.sealed", digest(b"fixture:s")));
    std::fs::write(&legacy, vec![0u8; 64]).unwrap();
    assert!(credential.is_file() && revision.is_file());
    // A runtime sharing the profile keeps the account.
    let mut sharing = config.clone();
    sharing.id = "sharing".into();
    manager.save(sharing).unwrap();
    assert!(manager.remove("fixture").unwrap());
    assert!(credential.is_file() && revision.is_file());
    assert!(runtime_snapshots(dir.path()).is_empty());
    assert!(manager.remove("sharing").unwrap());
    assert!(!credential.exists());
    assert!(!revision.exists());
    assert!(!legacy.exists());
    // The configuration record still decodes.
    assert!(manager.list().unwrap().is_empty());
    // A runtime added again under the id finds no credential.
    let mut again = config;
    again.argv = account_argv(&[]);
    manager.save(again).unwrap();
    assert!(
        !manager
            .account_status("fixture")
            .unwrap()
            .credential_present
    );
}

#[test]
fn signing_out_a_runtime_without_a_managed_account_is_refused_and_removes_nothing() {
    let (dir, manager, request) = setup(Driver::Codex, "normal");
    let mut config = manager.get("fixture").unwrap();
    config.auth = RuntimeAuth::Vault {
        secret_name: "runtime-fixture-key".into(),
        env_name: "OPENAI_API_KEY".into(),
    };
    manager.save(config).unwrap();
    assert_eq!(
        manager.executors().unwrap()[0]
            .execute(&request, &mut continue_all, &CancelToken::new())
            .unwrap()
            .status,
        RuntimeStatus::Completed
    );
    let snapshots = runtime_snapshots(dir.path());
    assert!(!snapshots.is_empty());
    set_argv(&manager, account_argv(&[]));
    assert_eq!(
        manager.logout("fixture").unwrap_err().code,
        "invalid_request"
    );
    assert_eq!(runtime_snapshots(dir.path()), snapshots);
    assert!(matches!(
        manager.get("fixture").unwrap().auth,
        RuntimeAuth::Vault { .. }
    ));
}

#[test]
fn a_stale_full_save_cannot_unbind_a_signed_in_account() {
    let (dir, manager, _) = setup(Driver::Codex, "normal");
    // A client's copy, read before the import bound the account.
    let stale = manager.get("fixture").unwrap();
    import(&manager, dir.path(), "OpaqueBoundAccount_8080");
    let mut edit = stale;
    edit.argv = account_argv(&[]);
    assert_eq!(manager.save(edit).unwrap_err().code, "conflict");
    let config = manager.get("fixture").unwrap();
    assert!(config.profile.is_some());
    assert!(matches!(config.auth, RuntimeAuth::None));
    // The same edit of a fresh read keeps the account.
    let mut fresh = config;
    fresh.argv = account_argv(&[]);
    manager.save(fresh).unwrap();
    assert!(manager.account_status("fixture").unwrap().authenticated);
    // Signed out, the runtime may leave its profile.
    manager.logout("fixture").unwrap();
    let mut unbound = manager.get("fixture").unwrap();
    unbound.profile = None;
    manager.save(unbound).unwrap();
    assert!(manager.get("fixture").unwrap().profile.is_none());
}

#[test]
fn health_reports_a_managed_account_only_in_its_documented_codes() {
    let documented = [
        "ready",
        "binary_missing",
        "auth_required",
        "installed_auth_unverified",
        "protocol_ready_auth_unverified",
    ];
    let (dir, manager, _) = setup(Driver::Codex, "normal");
    import(&manager, dir.path(), "OpaqueHealthCodes_3030");
    // The account peer speaks no turn protocol, so the model probe fails:
    // what an offline confirmed account looks like.
    set_argv(&manager, account_argv(&[]));
    let health = manager.health("fixture").unwrap();
    assert_eq!(health.status, "installed_auth_unverified");
    assert!(health.authenticated);
    assert!(!health.protocol_ready);
    set_argv(&manager, account_argv(&["pending"]));
    let flow = manager.login_start("fixture").unwrap();
    let pending = manager.health("fixture").unwrap();
    assert_eq!(pending.status, "auth_required");
    assert!(!pending.authenticated);
    manager.login_cancel("fixture", &flow.flow_id).unwrap();
    set_argv(&manager, account_argv(&[]));
    manager.logout("fixture").unwrap();
    let signed_out = manager.health("fixture").unwrap();
    assert_eq!(signed_out.status, "auth_required");
    for status in [health.status, pending.status, signed_out.status] {
        assert!(documented.contains(&status.as_str()), "{status}");
    }
}

#[test]
fn cancelled_and_failed_logins_leave_previous_authentication_unchanged() {
    let (_dir, manager, _) = setup(Driver::Codex, "normal");
    let mut config = manager.get("fixture").unwrap();
    config.auth = RuntimeAuth::Vault {
        secret_name: "runtime-fixture-key".into(),
        env_name: "OPENAI_API_KEY".into(),
    };
    config.argv = account_argv(&["pending"]);
    manager.save(config).unwrap();
    let unchanged = |manager: &RuntimeManager| {
        let config = manager.get("fixture").unwrap();
        assert!(config.profile.is_none());
        assert!(matches!(config.auth, RuntimeAuth::Vault { .. }));
    };
    let flow = manager.login_start("fixture").unwrap();
    // A pending login changes nothing saved.
    unchanged(&manager);
    assert!(manager.has_pending_logins());
    wait_for_authorization(&manager, &flow.flow_id);
    assert_eq!(
        manager
            .login_cancel("fixture", &flow.flow_id)
            .unwrap()
            .state,
        "cancelled"
    );
    unchanged(&manager);
    assert!(!manager.has_pending_logins());
    // A lock or emergency stop cancels every login; the manager stays usable.
    let flow = manager.login_start("fixture").unwrap();
    wait_for_authorization(&manager, &flow.flow_id);
    manager.cancel_logins().unwrap();
    assert_eq!(
        manager
            .login_status("fixture", &flow.flow_id)
            .unwrap()
            .state,
        "cancelled"
    );
    unchanged(&manager);
    set_argv(&manager, account_argv(&["fail"]));
    let flow = manager.login_start("fixture").unwrap();
    let failed = wait_for_login(&manager, &flow.flow_id);
    assert_eq!(failed.state, "failed");
    assert_eq!(failed.error_code.as_deref(), Some("login_failed"));
    assert!(
        !serde_json::to_string(&failed)
            .unwrap()
            .contains("arbitrary-secret")
    );
    unchanged(&manager);
    // Only a connected login switches the runtime to its managed account.
    set_argv(&manager, account_argv(&[]));
    let flow = manager.login_start("fixture").unwrap();
    assert_eq!(wait_for_login(&manager, &flow.flow_id).state, "connected");
    let config = manager.get("fixture").unwrap();
    assert_eq!(config.profile.as_deref(), Some(flow.profile.as_str()));
    assert!(matches!(config.auth, RuntimeAuth::None));
    assert!(manager.process_registry().is_accepting());
}

#[test]
fn explicitly_selected_native_credential_import_is_validated_and_encrypts_only_auth_file() {
    let (dir, manager, _) = setup(Driver::Codex, "normal");
    set_argv(&manager, account_argv(&[]));
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
fn import_refuses_links_and_oversized_credentials_without_echoing_contents() {
    let (dir, manager, _) = setup(Driver::Codex, "normal");
    set_argv(&manager, account_argv(&[]));
    let real = dir.path().join("real-profile");
    std::fs::create_dir(&real).unwrap();
    std::fs::write(
        real.join("auth.json"),
        br#"{"tokens":{"access_token":"LinkedProfileSecret_2718"}}"#,
    )
    .unwrap();
    let linked = dir.path().join("linked-profile");
    #[cfg(windows)]
    assert!(
        std::process::Command::new("cmd.exe")
            .args(["/C", "mklink", "/J"])
            .arg(&linked)
            .arg(&real)
            .output()
            .unwrap()
            .status
            .success()
    );
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real, &linked).unwrap();
    let oversized = dir.path().join("oversized-auth.json");
    let mut bytes = br#"{"tokens":{"access_token":"OversizedSecret_1618"},"pad":""#.to_vec();
    bytes.resize(1024 * 1024 + 1, b'x');
    std::fs::write(&oversized, bytes).unwrap();
    for source in [&linked, &oversized] {
        let refused = manager.import_profile("fixture", source).unwrap_err();
        assert_eq!(refused.code, "profile_invalid");
        assert!(!refused.message.contains("Secret_"));
    }
    let config = manager.get("fixture").unwrap();
    assert!(config.profile.is_none());
    assert!(manager.process_registry().is_empty());
}

#[test]
fn imported_native_account_secret_is_scrubbed_from_runtime_protocol() {
    let (dir, manager, request) = setup(Driver::Codex, "native_secret");
    import(&manager, dir.path(), "OpaqueNativeAccountSecret_5159");
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
    // Session snapshots never duplicate the managed credential.
    for entry in std::fs::read_dir(dir.path().join("private")).unwrap() {
        let bytes = std::fs::read(entry.unwrap().path()).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("OpaqueNativeAccountSecret"));
    }
}

#[test]
fn signed_out_managed_account_fails_auth_required() {
    let (dir, manager, request) = setup(Driver::Codex, "normal");
    import(&manager, dir.path(), "OpaqueSignedOutAccount_3141");
    let executor = manager.executors().unwrap().remove(0);
    assert_eq!(
        executor
            .execute(&request, &mut continue_all, &CancelToken::new())
            .unwrap()
            .status,
        RuntimeStatus::Completed
    );
    let argv = manager.get("fixture").unwrap().argv;
    set_argv(&manager, account_argv(&[]));
    assert_eq!(manager.logout("fixture").unwrap().status, "signed_out");
    set_argv(&manager, argv);
    let before = launches(dir.path());
    assert_eq!(
        manager.executors().unwrap()[0]
            .execute(&request, &mut continue_all, &CancelToken::new())
            .unwrap_err()
            .code,
        "auth_required"
    );
    assert_eq!(launches(dir.path()), before);
    // Logout removed every session snapshot of the runtime.
    assert!(runtime_snapshots(dir.path()).is_empty());
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
    let checkpoint = checkpoint.expect("native identity must be checkpointed before turn effects");
    assert!(is_resumable_reference(&checkpoint));
    request.runtime_ref = Some(checkpoint);
    request.operation = RuntimeOperation::Resume;
    request.run_id = "run-2".into();
    // A resumed native thread receives only what it has not seen: the
    // harness's `resume_messages`.
    request.messages = vec![aidex_providers::ChatMessage::user("follow-up")];
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
fn every_resume_refusal_happens_before_a_process_starts() {
    let (dir, manager, mut request) = setup(Driver::Codex, "restart");
    let executor = manager.executors().unwrap().remove(0);
    let saved = executor
        .execute(&request, &mut continue_all, &CancelToken::new())
        .unwrap()
        .runtime_ref
        .unwrap();
    assert!(is_resumable_reference(&saved));
    request.operation = RuntimeOperation::Resume;
    request.run_id = "run-2".into();
    let main_era = serde_json::json!({"runtime_id":"fixture","native_session_id":"native-session","native_turn_id":"native-turn","snapshot_id":saved["snapshot_id"],"run_id":"run-1","session_id":"session-1"});
    let mut no_branch = saved.clone();
    no_branch
        .as_object_mut()
        .unwrap()
        .remove("native_branch_id");
    let mut other_session = saved.clone();
    other_session["session_id"] = "session-2".into();
    let mut other_runtime = saved.clone();
    other_runtime["runtime_id"] = "other".into();
    let mut changed_digest = saved.clone();
    changed_digest["snapshot_digest"] = digest(b"changed").into();
    let mut other_branch = saved.clone();
    other_branch["native_branch_id"] = "another-branch".into();
    let mut missing_identity = saved.clone();
    missing_identity["native_session_id"] = "".into();
    let mut other_config = saved.clone();
    other_config["config_digest"] = digest(b"other configuration").into();
    let mut no_config = saved.clone();
    no_config.as_object_mut().unwrap().remove("config_digest");
    let mut other_binding = saved.clone();
    other_binding["credential_binding"] =
        serde_json::json!({"kind":"vault","record":digest(b"other record"),"revision":1});
    let refusals = [
        main_era,
        no_branch,
        other_session,
        other_runtime,
        changed_digest,
        other_branch,
        missing_identity,
        other_config,
        no_config,
        other_binding,
    ];
    let before = launches(dir.path());
    for reference in refusals {
        request.runtime_ref = Some(reference.clone());
        assert_eq!(
            executor
                .execute(&request, &mut continue_all, &CancelToken::new())
                .unwrap_err()
                .code,
            "resume_unavailable",
            "{reference}"
        );
    }
    request.runtime_ref = None;
    assert_eq!(
        executor
            .execute(&request, &mut continue_all, &CancelToken::new())
            .unwrap_err()
            .code,
        "resume_unavailable"
    );
    // A damaged snapshot is a refusal too, not a launch.
    let snapshot = dir
        .path()
        .join("private")
        .join(saved["snapshot_id"].as_str().unwrap());
    std::fs::write(&snapshot, vec![7u8; 128]).unwrap();
    request.runtime_ref = Some(saved);
    assert_eq!(
        executor
            .execute(&request, &mut continue_all, &CancelToken::new())
            .unwrap_err()
            .code,
        "resume_unavailable"
    );
    assert_eq!(launches(dir.path()), before);
    assert!(manager.process_registry().is_empty());
}

#[test]
fn oversized_completion_snapshot_completes_with_a_reference_that_is_not_resumable() {
    let (dir, manager, request) = setup(Driver::Codex, "large");
    let mut checkpoint = None;
    let done = manager.executors().unwrap()[0]
        .execute(
            &request,
            &mut |event| {
                if let RuntimeEvent::SessionStarted { runtime_ref } = event {
                    checkpoint = Some(runtime_ref.clone());
                }
                RuntimeControl::Continue
            },
            &CancelToken::new(),
        )
        .unwrap();
    // The turn's work already ran, so it completes.
    assert_eq!(done.status, RuntimeStatus::Completed);
    assert_eq!(done.message.unwrap().content, "hello");
    assert!(is_resumable_reference(&checkpoint.unwrap()));
    let reference = done.runtime_ref.unwrap();
    assert!(!is_resumable_reference(&reference));
    assert_eq!(reference["snapshot_error"], "state_limit");
    // The checkpoint taken before the turn is not left to be resumed.
    assert!(runtime_snapshots(dir.path()).is_empty());
}

#[test]
fn legacy_named_snapshot_is_removed_after_the_first_branch_snapshot() {
    let (dir, manager, request) = setup(Driver::Codex, "normal");
    let private = dir.path().join("private");
    let ours = private.join(format!("runtime-{}.sealed", digest(b"fixture:session-1")));
    let other_session = private.join(format!("runtime-{}.sealed", digest(b"fixture:session-2")));
    for legacy in [&ours, &other_session] {
        std::fs::write(legacy, vec![0u8; 64]).unwrap();
    }
    let done = manager.executors().unwrap()[0]
        .execute(&request, &mut continue_all, &CancelToken::new())
        .unwrap();
    assert!(is_resumable_reference(done.runtime_ref.as_ref().unwrap()));
    assert!(!ours.exists());
    assert!(other_session.exists());
    assert_eq!(runtime_snapshots(dir.path()).len(), 1);
}

#[test]
fn changing_native_account_refuses_old_session_identity_without_replaying_effects() {
    let (dir, manager, mut request) = setup(Driver::Codex, "restart");
    import(&manager, dir.path(), "OpaqueAccountA_1064");
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
    import(&manager, dir.path(), "OpaqueAccountB_2085");
    request.operation = RuntimeOperation::Resume;
    request.run_id = "run-2".into();
    // A resumed native thread receives only what it has not seen: the
    // harness's `resume_messages`.
    request.messages = vec![aidex_providers::ChatMessage::user("follow-up")];
    let before = launches(dir.path());
    assert_eq!(
        manager.executors().unwrap()[0]
            .execute(
                &request,
                &mut |_| RuntimeControl::Continue,
                &CancelToken::new()
            )
            .unwrap_err()
            .code,
        "resume_unavailable"
    );
    assert_eq!(launches(dir.path()), before);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("effect.txt")).unwrap(),
        "once\n"
    );
}

#[test]
fn shutdown_cancels_pending_official_login_and_refuses_native_turns_during_login() {
    let (_dir, manager, request) = setup(Driver::Codex, "normal");
    set_argv(&manager, account_argv(&["pending"]));
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
        request.messages = vec![aidex_providers::ChatMessage::user("follow-up")];
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
fn every_emitted_reference_survives_redaction_for_each_credential_kind() {
    for kind in ["none", "vault", "codex_oauth", "managed_profile"] {
        let (dir, manager, mut request, vault) = setup_with_vault(Driver::Codex, "approval");
        let mut config = manager.get("fixture").unwrap();
        match kind {
            "vault" => {
                config.auth = RuntimeAuth::Vault {
                    secret_name: "runtime-fixture-key".into(),
                    env_name: "OPENAI_API_KEY".into(),
                };
                manager.save(config).unwrap();
            }
            "codex_oauth" => {
                oauth_identity(&vault, "verified-account-a");
                config.auth = RuntimeAuth::CodexOauth {
                    provider_id: "openai".into(),
                };
                manager.save(config).unwrap();
            }
            "managed_profile" => import(&manager, dir.path(), "OpaqueReferenceAccount_7781"),
            _ => {}
        }
        let executor = manager.executors().unwrap().remove(0);
        let mut emitted = vec![];
        let mut decision = None;
        let paused = executor
            .execute(
                &request,
                &mut |event| {
                    match event {
                        RuntimeEvent::SessionStarted { runtime_ref } => {
                            emitted.push(("session started", runtime_ref.clone()));
                        }
                        RuntimeEvent::ApprovalRequest { runtime_ref, .. } => {
                            emitted.push(("approval request", runtime_ref.clone()));
                            decision = fingerprint(&request, event);
                            return RuntimeControl::Pause;
                        }
                        _ => {}
                    }
                    RuntimeControl::Continue
                },
                &CancelToken::new(),
            )
            .unwrap_or_else(|e| panic!("{kind}: {e}"));
        assert_eq!(paused.status, RuntimeStatus::WaitingApproval, "{kind}");
        emitted.push(("waiting approval", paused.runtime_ref.clone().unwrap()));
        request.operation = RuntimeOperation::Approval;
        request.runtime_ref = paused.runtime_ref;
        request.approvals = vec![decision.unwrap()];
        let done = executor
            .execute(&request, &mut continue_all, &CancelToken::new())
            .unwrap_or_else(|e| panic!("{kind}: {e}"));
        assert_eq!(done.status, RuntimeStatus::Completed, "{kind}");
        emitted.push(("completed", done.runtime_ref.unwrap()));
        let points: Vec<_> = emitted.iter().map(|(point, _)| *point).collect();
        assert_eq!(
            points,
            [
                "session started",
                "approval request",
                "waiting approval",
                "completed"
            ],
            "{kind}"
        );
        let approval = &emitted[1].1;
        assert_eq!(approval["native_request_id"], 77, "{kind}");
        assert!(approval["approval_digest"].is_string(), "{kind}");
        let vault_record = vault
            .secret_version(&aidex_vault::SecretName::new("runtime-fixture-key").unwrap())
            .unwrap()
            .secret_ref
            .secret_id
            .to_string();
        for (point, reference) in &emitted {
            let context = format!("{kind} {point}");
            assert_redaction_stable(reference, &context);
            assert!(reference["config_digest"].is_string(), "{context}");
            assert_eq!(
                reference["credential_binding"]["kind"],
                if kind == "managed_profile" {
                    "none"
                } else {
                    kind
                },
                "{context}"
            );
            assert_eq!(
                reference["account_revision"].is_string(),
                kind == "managed_profile",
                "{context}"
            );
            let text = reference.to_string();
            // Value-free: no credential, and the vault record only as a digest.
            assert!(!text.contains("opaqueMarker"), "{context}");
            assert!(!text.contains("OpaqueReferenceAccount"), "{context}");
            assert!(!text.contains(&vault_record), "{context}");
            if *point != "approval request" {
                assert!(is_resumable_reference(reference), "{context}");
            }
        }
    }
}
#[test]
fn drivers_without_native_restore_keep_no_snapshots_or_checkpoints() {
    for driver in [Driver::Pi, Driver::Omp] {
        let (dir, manager, mut request) = setup(driver, "normal");
        // A snapshot an older daemon kept for this session is never restored.
        let legacy = dir
            .path()
            .join("private")
            .join(format!("runtime-{}.sealed", digest(b"fixture:session-1")));
        std::fs::write(&legacy, vec![0u8; 64]).unwrap();
        let executor = manager.executors().unwrap().remove(0);
        assert!(!executor.descriptor().supports_resume);
        for run in ["run-1", "run-2"] {
            request.run_id = run.into();
            let mut started = 0;
            let done = executor
                .execute(
                    &request,
                    &mut |event| {
                        if matches!(event, RuntimeEvent::SessionStarted { .. }) {
                            started += 1;
                        }
                        RuntimeControl::Continue
                    },
                    &CancelToken::new(),
                )
                .unwrap_or_else(|e| panic!("{driver:?}: {e}"));
            assert_eq!(done.status, RuntimeStatus::Completed);
            assert_eq!(started, 0, "{driver:?}");
            let reference = done.runtime_ref.unwrap();
            assert!(!is_resumable_reference(&reference), "{driver:?}");
            // Not a failed snapshot: none is taken.
            assert!(reference.get("snapshot_error").is_none(), "{driver:?}");
            assert_redaction_stable(&reference, &format!("{driver:?}"));
        }
        assert!(runtime_snapshots(dir.path()).is_empty(), "{driver:?}");
        assert!(!legacy.exists(), "{driver:?}");
    }
}
#[test]
fn credential_refreshed_before_a_failed_turn_is_kept() {
    let (dir, manager, request) = setup(Driver::Codex, "refresh_cut");
    import(&manager, dir.path(), "OpaqueInitialAccount_4242");
    assert!(
        manager.executors().unwrap()[0]
            .execute(&request, &mut continue_all, &CancelToken::new())
            .is_err()
    );
    assert!(manager.process_registry().is_empty());
    // The next launch restores the refreshed credential, not the imported one.
    set_argv(&manager, peer_argv(Driver::Codex, "expect_refreshed"));
    assert_eq!(
        manager.executors().unwrap()[0]
            .execute(&request, &mut continue_all, &CancelToken::new())
            .unwrap()
            .status,
        RuntimeStatus::Completed
    );
    for entry in std::fs::read_dir(dir.path().join("private")).unwrap() {
        let bytes = std::fs::read(entry.unwrap().path()).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("RefreshedOpaqueToken"));
    }
}
#[test]
fn health_of_a_managed_account_without_its_binary_reports_binary_missing() {
    let (dir, manager, _) = setup(Driver::Codex, "normal");
    import(&manager, dir.path(), "OpaqueHealthAccount_5521");
    let mut config = manager.get("fixture").unwrap();
    config.executable = dir
        .path()
        .join("missing-codex.exe")
        .to_string_lossy()
        .into_owned();
    manager.save(config).unwrap();
    let health = manager.health("fixture").unwrap();
    assert!(!health.installed);
    assert!(!health.authenticated);
    assert!(!health.protocol_ready);
    assert_eq!(health.status, "binary_missing");
    assert!(manager.process_registry().is_empty());
}
#[test]
fn closing_the_gate_cancels_a_pending_login_and_a_later_login_works() {
    let gate = Arc::new(AtomicBool::new(true));
    let (_dir, manager, request, _vault) = setup_with_gate(Driver::Codex, "normal", gate.clone());
    set_argv(&manager, account_argv(&["pending"]));
    let flow = manager.login_start("fixture").unwrap();
    wait_for_authorization(&manager, &flow.flow_id);
    assert!(manager.has_pending_logins());
    // A vault lock, an emergency stop or going offline closes the gate.
    gate.store(false, Ordering::SeqCst);
    let ended = wait_for_login(&manager, &flow.flow_id);
    assert_eq!(ended.state, "cancelled");
    assert!(ended.authorization_url.is_none());
    assert!(!manager.has_pending_logins());
    assert!(manager.process_registry().is_empty());
    let config = manager.get("fixture").unwrap();
    assert!(config.profile.is_none());
    assert!(matches!(config.auth, RuntimeAuth::None));
    assert_eq!(
        manager.login_start("fixture").unwrap_err().code,
        "offline_refused"
    );
    // Reopened, the manager starts logins and turns again.
    gate.store(true, Ordering::SeqCst);
    assert!(manager.executors().unwrap()[0].validate(&request).is_ok());
    set_argv(&manager, account_argv(&[]));
    let flow = manager.login_start("fixture").unwrap();
    assert_eq!(wait_for_login(&manager, &flow.flow_id).state, "connected");
    assert!(!manager.has_pending_logins());
    assert!(manager.process_registry().is_accepting());
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
    assert!(!dir.path().join("runtimes/codex/standalone.exe").exists());
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
    assert_eq!(
        manager.install(Driver::Codex, None).unwrap_err().code,
        "conflict"
    );
}
#[test]
fn failed_run_returns_only_after_its_live_native_tree_left_the_registry() {
    // The peer answers turn/start with an error and keeps running, so the run fails
    // without the transport's own stop path. Several rounds make a stale entry, which
    // used to linger until the monitor thread reaped the killed tree, near certain.
    for _ in 0..5 {
        let (_dir, manager, request) = setup(Driver::Codex, "auth");
        let error = manager.executors().unwrap()[0]
            .execute(
                &request,
                &mut |_| RuntimeControl::Continue,
                &CancelToken::new(),
            )
            .unwrap_err();
        assert_eq!(error.code, "auth_required");
        assert!(manager.process_registry().is_empty());
        // Configuration changes are refused while a runtime is live; a failed run
        // must not leave a registration that reports the runtime as still busy.
        manager.save(manager.get("fixture").unwrap()).unwrap();
    }
}
fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(std::time::Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}
/// Start `request` and pause at its approval: the response and the decision
/// that approves exactly that request.
fn pause_for_approval(
    executor: &Arc<dyn RuntimeExecutor>,
    request: &RuntimeRequest,
    cancel: &CancelToken,
) -> (RuntimeResponse, RuntimeApprovalDecision) {
    let mut decision = None;
    let paused = executor
        .execute(
            request,
            &mut |event| {
                if let Some(found) = fingerprint(request, event) {
                    decision = Some(found);
                    return RuntimeControl::Pause;
                }
                RuntimeControl::Continue
            },
            cancel,
        )
        .unwrap();
    assert_eq!(paused.status, RuntimeStatus::WaitingApproval);
    (paused, decision.unwrap())
}
#[test]
fn a_parked_approval_released_by_cancel_or_a_closed_gate_leaves_no_home_and_resumes() {
    for driver in [Driver::Acp, Driver::Codex] {
        for closed_gate in [false, true] {
            let gate = Arc::new(AtomicBool::new(true));
            let (dir, manager, mut request, _vault) =
                setup_with_gate(driver, "approval", gate.clone());
            let executor = manager.executors().unwrap().remove(0);
            let cancel = CancelToken::new();
            let (paused, decision) = pause_for_approval(&executor, &request, &cancel);
            assert_eq!(manager.parked_runs(), 1);
            assert_eq!(manager.temporary_homes(), 1);
            // A run.cancel, or a vault lock, emergency stop or going offline.
            if closed_gate {
                gate.store(false, Ordering::SeqCst);
            } else {
                cancel.cancel();
            }
            wait_until("parked launch released", || {
                manager.process_registry().is_empty()
                    && manager.temporary_homes() == 0
                    && manager.parked_runs() == 0
            });
            gate.store(true, Ordering::SeqCst);
            // Its decision has nothing to continue any more.
            let mut approval = request.clone();
            approval.operation = RuntimeOperation::Approval;
            approval.runtime_ref = paused.runtime_ref.clone();
            approval.approvals = vec![decision];
            assert_eq!(
                executor
                    .execute(&approval, &mut continue_all, &CancelToken::new())
                    .unwrap_err()
                    .code,
                "approval_expired",
                "{driver:?}"
            );
            // The documented continuation after a lock or a cancel works.
            request.operation = RuntimeOperation::Resume;
            request.runtime_ref = paused.runtime_ref;
            let resumed = executor
                .execute(&request, &mut pause_on_approval, &CancelToken::new())
                .unwrap_or_else(|e| panic!("{driver:?} {closed_gate}: {e}"));
            assert_eq!(resumed.status, RuntimeStatus::WaitingApproval);
            assert!(!dir.path().join("effect.txt").exists());
            assert_eq!(manager.release_paused(), 1);
            assert_eq!(manager.temporary_homes(), 0);
            assert!(manager.process_registry().is_empty());
        }
    }
}
#[test]
fn releasing_parked_approvals_removes_their_homes_before_it_returns() {
    let (_dir, manager, mut request) = setup(Driver::Acp, "approval");
    let executor = manager.executors().unwrap().remove(0);
    let (paused, _) = pause_for_approval(&executor, &request, &CancelToken::new());
    // What the vault lock observer, emergency stop and going offline call.
    assert_eq!(manager.release_paused(), 1);
    assert_eq!(manager.temporary_homes(), 0);
    assert_eq!(manager.parked_runs(), 0);
    assert!(manager.process_registry().is_empty());
    request.operation = RuntimeOperation::Resume;
    request.runtime_ref = paused.runtime_ref;
    assert_eq!(
        executor
            .execute(&request, &mut pause_on_approval, &CancelToken::new())
            .unwrap()
            .status,
        RuntimeStatus::WaitingApproval
    );
    // One run released by id, as for a cancelled or failed run.
    assert!(manager.release_run(&request.run_id));
    assert!(!manager.release_run(&request.run_id));
    assert_eq!(manager.temporary_homes(), 0);
    assert!(manager.process_registry().is_empty());
}
#[test]
fn a_resume_supersedes_a_launch_still_waiting_while_a_start_is_refused() {
    let (dir, manager, mut request) = setup(Driver::Codex, "approval");
    let executor = manager.executors().unwrap().remove(0);
    let (paused, first) = pause_for_approval(&executor, &request, &CancelToken::new());
    assert_eq!(
        executor
            .execute(&request, &mut continue_all, &CancelToken::new())
            .unwrap_err()
            .code,
        "runtime_busy"
    );
    request.operation = RuntimeOperation::Resume;
    request.runtime_ref = paused.runtime_ref;
    let (resumed, second) = pause_for_approval(&executor, &request, &CancelToken::new());
    // Only the new launch is left, with its own home.
    assert_eq!(manager.parked_runs(), 1);
    assert_eq!(manager.temporary_homes(), 1);
    // The vendor numbers requests per connection: the same wire id, scoped to
    // its launch, so the earlier launch's decision never matches.
    assert_ne!(first.request_id, second.request_id);
    assert!(first.request_id.ends_with(":77") && second.request_id.ends_with(":77"));
    assert_eq!(first.runtime_ref["native_request_id"], 77);
    assert_eq!(second.runtime_ref["native_request_id"], 77);
    request.operation = RuntimeOperation::Approval;
    request.runtime_ref = resumed.runtime_ref;
    request.approvals = vec![first];
    assert_eq!(
        executor
            .execute(&request, &mut continue_all, &CancelToken::new())
            .unwrap_err()
            .code,
        "approval_mismatch"
    );
    request.approvals = vec![second];
    assert_eq!(
        executor
            .execute(&request, &mut continue_all, &CancelToken::new())
            .unwrap()
            .status,
        RuntimeStatus::Completed
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("effect.txt")).unwrap(),
        "once\n"
    );
    assert_eq!(manager.temporary_homes(), 0);
}
#[test]
fn acp_follow_up_continues_from_a_spec_shaped_load_without_its_history_replay() {
    for scenario in ["spec_load", "acp_replay"] {
        let (dir, manager, mut request) = setup(Driver::Acp, scenario);
        let executor = manager.executors().unwrap().remove(0);
        let done = executor
            .execute(&request, &mut continue_all, &CancelToken::new())
            .unwrap();
        request.runtime_ref = done.runtime_ref;
        request.operation = RuntimeOperation::Resume;
        request.run_id = "run-2".into();
        request.messages = vec![aidex_providers::ChatMessage::user("follow-up")];
        let mut observed = vec![];
        let follow_up = executor
            .execute(
                &request,
                &mut |event| {
                    observed.push(serde_json::to_string(event).unwrap());
                    RuntimeControl::Continue
                },
                &CancelToken::new(),
            )
            .unwrap_or_else(|e| panic!("{scenario}: {e}"));
        assert_eq!(follow_up.status, RuntimeStatus::Completed);
        // The replayed history is not the new turn's text, tools or steps.
        assert_eq!(follow_up.message.unwrap().content, "hello", "{scenario}");
        let observed = observed.join("\n");
        assert!(!observed.contains("OLD"), "{scenario}");
        assert!(!observed.contains("old-tool"), "{scenario}");
        let reference = follow_up.runtime_ref.unwrap();
        assert_eq!(reference["native_session_id"], "native-session");
        assert!(is_resumable_reference(&reference));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("loads.txt")).unwrap(),
            "native-session\n"
        );
    }
}
#[test]
fn acp_that_cannot_load_its_session_refuses_the_resume_as_unavailable() {
    for scenario in ["no_load", "load_error"] {
        let (dir, manager, mut request) = setup(Driver::Acp, scenario);
        let executor = manager.executors().unwrap().remove(0);
        let done = executor
            .execute(&request, &mut continue_all, &CancelToken::new())
            .unwrap();
        request.runtime_ref = done.runtime_ref;
        request.operation = RuntimeOperation::Resume;
        request.run_id = "run-2".into();
        let mut events = 0;
        let error = executor
            .execute(
                &request,
                &mut |_| {
                    events += 1;
                    RuntimeControl::Continue
                },
                &CancelToken::new(),
            )
            .unwrap_err();
        // The harness then starts a new native session with the transcript.
        assert_eq!(error.code, "resume_unavailable", "{scenario}");
        assert_eq!(events, 0, "{scenario}");
        // An agent without `loadSession` is never asked to load.
        assert_eq!(
            dir.path().join("loads.txt").exists(),
            scenario == "load_error"
        );
        assert!(manager.process_registry().is_empty());
        assert_eq!(manager.temporary_homes(), 0);
    }
}
#[test]
fn a_turn_continued_after_its_approval_completes_with_the_whole_turn_text() {
    for driver in [Driver::Codex, Driver::Acp] {
        let (_dir, manager, mut request) = setup(driver, "approval_preamble");
        let executor = manager.executors().unwrap().remove(0);
        let (paused, decision) = pause_for_approval(&executor, &request, &CancelToken::new());
        request.operation = RuntimeOperation::Approval;
        request.runtime_ref = paused.runtime_ref;
        request.approvals = vec![decision];
        let mut streamed = String::new();
        let done = executor
            .execute(
                &request,
                &mut |event| {
                    if let RuntimeEvent::TextDelta { text } = event {
                        streamed.push_str(text);
                    }
                    RuntimeControl::Continue
                },
                &CancelToken::new(),
            )
            .unwrap();
        // The continuation streams only what follows the pause; its completed
        // message is the whole native turn, for the harness to replace the
        // turn's partial text with.
        assert_eq!(streamed, "hello", "{driver:?}");
        assert_eq!(done.message.unwrap().content, "before hello", "{driver:?}");
    }
}
#[test]
fn a_credential_streamed_in_fragments_is_scrubbed_from_the_completed_message() {
    let (_dir, manager, request) = setup(Driver::Codex, "split_secret");
    let mut config = manager.get("fixture").unwrap();
    config.auth = RuntimeAuth::Vault {
        secret_name: "runtime-fixture-key".into(),
        env_name: "OPENAI_API_KEY".into(),
    };
    manager.save(config).unwrap();
    let done = manager.executors().unwrap()[0]
        .execute(&request, &mut continue_all, &CancelToken::new())
        .unwrap();
    let message = done.message.unwrap().content;
    assert!(!message.contains("opaqueMarker"), "{message}");
    assert_eq!(message, "said [REDACTED] done");
}
#[test]
fn concurrent_launches_of_one_managed_account_never_reuse_its_rotated_token() {
    let (dir, manager, request) = setup(Driver::Codex, "rotate");
    import(&manager, dir.path(), "OpaqueRotatingAccount_6612");
    let executor = manager.executors().unwrap().remove(0);
    let turns: Vec<_> = (0..2)
        .map(|i| {
            let executor = executor.clone();
            let mut request = request.clone();
            request.session_id = format!("session-{i}");
            request.run_id = format!("run-{i}");
            std::thread::spawn(move || {
                executor.execute(
                    &request,
                    &mut continue_all,
                    &CancelToken::new().with_timeout(Duration::from_secs(60)),
                )
            })
        })
        .collect();
    for turn in turns {
        assert_eq!(
            turn.join().unwrap().unwrap().status,
            RuntimeStatus::Completed
        );
    }
    // The stored profile holds the newest token: the next launch starts from
    // a token the provider has not seen used.
    assert_eq!(
        executor
            .execute(&request, &mut continue_all, &CancelToken::new())
            .unwrap()
            .status,
        RuntimeStatus::Completed
    );
    let used = std::fs::read_to_string(dir.path().join("used-tokens.txt")).unwrap();
    let used: Vec<_> = used.lines().collect();
    assert_eq!(used.len(), 3);
    assert_eq!(used[0], "OpaqueRotatingAccount_6612");
    assert!(used[1] != used[2] && used[1] != used[0]);
    for entry in std::fs::read_dir(dir.path().join("private")).unwrap() {
        let bytes = std::fs::read(entry.unwrap().path()).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("Rotated"));
    }
}
#[test]
fn health_never_probes_while_a_launch_of_the_managed_account_is_live() {
    let (dir, manager, request) = setup(Driver::Codex, "approval");
    import(&manager, dir.path(), "OpaqueProbeAccount_7781");
    let executor = manager.executors().unwrap().remove(0);
    pause_for_approval(&executor, &request, &CancelToken::new());
    let before = launches(dir.path());
    let health = manager.health("fixture").unwrap();
    assert!(!health.protocol_ready);
    assert_eq!(launches(dir.path()), before);
    assert_eq!(manager.release_paused(), 1);
    // With no launch of the account left, the probe runs again.
    manager.health("fixture").unwrap();
    assert_eq!(launches(dir.path()), before + 1);
}
#[test]
fn superseded_branch_snapshots_are_forgotten_only_for_their_own_runtime() {
    let (dir, manager, mut request) = setup(Driver::Codex, "normal");
    let executor = manager.executors().unwrap().remove(0);
    let mut references = vec![];
    for (session, run) in [
        ("session-1", "run-1"),
        ("session-1", "run-2"),
        ("session-2", "run-3"),
    ] {
        request.session_id = session.into();
        request.run_id = run.into();
        references.push(
            executor
                .execute(&request, &mut continue_all, &CancelToken::new())
                .unwrap()
                .runtime_ref
                .unwrap(),
        );
    }
    assert_eq!(runtime_snapshots(dir.path()).len(), 3);
    // A reference that names a record which is not one of its runtime's
    // branch snapshots removes nothing.
    let mut other_runtime = references[0].clone();
    other_runtime["runtime_id"] = "other".into();
    let mut forged = references[0].clone();
    forged["snapshot_id"] = "runtime-config.sealed".into();
    for reference in [&other_runtime, &forged] {
        assert!(!manager.forget_snapshot(reference));
    }
    assert_eq!(manager.list().unwrap().len(), 1);
    assert_eq!(runtime_snapshots(dir.path()).len(), 3);
    // A thread a configuration change ended, a refused resume.
    assert!(manager.forget_snapshot(&references[0]));
    assert!(!manager.forget_snapshot(&references[0]));
    assert_eq!(runtime_snapshots(dir.path()).len(), 2);
    // A deleted conversation, with the snapshot an older daemon kept for it.
    let legacy = dir
        .path()
        .join("private")
        .join(format!("runtime-{}.sealed", digest(b"fixture:session-1")));
    std::fs::write(&legacy, vec![0u8; 64]).unwrap();
    assert!(manager.forget_snapshot(&references[1]));
    assert!(manager.forget_legacy_snapshot("fixture", "session-1"));
    assert!(!legacy.exists());
    let left = runtime_snapshots(dir.path());
    assert_eq!(
        left,
        vec![references[2]["snapshot_id"].as_str().unwrap().to_owned()]
    );
    // Removing the runtime removes the rest.
    assert!(manager.remove("fixture").unwrap());
    assert!(runtime_snapshots(dir.path()).is_empty());
}
#[test]
fn opening_the_manager_sweeps_homes_a_crashed_daemon_left() {
    let base = std::env::temp_dir().join("aidex-rt");
    std::fs::create_dir_all(&base).unwrap();
    let crashed = tempfile::Builder::new()
        .prefix("d-")
        .tempdir_in(&base)
        .unwrap()
        .keep();
    std::fs::create_dir_all(crashed.join("aidex-runtime-x/home")).unwrap();
    std::fs::write(
        crashed.join("aidex-runtime-x/home/auth.json"),
        br#"{"tokens":{"access_token":"left-by-a-crash"}}"#,
    )
    .unwrap();
    std::fs::File::create(crashed.join("daemon.lock"))
        .unwrap()
        .set_modified(std::time::SystemTime::now() - Duration::from_secs(3600))
        .unwrap();
    // A daemon that is still running keeps its homes.
    let (_dir, manager, request) = setup(Driver::Acp, "approval");
    let executor = manager.executors().unwrap().remove(0);
    pause_for_approval(&executor, &request, &CancelToken::new());
    assert!(!crashed.exists());
    let (_other_dir, _other, _, _) = setup_with_vault(Driver::Codex, "normal");
    assert_eq!(manager.temporary_homes(), 1);
    assert_eq!(manager.release_paused(), 1);
}
#[test]
fn the_engine_releases_and_forgets_through_the_executor_contract() {
    let (dir, manager, request) = setup(Driver::Acp, "approval");
    let executor: Arc<dyn RuntimeExecutor> = manager.executors().unwrap().remove(0);
    let (paused, _) = pause_for_approval(&executor, &request, &CancelToken::new());
    assert_eq!(manager.parked_runs(), 1);
    // What the harness calls when it abandons a paused run: idempotent.
    executor.release(&request.run_id);
    executor.release(&request.run_id);
    assert_eq!(manager.parked_runs(), 0);
    assert_eq!(manager.temporary_homes(), 0);
    assert!(manager.process_registry().is_empty());
    // What it calls once nothing names a thread any more.
    let reference = paused.runtime_ref.unwrap();
    let before = runtime_snapshots(dir.path()).len();
    assert!(before > 0);
    executor.forget(&reference);
    executor.forget(&reference);
    assert_eq!(runtime_snapshots(dir.path()).len(), before - 1);
}
