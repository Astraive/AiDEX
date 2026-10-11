//! `vault.*` methods over the drive's encrypted vault (`aidex-vault`).
//!
//! Passphrases, recovery keys, and secret values are accepted only over the
//! authenticated local IPC connection. They are moved straight into zeroizing
//! containers and never appear in results, error messages, or logs: parameter
//! errors are reported without serde's value-echoing detail. Handle issuance and
//! resolution are deliberately not RPC methods (the broker calls `VaultService`).

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
    VaultError, VaultErrorKind,
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

/// Result of `vault.secret.list`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SecretList {
    pub secrets: Vec<SecretMeta>,
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
    let kind = error.kind();
    let rpc = match kind {
        VaultErrorKind::WrongPassphrase
        | VaultErrorKind::WrongRecoveryKey
        | VaultErrorKind::Locked => RpcError::new(ErrorCode::VaultLocked, error.to_string()),
        _ => RpcError::service(error.code(), error.to_string()),
    };
    rpc.with_data(json!({ "reason": kind.as_str() }))
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

/// System records are owned by their dedicated services, never generic editors.
fn editable_secret(name: &SecretName) -> Result<(), RpcError> {
    if matches!(
        name.as_str(),
        "native-record-encryption-key" | "external-runtime-encryption-key" | "aidex-preferences"
    ) || name.as_str().starts_with("oauth.")
    {
        return Err(RpcError::new(
            ErrorCode::PolicyDenied,
            "This record is managed by AiDEX; use its account or settings controls",
        ));
    }
    Ok(())
}

fn revoke_credential_users(shared: &Services) {
    shared.native().cancel_all();
    shared.extensions().disconnect_all();
    let _ = shared.runtime().cancel_logins();
}

pub(crate) fn register(router: &mut Router, services: &Arc<Services>) -> Result<(), RouterError> {
    let shared = Arc::clone(services);
    router.register(STATUS, move |_context, params| {
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
        shared.native().reconcile_after_unlock().map_err(|_| {
            RpcError::new(
                ErrorCode::Unavailable,
                "Vault created, but interrupted agent state could not be reconciled",
            )
            .with_data(json!({"vault_unlocked":true,"reason":"agent_recovery_failed"}))
        })?;
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
        shared.native().reconcile_after_unlock().map_err(|_| {
            RpcError::new(
                ErrorCode::Unavailable,
                "Vault unlocked, but interrupted agent state could not be reconciled",
            )
            .with_data(json!({"vault_unlocked":true,"reason":"agent_recovery_failed"}))
        })?;
        to_result(&status)
    })?;

    let shared = Arc::clone(services);
    router.register(LOCK, move |_context, params| {
        expect_no_params(&params)?;
        shared.native().cancel_all();
        shared.extensions().disconnect_all();
        let _ = shared.runtime().cancel_logins();
        let vault_id = shared.vault().status().vault_id;
        let outcome = shared.vault().lock();
        shared.extensions().disconnect_all();
        let _ = shared.runtime().cancel_logins();
        if let (true, Some(vault_id)) = (outcome.was_unlocked, vault_id) {
            record_event(
                &shared,
                EventScope::global(),
                EventKind::VaultLocked { vault_id },
            )?;
        }
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
        let name = SecretName::new(params.name).map_err(|error| vault_error(&error))?;
        editable_secret(&name)?;
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
            .map_err(|error| vault_error(&error))?;
        revoke_credential_users(&shared);
        record_event(
            &shared,
            EventScope::global(),
            EventKind::SecretStored {
                secret_ref: meta.secret_ref.clone(),
            },
        )?;
        to_result(&meta)
    })?;

    let shared = Arc::clone(services);
    router.register(SECRET_LIST, move |_context, params| {
        expect_no_params(&params)?;
        let secrets = shared
            .vault()
            .list_secrets()
            .map_err(|error| vault_error(&error))?;
        to_result(&SecretList { secrets })
    })?;

    let shared = Arc::clone(services);
    router.register(SECRET_DELETE, move |_context, params| {
        let params: DeleteParams = secret_params(SECRET_DELETE, params, "{name: string}")?;
        let name = SecretName::new(params.name).map_err(|error| vault_error(&error))?;
        editable_secret(&name)?;
        let deleted = shared
            .vault()
            .delete_secret(&name)
            .map_err(|error| vault_error(&error))?;
        revoke_credential_users(&shared);
        record_event(
            &shared,
            EventScope::global(),
            EventKind::SecretDeleted {
                secret_ref: deleted.secret_ref.clone(),
            },
        )?;
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
            .map_err(|error| vault_error(&error))?;
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
    use aidex_vault::KdfParams;

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
}
