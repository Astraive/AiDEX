use aidex_adapters::*;
use aidex_auth::AuthService;
use aidex_core::CancelToken;
use aidex_native_sessions::*;
use aidex_sandbox::Sandbox;
use aidex_vault::{SecretString, VaultService, VaultServiceConfig};
use std::{path::PathBuf, sync::Arc, time::Duration};
fn setup(driver: Driver, scenario: &str) -> (tempfile::TempDir, RuntimeManager, RuntimeRequest) {
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
        vault,
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
    };
    (dir, manager, request)
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
    let paused=executor.execute(&request,&mut |e| {if let RuntimeEvent::ApprovalRequest{request_id,runtime_ref,tool_name,arguments,..}=e {let fingerprint=aidex_native_sessions::digest(serde_json::json!({"runtime":"fixture","session":request.session_id,"run":request.run_id,"workspace":request.workspace,"permission":request.permission_mode,"request":request_id,"tool":tool_name,"arguments":arguments,"reference":runtime_ref}).to_string().as_bytes());approval=Some((request_id.clone(),runtime_ref.clone(),fingerprint));}RuntimeControl::Pause},&CancelToken::new()).unwrap();
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
        let paused=executor.execute(&request,&mut|e|{if let RuntimeEvent::ApprovalRequest{request_id,runtime_ref,tool_name,arguments,..}=e {let fingerprint=digest(serde_json::json!({"runtime":"fixture","session":request.session_id,"run":request.run_id,"workspace":request.workspace,"permission":request.permission_mode,"request":request_id,"tool":tool_name,"arguments":arguments,"reference":runtime_ref}).to_string().as_bytes());decision=Some(RuntimeApprovalDecision{request_id:request_id.clone(),approved:false,fingerprint,runtime_ref:runtime_ref.clone()});}RuntimeControl::Pause},&cancel).unwrap();
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
            .execute(&request, &mut |_| RuntimeControl::Pause, &cancel)
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
