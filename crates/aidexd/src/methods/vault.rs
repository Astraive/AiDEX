//! `vault.*` methods over the drive's encrypted vault (`aidex-vault`).
//!
//! Passphrases, recovery keys, and secret values are accepted only over the
//! authenticated local IPC connection. They are moved straight into zeroizing
//! containers and never appear in results, error messages, or logs: parameter
//! errors are reported without serde's value-echoing detail. Handle issuance and
//! resolution are deliberately not RPC methods (the broker calls `VaultService`).
//!
//! Records AiDEX itself owns (the native record key, the runtime state key,
//! preferences, and `oauth.*` sign-ins) are listed under `managed` and refused by
//! `vault.secret.set` and `vault.secret.delete`: deleting the native record key,
//! for example, would make every native conversation undecryptable.

use crate::{
    errors::{ErrorCode, RpcError},
    methods::record_event,
    router::{Router, RouterError},
    rpc::{expect_no_params, to_result},
    services::Services,
};
use aidex_events::{EventKind, EventScope};
use aidex_vault::{
    ConsumerScope, RecoveryKey, SecretBytes, SecretKind, SecretName, SecretPolicy, SecretString,
    VaultError, VaultErrorKind, VaultService,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value, json};
use std::sync::Arc;

pub use aidex_vault::{LockOutcome, SecretMeta, UnlockMethod, VaultState, VaultStatus};

pub const STATUS: &str = "vault.status";
pub const INIT: &str = "vault.init";
pub const UNLOCK: &str = "vault.unlock";
pub const LOCK: &str = "vault.lock";
pub const SECRET_SET: &str = "vault.secret.set";
pub const SECRET_LIST: &str = "vault.secret.list";
pub const SECRET_DELETE: &str = "vault.secret.delete";
pub const PASSPHRASE_CHANGE: &str = "vault.passphrase.change";

/// Params of `vault.init`. Not `Debug`: it carries the passphrase.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InitParams {
    passphrase: String,
    #[serde(default)]
    recovery: bool,
}

/// Result of `vault.init`. `recovery_key` is present only when requested and is
/// returned exactly once; the daemon keeps no copy.
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InitResult {
    pub state: VaultState,
    pub vault_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_key: Option<String>,
}

impl std::fmt::Debug for InitResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InitResult")
            .field("state", &self.state)
            .field("vault_id", &self.vault_id)
            .field(
                "recovery_key",
                &self.recovery_key.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

/// Params of `vault.unlock`: exactly one of `passphrase` or `recovery_key`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UnlockParams {
    #[serde(default)]
    passphrase: Option<String>,
    #[serde(default)]
    recovery_key: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetParams {
    name: String,
    value: String,
    #[serde(default)]
    consumers: Vec<String>,
    #[serde(default)]
    kind: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteParams {
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PassphraseChangeParams {
    #[serde(default)]
    old_passphrase: Option<String>,
    new_passphrase: String,
}

/// Result of `vault.secret.list`. `managed` names the listed records AiDEX owns
/// ([`managed_secret`]); `vault.secret.set` and `vault.secret.delete` refuse them.
/// It is always serialized (empty when none is stored).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SecretList {
    pub secrets: Vec<SecretMeta>,
    #[serde(default)]
    pub managed: Vec<SecretName>,
}

/// Prefix of the account sign-in records `aidex-auth` keeps
/// (`oauth.<provider>.access`, `oauth.<provider>.profile`, `oauth.hosts`).
const OAUTH_SECRET_PREFIX: &str = "oauth.";

/// Whether `name` is a record AiDEX itself owns: the native record encryption
/// key, the external runtime state key, the app, model and appearance
/// preferences, or an `oauth.*` account sign-in. Only the owning services write
/// or remove these.
pub fn managed_secret(name: &str) -> bool {
    [
        aidex_native_sessions::NATIVE_RECORD_KEY_SECRET,
        aidex_adapters::RUNTIME_STATE_KEY_SECRET,
        aidex_preferences::PREFERENCES_SECRET,
        aidex_preferences::MODEL_PREFERENCES_SECRET,
        aidex_preferences::APPEARANCE_SECRET,
    ]
    .contains(&name)
        || name.starts_with(OAUTH_SECRET_PREFIX)
}

/// The refusal of a generic write or delete of a managed record, before the
/// vault is touched: `policy_denied` with `data.reason = "managed_secret"`.
fn managed_refusal() -> RpcError {
    RpcError::new(
        ErrorCode::PolicyDenied,
        "This vault record is managed by AiDEX and cannot be changed or deleted here",
    )
    .with_data(json!({"reason": "managed_secret"}))
}

/// Result of `vault.secret.delete`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DeleteResult {
    pub deleted: SecretMeta,
}

/// Result of `vault.lock`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LockResult {
    pub state: VaultState,
    pub was_unlocked: bool,
    pub revoked_handles: usize,
}

