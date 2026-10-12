use super::*;
use aidex_vault::{SecretString, VaultServiceConfig};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use rsa::{RsaPrivateKey, pkcs8::EncodePrivateKey, traits::PublicKeyParts};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
};
use url::Url;

#[test]
fn unavailable_host_identity_leaves_service_usable_and_denies_signin_before_network() {
    let (_root, vault) = vault();
    let admissions = Arc::new(AtomicUsize::new(0));
    let observed = admissions.clone();
    let mut service = AuthService::with_network_gate(
        vault,
        Arc::new(move || {
            observed.fetch_add(1, Ordering::SeqCst);
            true
        }),
    )
    .unwrap();
    service.host_key = Err(AuthError::Unavailable);
    let service = Arc::new(service);
    assert!(matches!(
        service.start("openai", |_, _| Ok(())),
        Err(AuthError::Unavailable)
    ));
    assert_eq!(admissions.load(Ordering::SeqCst), 1);
    assert!(service.flows.lock().unwrap().is_empty());
}

struct Fixture {
    base: String,
    nonce: Arc<Mutex<String>>,
    challenge: Arc<Mutex<String>>,
    redirect_uri: Arc<Mutex<String>>,
    vault_probe: Arc<Mutex<Option<Arc<VaultService>>>>,
    defect: Arc<Mutex<String>>,
    refreshes: Arc<AtomicUsize>,
    connections: Arc<AtomicUsize>,
    stop: Arc<AtomicUsize>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Fixture {
    fn new() -> Self {
        let key = RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
        let jwk = json!({"kty":"RSA","kid":"fixture","alg":"RS256","use":"sig", "n":URL_SAFE_NO_PAD.encode(key.n().to_bytes_be()), "e":URL_SAFE_NO_PAD.encode(key.e().to_bytes_be())});
        let signing =
            EncodingKey::from_rsa_pem(key.to_pkcs8_pem(Default::default()).unwrap().as_bytes())
                .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let nonce = Arc::new(Mutex::new(String::new()));
        let challenge = Arc::new(Mutex::new(String::new()));
        let redirect_uri = Arc::new(Mutex::new(String::new()));
        let server_challenge = challenge.clone();
        let server_redirect_uri = redirect_uri.clone();
        let vault_probe = Arc::new(Mutex::new(None::<Arc<VaultService>>));
        let server_vault_probe = vault_probe.clone();
        let defect = Arc::new(Mutex::new(String::new()));
        let refreshes = Arc::new(AtomicUsize::new(0));
        let connections = Arc::new(AtomicUsize::new(0));
        let server_connections = connections.clone();
        let stop = Arc::new(AtomicUsize::new(0));
        let (server_base, server_nonce, server_defect, server_refreshes, server_stop) = (
            base.clone(),
            nonce.clone(),
            defect.clone(),
            refreshes.clone(),
            stop.clone(),
        );
        let worker = thread::spawn(move || {
            while server_stop.load(Ordering::SeqCst) == 0 {
                let Ok((mut stream, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                server_connections.fetch_add(1, Ordering::SeqCst);
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut input = Vec::new();
                let mut buf = [0; 4096];
                loop {
                    let n = stream.read(&mut buf).unwrap();
                    if n == 0 {
                        break;
                    }
                    input.extend_from_slice(&buf[..n]);
                    if let Some(split) = input.windows(4).position(|w| w == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&input[..split]);
                        let len = header
                            .lines()
                            .find_map(|l| {
                                l.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|s| s.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if input.len() >= split + 4 + len {
                            break;
                        }
                    }
                }
                let text = String::from_utf8_lossy(&input);
                let path = text.split_whitespace().nth(1).unwrap();
                let defect = server_defect.lock().unwrap().clone();
                let mut status = "200 OK";
                let result = if path == "/discovery" {
                    json!({"issuer":server_base,"authorization_endpoint":format!("{server_base}/authorize"),"token_endpoint":format!("{server_base}/token"),"jwks_uri":format!("{server_base}/jwks"),"revocation_endpoint":format!("{server_base}/revoke")})
                } else if path == "/jwks" {
                    json!({"keys":[jwk]})
                } else if path == "/revoke" {
                    json!({})
                } else if path == "/token" {
                    let body = text.split("\r\n\r\n").nth(1).unwrap_or("");
                    let params: std::collections::HashMap<_, _> =
                        url::form_urlencoded::parse(body.as_bytes())
                            .into_owned()
                            .collect();
                    assert_eq!(params.get("resource").unwrap(), "https://api.openai.com/v1");
                    assert_eq!(params.get("client_id").unwrap(), "oaiapp_fixture");
                    if params["grant_type"] == "refresh_token" {
                        let previous = server_refreshes.fetch_add(1, Ordering::SeqCst);
                        assert_eq!(
                            params["refresh_token"],
                            if previous == 0 || defect == "omit_refresh" {
                                "fixture-refresh"
                            } else {
                                "fixture-refresh-rotated"
                            }
                        );
                        if defect == "revoked" {
                            status = "400 Bad Request";
                            json!({"error":"invalid_grant"})
                        } else {
                            json!({"access_token":"fixture-access-refreshed", "refresh_token":if defect=="omit_refresh" {Value::Null} else {json!("fixture-refresh-rotated")}, "token_type":"Bearer", "expires_in":3600,"scope":if defect=="scope_refresh" {"openid"} else {"openid resource.invoke chatgpt.tokens.use.direct"}})
                        }
                    } else {
                        let saved = crate::storage::read_profile(
                            server_vault_probe.lock().unwrap().as_ref().unwrap(),
                            "openai",
                        )
                        .unwrap()
                        .unwrap();
                        assert_eq!(
                            saved.client_id, "oaiapp_fixture",
                            "registration must be saved before exchange"
                        );
                        use sha2::{Digest, Sha256};
                        assert_eq!(
                            URL_SAFE_NO_PAD
                                .encode(Sha256::digest(params["code_verifier"].as_bytes())),
                            *server_challenge.lock().unwrap()
                        );
                        assert_eq!(params["redirect_uri"], *server_redirect_uri.lock().unwrap());
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs();
                        let claims = json!({"sub":"fixture-user","email":"fixture@example.test","iss":if defect=="issuer" {"http://wrong"} else {&server_base},"aud":if defect=="audience" {"wrong"} else {"oaiapp_fixture"}, "exp":if defect=="expiry" {1} else {now+3600},"nonce":if defect=="nonce" {"wrong".into()} else {server_nonce.lock().unwrap().clone()}});
                        let mut header = Header::new(Algorithm::RS256);
                        header.kid = Some("fixture".into());
                        let mut id = jsonwebtoken::encode(&header, &claims, &signing).unwrap();
                        if defect == "signature" {
                            let n = id.len() - 10;
                            let ch = if &id[n..n + 1] == "a" { "b" } else { "a" };
                            id.replace_range(n..n + 1, ch);
                        }
                        json!({"access_token":"fixture-access", "refresh_token":"fixture-refresh", "id_token":id,"token_type":"Bearer", "expires_in":if defect=="short" {1} else {3600},"scope":if defect=="scope" {"openid"} else {"openid resource.invoke chatgpt.tokens.use.direct"}})
                    }
                } else {
                    json!({})
                };
                let result = if path == "/token" && defect == "token_http" {
                    status = "400 Bad Request";
                    json!({"error":"invalid_grant","description":"fixture-secret-do-not-display"})
                } else if path == "/token" && defect == "token_json" {
                    json!({"unexpected":"fixture-secret-do-not-display"})
                } else if path == "/jwks" && defect == "jwks_http" {
                    status = "503 Service Unavailable";
                    json!({"error":"fixture-secret-do-not-display"})
                } else {
                    result
                };
                let output = result.to_string();
                write!(stream,"HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",output.len(),output).unwrap();
            }
        });
        Self {
            base,
            nonce,
            challenge,
            redirect_uri,
            vault_probe,
            defect,
            refreshes,
            connections,
            stop,
            worker: Some(worker),
        }
    }
    fn service(&self, vault: Arc<VaultService>) -> Arc<AuthService> {
        *self.vault_probe.lock().unwrap() = Some(vault.clone());
        let mut service = AuthService::new(vault).unwrap();
        service.discovery = format!("{}/discovery", self.base);
        service.host_key = Ok("fixture-host-one".into());
        service.network_allowed = Arc::new(|| true);
        Arc::new(service)
    }
    fn callback(&self, start: &StartResult, defect: &str) {
        let url = Url::parse(&start.authorization_url).unwrap();
        let params: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        *self.nonce.lock().unwrap() = params["nonce"].clone();
        *self.challenge.lock().unwrap() = params["code_challenge"].clone();
        *self.redirect_uri.lock().unwrap() = params["redirect_uri"].clone();
        let mut callback = Url::parse(&params["redirect_uri"]).unwrap();
        callback
            .query_pairs_mut()
            .append_pair(
                "state",
                if defect == "state" {
                    "wrong"
                } else {
                    &params["state"]
                },
            )
            .append_pair(
                "client_id",
                if defect == "client" {
                    "dynamic_agent_client"
                } else {
                    "oaiapp_fixture"
                },
            );
        if defect == "denied" {
            callback
                .query_pairs_mut()
                .append_pair("error", "access_denied");
        } else {
            callback
                .query_pairs_mut()
                .append_pair("code", "fixture-code");
        }
        reqwest::blocking::get(callback).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(1, Ordering::SeqCst);
        let _ = self.worker.take().unwrap().join();
    }
}
fn vault() -> (tempfile::TempDir, Arc<VaultService>) {
    let dir = tempfile::tempdir().unwrap();
    let vault = Arc::new(
        VaultService::open(dir.path().join("vault"), VaultServiceConfig::default()).unwrap(),
    );
    vault
        .init(&SecretString::new("fixture passphrase only".into()), false)
        .unwrap();
    (dir, vault)
}
fn await_state(auth: &AuthService, flow: &str) -> AuthStatus {
    for _ in 0..150 {
        let result = auth.status(Some(flow), None).unwrap();
        if result.state != AuthState::Pending {
            return result;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("fixture flow remained pending");
}

#[test]
fn valid_oauth_callback_persists_scoped_tokens_and_metadata_only_status() {
    let fixture = Fixture::new();
    let (dir, vault) = vault();
    let auth = fixture.service(vault.clone());
    let start = auth.start("openai", |_, _| Ok(())).unwrap();
    let url = Url::parse(&start.authorization_url).unwrap();
    let params: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(params["client_id"], "dynamic_agent_client");
    assert_eq!(params["agent_name_hint"], "AiDEX");
    assert_eq!(params["code_challenge_method"], "S256");
    assert!(params["ext_agent_host_id"].starts_with("urn:uuid:"));
    fixture.callback(&start, "");
    let status = await_state(&auth, &start.flow_id);
    assert_eq!(status.state, AuthState::Connected);
    assert_eq!(
        status.account_label.as_deref(),
        Some("fixture@example.test")
    );
    let encoded = serde_json::to_string(&status).unwrap();
    assert!(!encoded.contains("fixture-access"));
    assert!(!encoded.contains("fixture-refresh"));
    assert_eq!(
        auth.resolve("openai", "oauth.openai.access")
            .unwrap()
            .expose(),
        b"fixture-access"
    );
    vault.lock();
    let reopened = Arc::new(
        VaultService::open(dir.path().join("vault"), VaultServiceConfig::default()).unwrap(),
    );
    reopened
        .unlock(&SecretString::new("fixture passphrase only".into()))
        .unwrap();
    let restarted = fixture.service(reopened);
    assert_eq!(
        restarted.status(None, Some("openai")).unwrap().state,
        AuthState::Connected
    );
    let returning = restarted.start("openai", |_, _| Ok(())).unwrap();
    let url = Url::parse(&returning.authorization_url).unwrap();
    let pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(pairs["client_id"], "oaiapp_fixture");
    assert!(!pairs.contains_key("agent_name_hint"));
    assert!(!pairs.contains_key("id_token_hint"));
    restarted.cancel(&returning.flow_id).unwrap();
}

#[test]
fn rejects_callback_and_identity_attacks_without_activating_credentials() {
    let fixture = Fixture::new();
    for defect in [
        "state",
        "client",
        "denied",
        "nonce",
        "issuer",
        "audience",
        "signature",
        "expiry",
        "scope",
    ] {
        let (_dir, vault) = vault();
        let auth = fixture.service(vault);
        *fixture.defect.lock().unwrap() = defect.into();
        let start = auth.start("openai", |_, _| Ok(())).unwrap();
        fixture.callback(&start, defect);
        let status = await_state(&auth, &start.flow_id);
        assert_eq!(status.state, AuthState::Failed, "{defect}");
        assert!(
            auth.resolve("openai", "oauth.openai.access").is_err(),
            "{defect}"
        );
        let encoded = serde_json::to_string(&status).unwrap();
        assert!(!encoded.contains("fixture-code"));
        assert!(!encoded.contains("fixture-access"));
    }
}

#[test]
fn locked_vault_cannot_start_or_release_oauth_credentials() {
    let fixture = Fixture::new();
    let (_dir, vault) = vault();
    let auth = fixture.service(vault.clone());
    vault.lock();
    assert_eq!(
        auth.start("openai", |_, _| Ok(())).unwrap_err(),
        AuthError::Locked
    );
    assert!(matches!(
        auth.resolve("openai", "oauth.openai.access"),
        Err(AuthError::Locked)
    ));
}

#[test]
fn cancellation_and_expiry_close_the_callback_listener() {
    let fixture = Fixture::new();
    let (_dir, vault) = vault();
    let auth = fixture.service(vault.clone());
    let start = auth.start("openai", |_, _| Ok(())).unwrap();
    assert_eq!(
        auth.cancel(&start.flow_id).unwrap().state,
        AuthState::Cancelled
    );
    thread::sleep(Duration::from_millis(150));
    let url = Url::parse(&start.authorization_url).unwrap();
    let callback = url
        .query_pairs()
        .find(|(k, _)| k == "redirect_uri")
        .unwrap()
        .1
        .into_owned();
    assert!(reqwest::blocking::get(callback).is_err());
    let mut expiring = AuthService::new(vault).unwrap();
    expiring.host_key = Ok("fixture-host-one".into());
    expiring.network_allowed = Arc::new(|| true);
    expiring.discovery = format!("{}/discovery", fixture.base);
    expiring.ttl = Duration::from_millis(30);
    let expiring = Arc::new(expiring);
    let start = expiring.start("openai", |_, _| Ok(())).unwrap();
    assert_eq!(
        await_state(&expiring, &start.flow_id).state,
        AuthState::Expired
    );
}

#[test]
fn concurrent_requests_rotate_once_and_keep_new_refresh_token_after_restart() {
    let fixture = Fixture::new();
    *fixture.defect.lock().unwrap() = "short".into();
    let (_dir, vault) = vault();
    let auth = fixture.service(vault.clone());
    let start = auth.start("openai", |_, _| Ok(())).unwrap();
    fixture.callback(&start, "");
    assert_eq!(
        await_state(&auth, &start.flow_id).state,
        AuthState::Connected
    );
    let workers: Vec<_> = (0..8)
        .map(|_| {
            let auth = auth.clone();
            thread::spawn(move || auth.resolve("openai", "oauth.openai.access").unwrap())
        })
        .collect();
    for worker in workers {
        assert_eq!(worker.join().unwrap().expose(), b"fixture-access-refreshed");
    }
    assert_eq!(fixture.refreshes.load(Ordering::SeqCst), 1);
    let restarted = fixture.service(vault);
    assert_eq!(
        restarted
            .resolve("openai", "oauth.openai.access")
            .unwrap()
            .expose(),
        b"fixture-access-refreshed"
    );
    assert_eq!(
        restarted.disconnect("openai").unwrap().state,
        AuthState::Disconnected
    );
    assert!(restarted.resolve("openai", "oauth.openai.access").is_err());
    let reauth = restarted.start("openai", |_, _| Ok(())).unwrap();
    assert!(
        reauth
            .authorization_url
            .contains("client_id=oaiapp_fixture")
    );
    restarted.cancel(&reauth.flow_id).unwrap();
}

#[test]
fn revoked_refresh_clears_tokens_and_invalidates_connected_flow_status() {
    let fixture = Fixture::new();
    *fixture.defect.lock().unwrap() = "short".into();
    let (_dir, vault) = vault();
    let auth = fixture.service(vault.clone());
    let start = auth.start("openai", |_, _| Ok(())).unwrap();
    fixture.callback(&start, "");
    assert_eq!(
        await_state(&auth, &start.flow_id).state,
        AuthState::Connected
    );
    *fixture.defect.lock().unwrap() = "revoked".into();
    assert!(matches!(
        auth.resolve("openai", "oauth.openai.access"),
        Err(AuthError::Reauthorize)
    ));
    assert_eq!(
        auth.status(Some(&start.flow_id), None).unwrap().state,
        AuthState::Disconnected
    );
    assert!(crate::storage::read_access(&vault, "openai").is_err());
}

#[test]
fn refresh_omission_retains_previous_rotation_and_earliest_refresh_is_enforced() {
    let fixture = Fixture::new();
    let (_dir, vault) = vault();
    let auth = fixture.service(vault.clone());
    let start = auth.start("openai", |_, _| Ok(())).unwrap();
    fixture.callback(&start, "");
    assert_eq!(
        await_state(&auth, &start.flow_id).state,
        AuthState::Connected
    );
    let mut profile = crate::storage::read_profile(&vault, "openai")
        .unwrap()
        .unwrap();
    profile.tokens.as_mut().unwrap().expires_at = crate::oauth::now();
    profile.tokens.as_mut().unwrap().earliest_refresh_at = Some(crate::oauth::now() + 600);
    crate::storage::write_profile(&vault, "openai", &profile).unwrap();
    assert!(matches!(
        auth.resolve("openai", "oauth.openai.access"),
        Err(AuthError::Unavailable)
    ));
    assert_eq!(fixture.refreshes.load(Ordering::SeqCst), 0);
    profile.tokens.as_mut().unwrap().earliest_refresh_at = None;
    crate::storage::write_profile(&vault, "openai", &profile).unwrap();
    *fixture.defect.lock().unwrap() = "omit_refresh".into();
    assert_eq!(
        auth.resolve("openai", "oauth.openai.access")
            .unwrap()
            .expose(),
        b"fixture-access-refreshed"
    );
    let saved = crate::storage::read_profile(&vault, "openai")
        .unwrap()
        .unwrap();
    assert_eq!(
        saved.tokens.as_ref().unwrap().refresh_token.as_deref(),
        Some("fixture-refresh")
    );
}

#[test]
fn host_id_mapping_is_portable_and_provider_values_remain_scoped() {
    let (_dir, vault) = vault();
    let first = crate::storage::host_id(&vault, "host-one").unwrap();
    let second = crate::storage::host_id(&vault, "host-two").unwrap();
    assert_ne!(first, second);
    assert_eq!(crate::storage::host_id(&vault, "host-one").unwrap(), first);
    assert!(!first.contains("host-one"));
    crate::storage::write_access(&vault, "openai", b"fixture-private-token").unwrap();
    let name = aidex_vault::SecretName::new("oauth.openai.access").unwrap();
    assert!(
        vault
            .issue_handle(
                &name,
                &aidex_vault::ConsumerScope::provider("other").unwrap(),
                Duration::from_secs(10)
            )
            .is_err()
    );
}

#[test]
fn failed_provider_activation_preserves_previously_active_account() {
    let fixture = Fixture::new();
    let (_dir, vault) = vault();
    let auth = fixture.service(vault);
    let start = auth.start("openai", |_, _| Ok(())).unwrap();
    fixture.callback(&start, "");
    assert_eq!(
        await_state(&auth, &start.flow_id).state,
        AuthState::Connected
    );
    let reauth = auth
        .start("openai", |_, _| Err(AuthError::Unavailable))
        .unwrap();
    fixture.callback(&reauth, "");
    assert_eq!(await_state(&auth, &reauth.flow_id).state, AuthState::Failed);
    assert_eq!(
        auth.resolve("openai", "oauth.openai.access")
            .unwrap()
            .expose(),
        b"fixture-access"
    );
}

#[test]
fn refresh_loss_of_plan_consent_requires_reauthorization() {
    let fixture = Fixture::new();
    *fixture.defect.lock().unwrap() = "short".into();
    let (_dir, vault) = vault();
    let auth = fixture.service(vault.clone());
    let start = auth.start("openai", |_, _| Ok(())).unwrap();
    fixture.callback(&start, "");
    assert_eq!(
        await_state(&auth, &start.flow_id).state,
        AuthState::Connected
    );
    *fixture.defect.lock().unwrap() = "scope_refresh".into();
    assert!(matches!(
        auth.resolve("openai", "oauth.openai.access"),
        Err(AuthError::PlanPermission)
    ));
    assert_eq!(
        auth.status(Some(&start.flow_id), None).unwrap().state,
        AuthState::Disconnected
    );
    assert!(crate::storage::read_access(&vault, "openai").is_err());
}

#[test]
fn fractional_earliest_refresh_never_refreshes_before_server_time() {
    for (value, expected) in [
        (json!(1790036132.5), 1790036133u64),
        (json!("2026-09-22T00:15:32.500Z"), 1790036133u64),
    ] {
        let response:crate::oauth::TokenResponse=serde_json::from_value(json!({"access_token":"fixture-access","token_type":"Bearer","expires_in":3600,"scope":"resource.invoke chatgpt.tokens.use.direct","earliest_refresh_at":value})).unwrap();
        assert_eq!(
            response.into_tokens(None).unwrap().earliest_refresh_at,
            Some(expected)
        );
    }
}

#[test]
fn offline_gate_denies_all_auth_sockets_and_browser_open_but_disconnects_locally() {
    use std::sync::atomic::AtomicBool;
    let fixture = Fixture::new();
    let (_dir, vault) = vault();
    let allowed = Arc::new(AtomicBool::new(false));
    let policy = allowed.clone();
    let mut service = AuthService::new(vault.clone()).unwrap();
    service.host_key = Ok("fixture-host-one".into());
    service.discovery = format!("{}/discovery", fixture.base);
    service.network_allowed = Arc::new(move || policy.load(Ordering::SeqCst));
    let auth = Arc::new(service);
    *fixture.vault_probe.lock().unwrap() = Some(vault.clone());
    assert_eq!(
        auth.start("openai", |_, _| Ok(())).unwrap_err(),
        AuthError::NetworkOffline
    );
    assert_eq!(fixture.connections.load(Ordering::SeqCst), 0);
    allowed.store(true, Ordering::SeqCst);
    let start = auth.start("openai", |_, _| Ok(())).unwrap();
    allowed.store(false, Ordering::SeqCst);
    assert_eq!(
        auth.open(&start.authorization_url),
        Err(AuthError::NetworkOffline)
    );
    let before = fixture.connections.load(Ordering::SeqCst);
    fixture.callback(&start, "");
    assert_eq!(await_state(&auth, &start.flow_id).state, AuthState::Failed);
    assert!(crate::storage::read_access(&vault, "openai").is_err());
    assert_eq!(fixture.connections.load(Ordering::SeqCst), before);
    allowed.store(true, Ordering::SeqCst);
    *fixture.defect.lock().unwrap() = "short".into();
    let start = auth.start("openai", |_, _| Ok(())).unwrap();
    fixture.callback(&start, "");
    assert_eq!(
        await_state(&auth, &start.flow_id).state,
        AuthState::Connected
    );
    allowed.store(false, Ordering::SeqCst);
    let before = fixture.connections.load(Ordering::SeqCst);
    assert!(matches!(
        auth.resolve("openai", "oauth.openai.access"),
        Err(AuthError::NetworkOffline)
    ));
    assert_eq!(fixture.refreshes.load(Ordering::SeqCst), 0);
    let status = auth.disconnect("openai").unwrap();
    assert_eq!(status.state, AuthState::Disconnected);
    assert!(
        status
            .error
            .unwrap()
            .contains("Remote revocation was not confirmed")
    );
    assert!(crate::storage::read_access(&vault, "openai").is_err());
    assert_eq!(fixture.connections.load(Ordering::SeqCst), before);
}

#[test]
fn exchange_failures_identify_the_boundary_without_exposing_response_bodies() {
    for (defect, expected) in [
        ("token_http", "HTTP 400"),
        ("token_json", "unsupported token response"),
        ("jwks_http", "verification keys"),
    ] {
        let fixture = Fixture::new();
        *fixture.defect.lock().unwrap() = defect.into();
        let (_dir, vault) = vault();
        let auth = fixture.service(vault);
        let start = auth.start("openai", |_, _| Ok(())).unwrap();
        fixture.callback(&start, "");
        let status = await_state(&auth, &start.flow_id);
        assert_eq!(status.state, AuthState::Failed);
        let error = status.error.unwrap();
        assert!(error.contains(expected), "{defect}: {error}");
        assert!(!error.contains("fixture-secret"));
    }
}

#[test]
fn fresh_account_registration_does_not_reuse_another_workspaces_client() {
    let fixture = Fixture::new();
    *fixture.defect.lock().unwrap() = "token_http".into();
    let (_dir, vault) = vault();
    let auth = fixture.service(vault.clone());
    let first = auth.start("openai", |_, _| Ok(())).unwrap();
    fixture.callback(&first, "");
    assert_eq!(await_state(&auth, &first.flow_id).state, AuthState::Failed);
    let original = storage::read_profile(&vault, "openai").unwrap().unwrap();
    assert_eq!(original.client_id, "oaiapp_fixture");
    let new = auth.start("openai-personal", |_, _| Ok(())).unwrap();
    let url = Url::parse(&new.authorization_url).unwrap();
    let params: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(params["client_id"], "dynamic_agent_client");
    assert_eq!(params["agent_name_hint"], "AiDEX");
    assert_eq!(
        storage::read_profile(&vault, "openai")
            .unwrap()
            .unwrap()
            .client_id,
        original.client_id
    );
    assert!(
        storage::read_profile(&vault, "openai-personal")
            .unwrap()
            .is_none()
    );
    auth.cancel(&new.flow_id).unwrap();
}
