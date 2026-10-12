//! Secret handles: a stored git credential is evaluated as `secret.use`, injected
//! only into the git child process, and never appears in events, cards, outcomes,
//! or approval records.

mod common;

use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use aidex_broker::{BrokerOutcome, Principal, approvals::ApprovalResolution};
use aidex_core::EntityId;
use aidex_vault::{
    ConsumerScope, KdfParams, SecretBytes, SecretKind, SecretName, SecretPolicy, SecretString,
    VaultService, VaultServiceConfig,
};
use common::{Fixture, Options};
use serde_json::json;

/// Base64 of `text` (test-local copy, so the assertion does not trust the code under test).
fn base64(text: &str) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = text.as_bytes();
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// A loopback HTTP server that records request headers and refuses every push.
fn recording_server() -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
            let mut buffer = Vec::new();
            let mut chunk = [0u8; 4096];
            while !buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => buffer.extend_from_slice(&chunk[..read]),
                }
            }
            recorded
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(&buffer).into_owned());
            let _ = stream.write_all(
                b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    (port, requests)
}

#[test]
fn git_push_credentials_reach_only_the_child_process() {
    let (port, requests) = recording_server();
    let vault_dir = tempfile::tempdir().unwrap();
    let vault = Arc::new(
        VaultService::open(
            vault_dir.path(),
            VaultServiceConfig {
                kdf: KdfParams::new(8, 1, 1).unwrap(),
                idle_timeout: None,
            },
        )
        .unwrap(),
    );
    vault
        .init(
            &SecretString::new("correct horse battery staple".into()),
            false,
        )
        .unwrap();
    // Generated at runtime: never a realistic-looking credential in the repository.
    let token = format!(
        "tok{}",
        EntityId::generate("t").unwrap().as_str().replace('_', "")
    );
    let consumer = ConsumerScope::git(format!("127.0.0.1:{port}")).unwrap();
    vault
        .put_secret(
            &SecretName::new("forge-token").unwrap(),
            SecretKind::Token,
            SecretBytes::from(token.clone()),
            SecretPolicy::new([consumer.clone()]).unwrap(),
        )
        .unwrap();

    let options = Options {
        vault: Some(vault.clone()),
        ..Options::default()
    };
    let fixture = Fixture::new(options);
    fixture.git_repository();
    fixture.git(&[
        "remote",
        "add",
        "origin",
        &format!("http://127.0.0.1:{port}/acme/app.git"),
    ]);

    let outcome = fixture.submit("git.push", json!({"branch": "main"}));
    let BrokerOutcome::PendingApproval {
        approval_id, card, ..
    } = &outcome
    else {
        panic!("expected approval, got {outcome:?}");
    };
    assert_eq!(card.secrets.len(), 1);
    assert_eq!(card.secrets[0].secret_name, "forge-token");
    assert_eq!(card.secrets[0].consumer, consumer.to_string());
    let rules = fixture.decision_rules();
    assert_eq!(
        rules.len(),
        2,
        "git.push and secret.use are both evaluated: {rules:?}"
    );

    fixture
        .broker
        .resolve_approval(
            approval_id,
            ApprovalResolution::ApproveOnce,
            &Principal::LocalUser,
        )
        .unwrap();
    let outcome = fixture.broker.continue_after_approval(approval_id).unwrap();
    // The fake forge refuses the push, so git fails, but it presented the credential.
    let BrokerOutcome::Failed { failure, .. } = &outcome else {
        panic!("expected git to fail against the refusing server, got {outcome:?}");
    };
    assert_eq!(failure.reason, "process_failed", "{failure:?}");
    let expected = format!("Basic {}", base64(&format!("x-access-token:{token}")));
    let seen = requests.lock().unwrap().join("\n---\n");
    assert!(
        seen.lines().any(
            |line| line.to_ascii_lowercase().starts_with("authorization:")
                && line.contains(&expected)
        ),
        "the child did not send the injected credential:\n{seen}"
    );

    // The value appears nowhere the agent, logs, or audit trail can see.
    let encoded = base64(&format!("x-access-token:{token}"));
    let record = fixture.broker.approval(approval_id).unwrap();
    let surfaces = [
        fixture.event_text(),
        serde_json::to_string(&outcome).unwrap(),
        serde_json::to_string(&record).unwrap(),
        format!("{outcome:?} {record:?} {:?}", fixture.broker),
    ];
    for surface in &surfaces {
        assert!(!surface.contains(&token), "token leaked: {surface}");
        assert!(
            !surface.contains(&encoded),
            "encoded token leaked: {surface}"
        );
    }
    let names = fixture.names();
    for expected in [
        "secret.granted",
        "secret.revoked",
        "process.started",
        "process.exited",
    ] {
        assert!(names.contains(&expected), "{names:?}");
    }
    assert!(fixture.event_text().contains("forge-token"));
}