/// Result of `vault.passphrase.change`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PassphraseChanged {
    pub changed: bool,
}

/// Map a vault error to its wire error. Wrong credentials and a locked vault use
/// `vault_locked` (-32020); `data.reason` carries the stable vault error kind.
pub fn vault_error(error: &VaultError) -> RpcError {
    wire_error(error, false)
}

/// [`vault_error`] for an operation on the unlocked vault. When the error ended the
/// session (`conflict`, or an uncertain commit of the vault's own header), the vault
/// is now locked and `data.session_dropped` is `true`: the client must unlock again.
pub fn vault_session_error(vault: &VaultService, error: &VaultError) -> RpcError {
    wire_error(error, vault.ends_session(error))
}

fn wire_error(error: &VaultError, session_dropped: bool) -> RpcError {
    let kind = error.kind();
    let rpc = match kind {
        VaultErrorKind::WrongPassphrase
        | VaultErrorKind::WrongRecoveryKey
        | VaultErrorKind::Locked => RpcError::new(ErrorCode::VaultLocked, error.to_string()),
        _ => RpcError::service(error.code(), error.to_string()),
    };
    let mut data = json!({ "reason": kind.as_str() });
    if session_dropped {
        data["session_dropped"] = Value::Bool(true);
    }
    rpc.with_data(data)
}

/// Lock the vault on the user's request (`vault.lock`, `emergency.stop` with
/// `lock_vault`). Active runs are cancelled first, while the record key is still
/// available, so they are recorded as cancelled: native runs, and external runs
/// whose thread drives their vendor process. An external run paused for an
/// approval is not: its parked launch is released (process ended, temporary home
/// removed) and the unlock settles the run as `interrupted` (its approval
/// expired), exactly as after the idle auto-lock, so `run.resume` continues it
/// from its saved reference (a paused child whose parent is executing is
/// cancelled with that parent). Ending the session then runs the daemon's lock
/// cleanup (`services/vault_lock.rs`: uploads aborted, runtime logins cancelled
/// and vendor processes ended, `vault.locked` recorded once) before this
/// returns. `emergency.stop` has already cancelled every run, paused ones
/// included, before it locks.
pub(crate) fn lock_vault(services: &Services) -> LockOutcome {
    services.native().cancel_all_but_paused();
    services.vault().lock()
}

/// After `vault.init` or `vault.unlock`: settle runs a lock left without a
/// final state (cancelled while the record key was gone, or waiting on an
/// approval whose plan the lock dropped) as `interrupted`, then load the
/// configured runtimes. Both are best effort and never fail the unlock: an
/// unsettled run is settled by the next unlock or the next daemon start, and
/// runtime conversations also load their runtime lazily.
fn after_unlock(services: &Services) {
    if services.native().reconcile_after_unlock().is_err() {
        // Only a fixed line: the error can name run content.
        eprintln!("aidexd: some runs could not be settled after the vault unlocked");
    }
    let _ = super::runtime::sync_runtimes(services);
}

/// Decode params whose fields may hold secrets: serde messages such as
/// "invalid type: string \"...\"" can echo values, so only missing-field names (which
/// come from our own schema) are reported.
fn secret_params<T: DeserializeOwned>(
    method: &str,
    params: Value,
    shape: &str,
) -> Result<T, RpcError> {
    let params = match params {
        Value::Null => Value::Object(Map::new()),
        other => other,
    };
    serde_json::from_value(params).map_err(|error| {
        let detail = error.to_string();
        let missing = detail
            .strip_prefix("missing field `")
            .and_then(|rest| rest.split('`').next())
            .filter(|field| shape.contains(field));
        let message = match missing {
            Some(field) => format!(
                "invalid parameters for `{method}`: missing field `{field}`; expected {shape}"
            ),
            None => format!("invalid parameters for `{method}`; expected {shape}"),
        };
        RpcError::invalid_params(message)
    })
}

pub(crate) fn register(router: &mut Router, services: &Arc<Services>) -> Result<(), RouterError> {
    let shared = Arc::clone(services);
    router.register_passive(STATUS, move |_context, params| {
        expect_no_params(&params)?;
        to_result(&shared.vault().status())
    })?;

    let shared = Arc::clone(services);
    router.register(INIT, move |_context, params| {
        let params: InitParams =
            secret_params(INIT, params, "{passphrase: string, recovery?: bool}")?;
        let passphrase = SecretString::new(params.passphrase);
        let outcome = shared
            .vault()
            .init(&passphrase, params.recovery)
            .map_err(|error| vault_error(&error))?;
        record_event(
            &shared,
            EventScope::global(),
            EventKind::VaultUnlocked {
                vault_id: outcome.vault_id.clone(),
            },
        )?;
        after_unlock(&shared);
        to_result(&InitResult {
            state: VaultState::Unlocked,
            vault_id: outcome.vault_id.to_string(),
            recovery_key: outcome
                .recovery_key
                .as_ref()
                .map(|key| key.encode().expose().to_owned()),
        })
    })?;

    let shared = Arc::clone(services);
    router.register(UNLOCK, move |_context, params| {
        let shape = "{passphrase: string} or {recovery_key: string}";
        let params: UnlockParams = secret_params(UNLOCK, params, shape)?;
        let status = match (params.passphrase, params.recovery_key) {
            (Some(passphrase), None) => shared.vault().unlock(&SecretString::new(passphrase)),
            (None, Some(text)) => {
                let text = SecretString::new(text);
                let key = RecoveryKey::parse(text.expose()).map_err(|error| vault_error(&error))?;
                shared.vault().unlock_with_recovery(&key)
            }
            _ => {
                return Err(RpcError::invalid_params(format!(
                    "invalid parameters for `{UNLOCK}`; expected {shape}"
                )));
            }
        }
        .map_err(|error| vault_error(&error))?;
        if let Some(vault_id) = &status.vault_id {
            record_event(
                &shared,
                EventScope::global(),
                EventKind::VaultUnlocked {
                    vault_id: vault_id.clone(),
                },
            )?;
        }
        // The runtime configuration is sealed with a vault key, so the native
        // engine learns the configured runtimes only now.
        after_unlock(&shared);
        to_result(&status)
    })?;

    let shared = Arc::clone(services);
    router.register(LOCK, move |_context, params| {
        expect_no_params(&params)?;
        let outcome = lock_vault(&shared);
        to_result(&LockResult {
            state: shared.vault().status().state,
            was_unlocked: outcome.was_unlocked,
            revoked_handles: outcome.revoked_handles,
        })
    })?;

    let shared = Arc::clone(services);
    router.register(SECRET_SET, move |_context, params| {
        let params: SetParams = secret_params(
            SECRET_SET,
            params,
            "{name: string, value: string, consumers?: [string], kind?: string}",
        )?;
        let value = SecretBytes::from(params.value);
        if managed_secret(&params.name) {
            return Err(managed_refusal());
        }
        let name = SecretName::new(params.name).map_err(|error| vault_error(&error))?;
        let kind = match params.kind.as_deref() {
            Some(kind) => SecretKind::parse(kind).map_err(|error| vault_error(&error))?,
            None => SecretKind::Generic,
        };
        let consumers = params
            .consumers
            .iter()
            .map(|scope| ConsumerScope::parse(scope))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| vault_error(&error))?;
        let policy = SecretPolicy::new(consumers).map_err(|error| vault_error(&error))?;
        let meta = shared
            .vault()
            .put_secret(&name, kind, value, policy)
            .map_err(|error| vault_session_error(shared.vault(), &error))?;
        to_result(&meta)
    })?;

    let shared = Arc::clone(services);
    router.register_passive(SECRET_LIST, move |_context, params| {
        expect_no_params(&params)?;
        let secrets = shared
            .vault()
            .list_secrets()
            .map_err(|error| vault_session_error(shared.vault(), &error))?;
        let managed = secrets
            .iter()
            .filter(|meta| managed_secret(meta.name.as_str()))
            .map(|meta| meta.name.clone())
            .collect();
        to_result(&SecretList { secrets, managed })
    })?;

    let shared = Arc::clone(services);
    router.register(SECRET_DELETE, move |_context, params| {
        let params: DeleteParams = secret_params(SECRET_DELETE, params, "{name: string}")?;
        if managed_secret(&params.name) {
            return Err(managed_refusal());
        }
        let name = SecretName::new(params.name).map_err(|error| vault_error(&error))?;
        let deleted = shared
            .vault()
            .delete_secret(&name)
            .map_err(|error| vault_session_error(shared.vault(), &error))?;
        to_result(&DeleteResult { deleted })
    })?;

    let shared = Arc::clone(services);
    router.register(PASSPHRASE_CHANGE, move |_context, params| {
        let params: PassphraseChangeParams = secret_params(
            PASSPHRASE_CHANGE,
            params,
            "{old_passphrase?: string, new_passphrase: string}",
        )?;
        let old = params.old_passphrase.map(SecretString::new);
        let new = SecretString::new(params.new_passphrase);
        shared
            .vault()
            .change_passphrase(old.as_ref(), &new)
            .map_err(|error| vault_session_error(shared.vault(), &error))?;
        to_result(&PassphraseChanged { changed: true })
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        lifecycle::ShutdownSignal,
        router::{CallContext, ClientInfo},
    };
    use aidex_core::EntityId;
    use aidex_events::{Cursor, EventFilter};
    use aidex_vault::{
        KdfParams, Vault,
        testing::{WriteFailure, fail_next_write},
    };
    use std::{
        thread,
        time::{Duration, Instant},
    };

    const PASSPHRASE: &str = "correct horse battery staple";
    const VALUE: &str = "PLAINTEXT-MARKER-VALUE-91ab";

    fn fixture() -> (tempfile::TempDir, Arc<Services>, Router, CallContext) {
        let root = crate::services::testing::temp_drive();
        let shutdown = ShutdownSignal::new();
        let services = Arc::new(Services::open(root.path(), shutdown.clone()).unwrap());
        services
            .vault()
            .set_kdf_params(KdfParams::new(8, 1, 1).unwrap());
        let mut router = Router::new();
        register(&mut router, &services).unwrap();
        let context = CallContext::detached(ClientInfo::new("test", "1"), shutdown);
        (root, services, router, context)
    }

    fn call(
        router: &Router,
        context: &CallContext,
        method: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        router.call(context, method, params)
    }

    #[test]
    fn vault_lives_in_the_drive_vault_directory() {
        let (root, services, router, context) = fixture();
        let status: VaultStatus =
            serde_json::from_value(call(&router, &context, STATUS, Value::Null).unwrap()).unwrap();
        assert_eq!(status.state, VaultState::Missing);
        call(&router, &context, INIT, json!({"passphrase": PASSPHRASE})).unwrap();
        assert_eq!(services.vault().dir(), services.paths().vault());
        assert!(services.paths().vault().join("vault.json").is_file());
        assert!(
            root.path()
                .join("private")
                .join("vault")
                .join("vault.json")
                .is_file(),
            "the default layout stores the vault under private/vault"
        );
    }

    #[test]
    fn secret_bearing_params_are_never_echoed_in_errors() {
        let (_root, _services, router, context) = fixture();
        for (method, params) in [
            (INIT, json!({"passphrase": 98765432123_u64})),
            (
                INIT,
                json!({"passphrase": PASSPHRASE, "recovery": "98765432123"}),
            ),
            (INIT, json!({"passphrase": PASSPHRASE, "98765432123": true})),
            (UNLOCK, json!({"passphrase": ["98765432123"]})),
            (
                UNLOCK,
                json!({"passphrase": "98765432123", "recovery_key": "98765432123"}),
            ),
            (SECRET_SET, json!({"name": "a", "value": 98765432123_u64})),
            (
                SECRET_SET,
                json!({"name": "a", "value": {"x": "98765432123"}}),
            ),
            (
                PASSPHRASE_CHANGE,
                json!({"new_passphrase": 98765432123_u64}),
            ),
        ] {
            let error = call(&router, &context, method, params).unwrap_err();
            assert_eq!(error.kind(), Some(ErrorCode::InvalidParams), "{method}");
            let text = serde_json::to_string(&error).unwrap();
            assert!(!text.contains("98765432123"), "{method}: {text}");
        }
        let error = call(&router, &context, INIT, json!({})).unwrap_err();
        assert!(
            error.message.contains("missing field `passphrase`"),
            "{}",
            error.message
        );
    }

    #[test]
    fn wrong_credentials_and_locked_vaults_map_to_vault_locked() {
        let (_root, _services, router, context) = fixture();
        call(&router, &context, INIT, json!({"passphrase": PASSPHRASE})).unwrap();
        call(&router, &context, LOCK, Value::Null).unwrap();
        let locked = call(&router, &context, SECRET_LIST, Value::Null).unwrap_err();
        assert_eq!(locked.kind(), Some(ErrorCode::VaultLocked));
        assert_eq!(locked.data, Some(json!({"reason": "locked"})));
        let wrong = call(
            &router,
            &context,
            UNLOCK,
            json!({"passphrase": "not the passphrase"}),
        )
        .unwrap_err();
        assert_eq!(wrong.kind(), Some(ErrorCode::VaultLocked));
        assert_eq!(wrong.data, Some(json!({"reason": "wrong_passphrase"})));
        assert!(!wrong.message.contains("not the passphrase"));
        let set = call(
            &router,
            &context,
            SECRET_SET,
            json!({"name": "openai", "value": VALUE, "consumers": ["provider:openai"]}),
        )
        .unwrap_err();
        assert_eq!(set.kind(), Some(ErrorCode::VaultLocked));
        assert!(!serde_json::to_string(&set).unwrap().contains(VALUE));
    }

    #[test]
    fn set_validates_names_kinds_and_consumers() {
        let (_root, _services, router, context) = fixture();
        call(&router, &context, INIT, json!({"passphrase": PASSPHRASE})).unwrap();
        for (params, reason) in [
            (
                json!({"name": "bad name", "value": VALUE}),
                "invalid_argument",
            ),
            (
                json!({"name": "a", "value": VALUE, "kind": "cookie"}),
                "invalid_argument",
            ),
            (
                json!({"name": "a", "value": VALUE, "consumers": ["model:gpt"]}),
                "forbidden_consumer",
            ),
            (
                json!({"name": "a", "value": VALUE, "consumers": ["env:HOME"]}),
                "forbidden_consumer",
            ),
            (
                json!({"name": "a", "value": VALUE, "consumers": ["bogus"]}),
                "invalid_argument",
            ),
            (json!({"name": "a", "value": ""}), "invalid_argument"),
        ] {
            let error = call(&router, &context, SECRET_SET, params.clone()).unwrap_err();
            assert_eq!(error.data, Some(json!({"reason": reason})), "{params}");
            assert!(!serde_json::to_string(&error).unwrap().contains(VALUE));
        }
        let meta: SecretMeta = serde_json::from_value(
            call(
                &router,
                &context,
                SECRET_SET,
                json!({"name": "openai", "value": VALUE, "consumers": ["provider:openai"], "kind": "api_key"}),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(meta.kind, SecretKind::ApiKey);
        let listed = call(&router, &context, SECRET_LIST, Value::Null).unwrap();
        assert!(!listed.to_string().contains(VALUE));
    }

    fn init(router: &Router, context: &CallContext) -> EntityId {
        let init: InitResult = serde_json::from_value(
            call(router, context, INIT, json!({"passphrase": PASSPHRASE})).unwrap(),
        )
        .unwrap();
        EntityId::new(init.vault_id).unwrap()
    }

    /// `vault.*` events recorded after `after`.
    fn vault_events(services: &Services, after: Cursor) -> Vec<EventKind> {
        let filter = EventFilter::all().with_kind("vault");
        let page = services.events().replay(&filter, after, 100).unwrap();
        page.events
            .into_iter()
            .map(|envelope| envelope.event)
            .collect()
    }

    /// Wait (generously, for slow CI runners) until `count` `vault.*` events were
    /// recorded after `after`, then check that no further one follows.
    fn settled_vault_events(services: &Services, after: Cursor, count: usize) -> Vec<EventKind> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while vault_events(services, after).len() < count {
            assert!(
                Instant::now() < deadline,
                "{:?}",
                vault_events(services, after)
            );
            thread::sleep(Duration::from_millis(10));
        }
        thread::sleep(Duration::from_millis(100));
        let events = vault_events(services, after);
        assert_eq!(events.len(), count, "{events:?}");
        events
    }

    #[test]
    fn explicit_locks_record_vault_locked_once_before_returning() {
        let (_root, services, router, context) = fixture();
        let vault_id = init(&router, &context);
        let after = services.events().latest_cursor().unwrap();
        let first: LockResult =
            serde_json::from_value(call(&router, &context, LOCK, Value::Null).unwrap()).unwrap();
        assert!(first.was_unlocked);
        assert_eq!(
            vault_events(&services, after),
            [EventKind::VaultLocked {
                vault_id: vault_id.clone()
            }]
        );
        let second: LockResult =
            serde_json::from_value(call(&router, &context, LOCK, Value::Null).unwrap()).unwrap();
        assert!(!second.was_unlocked);
        assert_eq!(
            settled_vault_events(&services, after, 1),
            [EventKind::VaultLocked { vault_id }]
        );
    }

    #[test]
    fn emergency_stop_with_lock_vault_records_vault_locked_once() {
        let (_root, services, mut router, context) = fixture();
        crate::methods::emergency::register(&mut router, &services).unwrap();
        let vault_id = init(&router, &context);
        let after = services.events().latest_cursor().unwrap();
        let stop = || {
            call(
                &router,
                &context,
                crate::methods::emergency::STOP,
                json!({"lock_vault": true}),
            )
            .unwrap()
        };
        assert_eq!(stop()["vault_locked"], true);
        assert_eq!(services.vault().status().state, VaultState::Locked);
        // Recorded by the lock cleanup before the call returned, and only there.
        assert_eq!(
            vault_events(&services, after),
            [EventKind::VaultLocked {
                vault_id: vault_id.clone()
            }]
        );
        // Stopping again with the vault already locked records nothing more.
        assert_eq!(stop()["vault_locked"], true);
        assert_eq!(
            settled_vault_events(&services, after, 1),
            [EventKind::VaultLocked { vault_id }]
        );
    }

    #[test]
    fn idle_auto_lock_runs_the_lock_cleanup_once() {
        let (_root, services, router, context) = fixture();
        let vault_id = init(&router, &context);
        let after = services.events().latest_cursor().unwrap();
        services
            .vault()
            .set_idle_timeout(Some(Duration::from_millis(50)));
        assert_eq!(
            settled_vault_events(&services, after, 1),
            [EventKind::VaultLocked {
                vault_id: vault_id.clone()
            }]
        );
        assert_eq!(services.vault().status().state, VaultState::Locked);

        services.vault().set_idle_timeout(None);
        call(&router, &context, UNLOCK, json!({"passphrase": PASSPHRASE})).unwrap();
        assert_eq!(
            vault_events(&services, after),
            [
                EventKind::VaultLocked {
                    vault_id: vault_id.clone()
                },
                EventKind::VaultUnlocked { vault_id }
            ]
        );
    }

    #[test]
    fn dropped_sessions_report_session_dropped_and_run_the_lock_cleanup_once() {
        let (_root, services, router, context) = fixture();
        let vault_id = init(&router, &context);
        call(
            &router,
            &context,
            SECRET_SET,
            json!({"name": "openai", "value": VALUE, "consumers": ["provider:openai"]}),
        )
        .unwrap();
        let after = services.events().latest_cursor().unwrap();
        // Another writer commits behind the daemon's session.
        let mut other = Vault::open(services.vault().dir())
            .unwrap()
            .unlock(&SecretString::from(PASSPHRASE.to_owned()))
            .unwrap();
        other.delete(&SecretName::new("openai").unwrap()).unwrap();
        drop(other);

        let conflict =
            call(&router, &context, SECRET_DELETE, json!({"name": "openai"})).unwrap_err();
        assert_eq!(
            conflict.data,
            Some(json!({"reason": "conflict", "session_dropped": true}))
        );
        assert_eq!(
            settled_vault_events(&services, after, 1),
            [EventKind::VaultLocked {
                vault_id: vault_id.clone()
            }]
        );
        // Later calls see a plainly locked vault until the client unlocks again.
        let locked = call(&router, &context, SECRET_LIST, Value::Null).unwrap_err();
        assert_eq!(locked.data, Some(json!({"reason": "locked"})));
        call(&router, &context, UNLOCK, json!({"passphrase": PASSPHRASE})).unwrap();
        let listed: SecretList =
            serde_json::from_value(call(&router, &context, SECRET_LIST, Value::Null).unwrap())
                .unwrap();
        assert!(listed.secrets.is_empty());
        assert_eq!(
            vault_events(&services, after),
            [
                EventKind::VaultLocked {
                    vault_id: vault_id.clone()
                },
                EventKind::VaultUnlocked { vault_id }
            ]
        );
    }

    #[test]
    fn an_uncertain_header_commit_drops_the_session_and_runs_the_lock_cleanup_once() {
        let (_root, services, router, context) = fixture();
        let vault_id = init(&router, &context);
        call(
            &router,
            &context,
            SECRET_SET,
            json!({"name": "openai", "value": VALUE, "consumers": ["provider:openai"]}),
        )
        .unwrap();
        let after = services.events().latest_cursor().unwrap();
        // The new header is renamed into place, then syncing its directory fails.
        fail_next_write(services.vault().dir(), WriteFailure::AfterRename);
        let uncertain =
            call(&router, &context, SECRET_DELETE, json!({"name": "openai"})).unwrap_err();
        assert_eq!(
            uncertain.data,
            Some(json!({"reason": "io", "session_dropped": true}))
        );
        assert!(!uncertain.message.contains(VALUE));
        assert_eq!(
            settled_vault_events(&services, after, 1),
            [EventKind::VaultLocked { vault_id }]
        );
        assert_eq!(services.vault().status().state, VaultState::Locked);
        let locked = call(&router, &context, SECRET_LIST, Value::Null).unwrap_err();
        assert_eq!(locked.data, Some(json!({"reason": "locked"})));
    }

    /// The managed records the test seeds as their owners would (besides the
    /// native record key, which a conversation creates).
    const SEEDED: [&str; 6] = [
        "external-runtime-encryption-key",
        "aidex-preferences",
        "aidex-model-preferences",
        "aidex-appearance",
        "oauth.openai.access",
        "oauth.hosts",
    ];

    fn listed(router: &Router, context: &CallContext) -> SecretList {
        serde_json::from_value(call(router, context, SECRET_LIST, Value::Null).unwrap()).unwrap()
    }

    #[test]
    fn managed_records_are_listed_and_refused_by_set_and_delete() {
        let (_root, services, mut router, context) = fixture();
        crate::methods::native::register(&mut router, &services).unwrap();
        init(&router, &context);
        let owner = ConsumerScope::process("fixture-owner").unwrap();
        for name in SEEDED {
            services
                .vault()
                .put_secret(
                    &SecretName::new(name).unwrap(),
                    SecretKind::Generic,
                    SecretBytes::from(format!("{VALUE}-{name}")),
                    SecretPolicy::new([owner.clone()]).unwrap(),
                )
                .unwrap();
        }
        // A conversation creates the native record key and seals its history.
        let session = services
            .native()
            .store()
            .create(
                serde_json::from_value(json!({"provider_id": "fixture", "model_id": "small"}))
                    .unwrap(),
            )
            .unwrap();
        services
            .native()
            .store()
            .update(&session.id, |detail| {
                detail
                    .messages
                    .push(aidex_providers::ChatMessage::user("sealed history"));
                Ok(())
            })
            .unwrap();
        call(
            &router,
            &context,
            SECRET_SET,
            json!({"name": "openai", "value": VALUE, "consumers": ["provider:openai"]}),
        )
        .unwrap();

        let list = listed(&router, &context);
        let mut expected: Vec<&str> = SEEDED.to_vec();
        expected.push(aidex_native_sessions::NATIVE_RECORD_KEY_SECRET);
        expected.sort_unstable();
        let mut managed: Vec<&str> = list.managed.iter().map(SecretName::as_str).collect();
        managed.sort_unstable();
        assert_eq!(managed, expected);
        assert_eq!(list.secrets.len(), expected.len() + 1);
        assert!(managed.iter().all(|name| managed_secret(name)));
        assert!(!managed_secret("openai"));

        let before = list.secrets.clone();
        // Also a managed name that does not exist yet.
        for name in expected.iter().copied().chain(["oauth.other.access"]) {
            for (method, params) in [
                (
                    SECRET_SET,
                    json!({"name": name, "value": VALUE, "consumers": ["provider:openai"]}),
                ),
                (SECRET_DELETE, json!({"name": name})),
            ] {
                let error = call(&router, &context, method, params).unwrap_err();
                assert_eq!(
                    error.kind(),
                    Some(ErrorCode::PolicyDenied),
                    "{method} {name}"
                );
                assert_eq!(error.data, Some(json!({"reason": "managed_secret"})));
                assert!(!serde_json::to_string(&error).unwrap().contains(VALUE));
            }
        }
        assert_eq!(listed(&router, &context).secrets, before, "nothing changed");
        // History sealed with the record key still decrypts.
        let view = router
            .call(&context, "conversation.get", json!({"id": session.id}))
            .unwrap();
        assert_eq!(view["messages"][0]["content"], "sealed history");

        // Ordinary secrets still change and delete.
        call(
            &router,
            &context,
            SECRET_SET,
            json!({"name": "openai", "value": "replacement value", "consumers": []}),
        )
        .unwrap();
        call(&router, &context, SECRET_DELETE, json!({"name": "openai"})).unwrap();
        let after = listed(&router, &context);
        assert_eq!(after.secrets.len(), expected.len());
        assert_eq!(after.managed.len(), expected.len());
    }

    /// `vault.secret.list` has exactly the keys of `SecretList` and `SecretMeta`
    /// in packages/client/src/dto/vault.ts.
    #[test]
    fn secret_list_keys_match_the_client_types() {
        let (_root, _services, router, context) = fixture();
        init(&router, &context);
        let keys = |value: &Value| {
            let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
            keys.sort();
            keys
        };
        assert_eq!(
            call(&router, &context, SECRET_LIST, Value::Null).unwrap(),
            json!({"secrets": [], "managed": []}),
            "both lists are always present"
        );
        call(
            &router,
            &context,
            SECRET_SET,
            json!({"name": "openai", "value": VALUE, "consumers": ["provider:openai"]}),
        )
        .unwrap();
        let list = call(&router, &context, SECRET_LIST, Value::Null).unwrap();
        assert_eq!(keys(&list), ["managed", "secrets"]);
        let meta = &list["secrets"][0];
        assert_eq!(
            keys(meta),
            [
                "created_at",
                "kind",
                "name",
                "policy",
                "secret_ref",
                "updated_at"
            ]
        );
        assert_eq!(keys(&meta["secret_ref"]), ["secret_id", "vault_id"]);
        assert_eq!(keys(&meta["policy"]), ["allowed_consumers"]);
    }

    #[test]
    fn only_errors_that_ended_the_session_report_session_dropped() {
        let (root, services, _router, _context) = fixture();
        let vault = services.vault();
        let uncertain = |path: std::path::PathBuf| VaultError::CommitUncertain {
            path,
            source: std::io::Error::other("directory sync failed"),
        };
        let own = uncertain(vault.dir().join("vault.json"));
        assert_eq!(
            vault_session_error(vault, &own).data,
            Some(json!({"reason": "io", "session_dropped": true}))
        );
        assert_eq!(vault_error(&own).data, Some(json!({"reason": "io"})));
        let backup = uncertain(root.path().join("backup").join("vault.json"));
        assert_eq!(
            vault_session_error(vault, &backup).data,
            Some(json!({"reason": "io"})),
            "a backup destination's uncertain commit leaves the session alone"
        );
        assert_eq!(
            vault_session_error(vault, &VaultError::Conflict).data,
            Some(json!({"reason": "conflict", "session_dropped": true}))
        );
        assert_eq!(
            vault_session_error(vault, &VaultError::Locked).data,
            Some(json!({"reason": "locked"}))
        );
    }
}
