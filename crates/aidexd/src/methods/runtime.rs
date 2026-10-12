//! `runtime.*` methods over external agent runtimes (`aidex-adapters`).
//!
//! Runtime configuration is literal: executables, argv, and vault secret
//! *names*. Credentials never enter these DTOs. The configuration is sealed
//! with a vault key, so everything except `runtime.catalog` needs the vault
//! unlocked; the native engine only runs runtimes [`sync_runtimes`] loaded.
use crate::{
    errors::{ErrorCode, RpcError},
    router::{Router, RouterError},
    rpc::{expect_no_params, parse_params, to_result},
    services::Services,
};
use aidex_adapters::{Driver, RuntimeAuth, RuntimeConfig, RuntimeError, RuntimeManager};
use aidex_native_sessions::SessionConfig;
use aidex_vault::VaultState;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

pub const RUNTIME_CATALOG: &str = "runtime.catalog";
pub const RUNTIME_LIST: &str = "runtime.list";
pub const RUNTIME_ADD: &str = "runtime.add";
pub const RUNTIME_UPDATE: &str = "runtime.update";
pub const RUNTIME_CONFIG_SAVE: &str = "runtime.config.save";
pub const RUNTIME_REMOVE: &str = "runtime.remove";
pub const RUNTIME_CONFIG_GET: &str = "runtime.config.get";
pub const RUNTIME_HEALTH: &str = "runtime.health";
pub const RUNTIME_MODELS: &str = "runtime.models";
pub const RUNTIME_INSTALL: &str = "runtime.install";
pub const RUNTIME_AUTH_START: &str = "runtime.auth.start";
pub const RUNTIME_AUTH_STATUS: &str = "runtime.auth.status";
pub const RUNTIME_AUTH_CANCEL: &str = "runtime.auth.cancel";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Id {
    id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Install {
    driver: Driver,
    #[serde(default)]
    source: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Auth {
    id: String,
    #[serde(default)]
    flow_id: Option<String>,
}

fn error(e: RuntimeError) -> RpcError {
    RpcError::new(
        match e.code.as_str() {
            "vault_locked" => ErrorCode::VaultLocked,
            "invalid_config" | "invalid_request" => ErrorCode::InvalidArgument,
            "not_found" => ErrorCode::NotFound,
            "conflict" | "runtime_busy" => ErrorCode::Conflict,
            "cancelled" => ErrorCode::Cancelled,
            _ => ErrorCode::Unavailable,
        },
        e.message,
    )
    .with_data(json!({"reason":e.code}))
}

/// The runtime manager, or `unavailable` when it failed to open at startup.
fn manager(services: &Services) -> Result<&Arc<RuntimeManager>, RpcError> {
    services.runtime().map_err(|reason| {
        RpcError::new(
            ErrorCode::Unavailable,
            format!("External runtimes are unavailable: {reason}"),
        )
        .with_data(json!({"reason":"runtime_unavailable"}))
    })
}

/// Whether a session runs on an external runtime rather than the native loop.
pub(crate) fn uses_external_runtime(config: &SessionConfig) -> bool {
    config
        .runtime_id
        .as_deref()
        .is_some_and(|id| id != "native")
}

/// Load every enabled configured runtime into the native engine.
///
/// A no-op (and not an error) while the vault is locked, because the runtime
/// configuration is sealed with a vault key; when no runtime configuration
/// was ever saved (reading one would create that key, so a plain unlock must
/// not); and when the runtime manager is unavailable. The engine refuses the
/// swap while an external run is active.
pub(crate) fn sync_runtimes(services: &Services) -> Result<(), String> {
    let Ok(manager) = services.runtime() else {
        return Ok(());
    };
    if services.vault().status().state != VaultState::Unlocked || !manager.has_saved_configuration()
    {
        return Ok(());
    }
    services.native().mutate_runtimes(|| {
        let executors = manager.executors().map_err(|e| e.to_string())?;
        Ok(((), executors))
    })
}

/// Load runtimes before a session that needs `config`'s external runtime is
/// created or started, unless the engine already has it. Best effort: the
/// engine reports a runtime it still lacks.
pub(crate) fn ensure_session_runtime(services: &Services, config: &SessionConfig) {
    if !uses_external_runtime(config) {
        return;
    }
    let registered = config.runtime_id.as_deref().is_some_and(|id| {
        services
            .native()
            .runtime_descriptors()
            .iter()
            .any(|descriptor| descriptor.id == id)
    });
    if !registered {
        let _ = sync_runtimes(services);
    }
}

fn provider(services: &Services, id: &str) -> Result<String, RpcError> {
    match manager(services)?.get(id).map_err(error)?.auth {
        RuntimeAuth::CodexOauth { provider_id } => Ok(provider_id),
        _ => Err(RpcError::new(
            ErrorCode::Unavailable,
            "This runtime uses an explicitly scoped vault API key. Browser login is unavailable for this adapter.",
        )
        .with_data(json!({"reason":"auth_unsupported"}))),
    }
}

pub(crate) fn register(router: &mut Router, services: &Arc<Services>) -> Result<(), RouterError> {
    // The driver catalog is static: it is served even without the manager.
    router.register(RUNTIME_CATALOG, move |_, p| {
        expect_no_params(&p)?;
        to_result(&aidex_adapters::catalog())
    })?;
    let s = services.clone();
    router.register(RUNTIME_LIST, move |_, p| {
        expect_no_params(&p)?;
        let runtimes = manager(&s)?.list().map_err(error)?;
        // Best effort: a refused swap (an external run is active) keeps the
        // runtimes the engine already has.
        let _ = sync_runtimes(&s);
        to_result(&json!({"runtimes":runtimes}))
    })?;
    for method in [RUNTIME_ADD, RUNTIME_UPDATE, RUNTIME_CONFIG_SAVE] {
        let s = services.clone();
        router.register(method, move |_, p| {
            let config: RuntimeConfig = parse_params(p)?;
            let manager = manager(&s)?;
            if method == RUNTIME_ADD
                && manager
                    .list()
                    .map_err(error)?
                    .iter()
                    .any(|c| c.id == config.id)
            {
                return Err(RpcError::new(
                    ErrorCode::Conflict,
                    "Runtime ID already exists",
                ));
            }
            if method == RUNTIME_UPDATE {
                manager.get(&config.id).map_err(error)?;
            }
            let mut domain_error = None;
            let result = s.native().mutate_runtimes(|| {
                let config = manager.save(config).map_err(|e| {
                    domain_error = Some(error(e));
                    "Runtime configuration was refused".to_owned()
                })?;
                let executors = manager.executors().map_err(|e| {
                    domain_error = Some(error(e));
                    "Runtime registry unavailable".to_owned()
                })?;
                Ok((config, executors))
            });
            match result {
                Ok(value) => to_result(&value),
                Err(message) => {
                    Err(domain_error.unwrap_or_else(|| RpcError::new(ErrorCode::Conflict, message)))
                }
            }
        })?;
    }
    let s = services.clone();
    router.register(RUNTIME_REMOVE, move |_, p| {
        let p: Id = parse_params(p)?;
        let manager = manager(&s)?;
        let mut domain_error = None;
        let removed = s
            .native()
            .mutate_runtimes(|| {
                let removed = manager.remove(&p.id).map_err(|e| {
                    domain_error = Some(error(e));
                    "Runtime removal refused".to_owned()
                })?;
                let executors = manager.executors().map_err(|e| {
                    domain_error = Some(error(e));
                    "Runtime registry unavailable".to_owned()
                })?;
                Ok((removed, executors))
            })
            .map_err(|message| {
                domain_error.unwrap_or_else(|| RpcError::new(ErrorCode::Conflict, message))
            })?;
        Ok(json!({"removed":removed}))
    })?;
    let s = services.clone();
    router.register(RUNTIME_CONFIG_GET, move |_, p| {
        let p: Id = parse_params(p)?;
        to_result(&manager(&s)?.get(&p.id).map_err(error)?)
    })?;
    let s = services.clone();
    router.register(RUNTIME_HEALTH, move |_, p| {
        let p: Id = parse_params(p)?;
        to_result(&manager(&s)?.health(&p.id).map_err(error)?)
    })?;
    let s = services.clone();
    router.register(RUNTIME_MODELS, move |_, p| {
        let p: Id = parse_params(p)?;
        Ok(json!({"models":manager(&s)?.models(&p.id).map_err(error)?}))
    })?;
    let s = services.clone();
    router.register(RUNTIME_INSTALL, move |_, p| {
        let p: Install = parse_params(p)?;
        to_result(
            &manager(&s)?
                .install(p.driver, p.source.as_deref())
                .map_err(error)?,
        )
    })?;
    let s = services.clone();
    router.register(RUNTIME_AUTH_START, move |_, p| {
        let p: Id = parse_params(p)?;
        let provider = provider(&s, &p.id)?;
        to_result(
            &s.models()
                .auth()
                .start(&provider, |_, _| Ok(()))
                .map_err(|e| RpcError::new(ErrorCode::Unavailable, e.to_string()))?,
        )
    })?;
    let s = services.clone();
    router.register(RUNTIME_AUTH_STATUS, move |_, p| {
        let p: Auth = parse_params(p)?;
        let provider = provider(&s, &p.id)?;
        let status = s
            .models()
            .auth()
            .status(p.flow_id.as_deref(), Some(&provider))
            .map_err(|e| RpcError::new(ErrorCode::Unavailable, e.to_string()))?;
        if status.provider_id != provider {
            return Err(RpcError::invalid_params(
                "Flow does not belong to runtime account",
            ));
        }
        to_result(&status)
    })?;
    let s = services.clone();
    router.register(RUNTIME_AUTH_CANCEL, move |_, p| {
        let p: Auth = parse_params(p)?;
        let provider = provider(&s, &p.id)?;
        let flow = p
            .flow_id
            .ok_or_else(|| RpcError::invalid_params("flow_id required"))?;
        let status = s
            .models()
            .auth()
            .status(Some(&flow), None)
            .map_err(|e| RpcError::new(ErrorCode::Unavailable, e.to_string()))?;
        if status.provider_id != provider {
            return Err(RpcError::invalid_params(
                "Flow does not belong to runtime account",
            ));
        }
        to_result(
            &s.models()
                .auth()
                .cancel(&flow)
                .map_err(|e| RpcError::new(ErrorCode::Unavailable, e.to_string()))?,
        )
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
    use aidex_vault::SecretString;
    use serde_json::Value;

    const PASSPHRASE: &str = "fixture passphrase only";

    fn fixture() -> (tempfile::TempDir, Arc<Services>, Router, CallContext) {
        let drive = crate::services::testing::temp_drive();
        let shutdown = ShutdownSignal::new();
        let services = Arc::new(Services::open(drive.path(), shutdown.clone()).unwrap());
        let context = CallContext::detached(ClientInfo::new("runtime-test", "1"), shutdown);
        let mut router = Router::new();
        register(&mut router, &services).unwrap();
        (drive, services, router, context)
    }

    fn engine_runtimes(services: &Services) -> Vec<String> {
        services
            .native()
            .runtime_descriptors()
            .into_iter()
            .map(|descriptor| descriptor.id)
            .collect()
    }

    #[test]
    fn runtime_rpc_catalog_is_finite_and_config_requires_vault() {
        let (_drive, services, router, context) = fixture();
        assert!(services.runtime().is_ok());
        let catalog = router.call(&context, "runtime.catalog", json!({})).unwrap();
        assert_eq!(catalog.as_array().unwrap().len(), 10);
        assert!(router.call(&context, "runtime.list", json!({})).is_err());
        assert!(
            router
                .call(&context, "runtime.install", json!({"driver":"codex"}))
                .is_err()
        );
        assert!(
            router
                .call(
                    &context,
                    "runtime.add",
                    json!({"id":"a","driver":"codex","executable":"codex.exe","token":"forbidden"})
                )
                .is_err()
        );
    }

    #[test]
    fn configured_runtimes_reach_the_engine_and_report_a_missing_binary() {
        let (drive, services, mut router, context) = fixture();
        crate::methods::vault::register(&mut router, &services).unwrap();
        services
            .vault()
            .init(&SecretString::new(PASSPHRASE.into()), false)
            .unwrap();
        // Offline: probing the runtime's models must not launch anything.
        services.models().offline().set_offline(true).unwrap();
        let listed = router.call(&context, RUNTIME_LIST, Value::Null).unwrap();
        assert_eq!(listed, json!({"runtimes":[]}));

        let executable = drive.path().join("missing").join("codex.exe");
        let config = json!({
            "id":"codex-fixture",
            "driver":"codex",
            "executable":executable.to_string_lossy(),
        });
        let added = router.call(&context, RUNTIME_ADD, config.clone()).unwrap();
        assert_eq!(added["id"], "codex-fixture");
        assert_eq!(added["enabled"], true);
        assert_eq!(added["auth"], json!({"kind":"none"}));
        assert_eq!(
            router
                .call(&context, RUNTIME_ADD, config.clone())
                .unwrap_err()
                .kind(),
            Some(ErrorCode::Conflict)
        );
        assert_eq!(engine_runtimes(&services), ["codex-fixture"]);
        let listed = router.call(&context, RUNTIME_LIST, Value::Null).unwrap();
        assert_eq!(listed["runtimes"][0]["id"], "codex-fixture");

        let health = router
            .call(&context, RUNTIME_HEALTH, json!({"id":"codex-fixture"}))
            .unwrap();
        assert_eq!(health["status"], "binary_missing");
        assert_eq!(health["installed"], false);
        assert_eq!(health["protocol_ready"], false);
        let models = router
            .call(&context, RUNTIME_MODELS, json!({"id":"codex-fixture"}))
            .unwrap_err();
        assert_eq!(models.kind(), Some(ErrorCode::Unavailable));
        assert_eq!(models.data.unwrap()["reason"], "offline_refused");
        assert_eq!(
            router
                .call(&context, RUNTIME_HEALTH, json!({"id":"absent"}))
                .unwrap_err()
                .kind(),
            Some(ErrorCode::NotFound)
        );

        // A daemon restart leaves the engine empty until the vault unlocks.
        services.native().synchronize_runtimes(Vec::new()).unwrap();
        services.vault().lock();
        assert_eq!(sync_runtimes(&services), Ok(()));
        assert!(engine_runtimes(&services).is_empty());
        router
            .call(&context, "vault.unlock", json!({"passphrase":PASSPHRASE}))
            .unwrap();
        assert_eq!(engine_runtimes(&services), ["codex-fixture"]);
        // `runtime.list` also loads them lazily.
        services.native().synchronize_runtimes(Vec::new()).unwrap();
        router.call(&context, RUNTIME_LIST, Value::Null).unwrap();
        assert_eq!(engine_runtimes(&services), ["codex-fixture"]);

        let mut disabled = config.clone();
        disabled["enabled"] = json!(false);
        router.call(&context, RUNTIME_UPDATE, disabled).unwrap();
        assert!(engine_runtimes(&services).is_empty());
        assert_eq!(
            router
                .call(&context, RUNTIME_REMOVE, json!({"id":"codex-fixture"}))
                .unwrap(),
            json!({"removed":true})
        );
        assert_eq!(
            router.call(&context, RUNTIME_LIST, Value::Null).unwrap(),
            json!({"runtimes":[]})
        );
    }

    #[test]
    fn the_reserved_native_id_is_refused_before_anything_is_saved() {
        let (drive, services, router, context) = fixture();
        services
            .vault()
            .init(&SecretString::new(PASSPHRASE.into()), false)
            .unwrap();
        services.models().offline().set_offline(true).unwrap();
        let executable = drive.path().join("missing").join("codex.exe");
        let config = |id: &str| {
            json!({
                "id":id,
                "driver":"codex",
                "executable":executable.to_string_lossy(),
            })
        };
        for method in [RUNTIME_ADD, RUNTIME_CONFIG_SAVE] {
            let refused = router.call(&context, method, config("native")).unwrap_err();
            assert_eq!(refused.kind(), Some(ErrorCode::InvalidArgument));
            assert_eq!(refused.data.unwrap()["reason"], "invalid_config");
        }
        assert!(services.runtime().unwrap().list().unwrap().is_empty());
        assert_eq!(sync_runtimes(&services), Ok(()));

        // Runtime changes and syncs keep working afterwards.
        router
            .call(&context, RUNTIME_ADD, config("codex-fixture"))
            .unwrap();
        services.native().synchronize_runtimes(Vec::new()).unwrap();
        assert_eq!(sync_runtimes(&services), Ok(()));
        assert_eq!(engine_runtimes(&services), ["codex-fixture"]);
    }

    #[test]
    fn unlocking_a_vault_without_runtimes_adds_no_secret() {
        let (_drive, services, mut router, context) = fixture();
        crate::methods::vault::register(&mut router, &services).unwrap();
        router
            .call(&context, "vault.init", json!({"passphrase":PASSPHRASE}))
            .unwrap();
        router.call(&context, "vault.lock", Value::Null).unwrap();
        router
            .call(&context, "vault.unlock", json!({"passphrase":PASSPHRASE}))
            .unwrap();
        assert_eq!(
            router
                .call(&context, "vault.secret.list", Value::Null)
                .unwrap(),
            json!({"secrets":[]})
        );
        assert!(engine_runtimes(&services).is_empty());
    }

    #[test]
    fn external_runtime_sessions_load_their_runtime_before_starting() {
        let (drive, services, mut router, context) = fixture();
        crate::methods::native::register(&mut router, &services).unwrap();
        services
            .vault()
            .init(&SecretString::new(PASSPHRASE.into()), false)
            .unwrap();
        services.models().offline().set_offline(true).unwrap();
        let workspace_dir = tempfile::tempdir().unwrap();
        let workspace = services
            .workspaces()
            .add_host(workspace_dir.path(), None)
            .unwrap();
        let executable = drive.path().join("missing").join("codex.exe");
        router
            .call(
                &context,
                RUNTIME_ADD,
                json!({"id":"codex-fixture","driver":"codex","executable":executable.to_string_lossy()}),
            )
            .unwrap();
        let session = json!({
            "runtime_id":"codex-fixture",
            "workspace_id":workspace.id,
            "provider_id":aidex_native_sessions::RUNTIME_PROVIDER_ID,
            "model_id":aidex_native_sessions::RUNTIME_DEFAULT_MODEL,
            "mode":"agent",
            "permission_mode":"full_access",
        });

        services.native().synchronize_runtimes(Vec::new()).unwrap();
        let created = router
            .call(&context, "conversation.create", session)
            .unwrap();
        assert_eq!(engine_runtimes(&services), ["codex-fixture"]);
        let id = created["id"].as_str().unwrap().to_owned();

        services.native().synchronize_runtimes(Vec::new()).unwrap();
        let run = router
            .call(
                &context,
                "message.send",
                json!({"session_id":id,"content":"hello","command_id":"send-1"}),
            )
            .unwrap();
        assert_eq!(engine_runtimes(&services), ["codex-fixture"]);
        // The run itself fails (offline, no binary); wait so nothing outlives the drive.
        let run_id = run["id"].as_str().unwrap().to_owned();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            let status = services.native().store().run(&run_id).unwrap().status;
            if !status.active() {
                break;
            }
            assert!(std::time::Instant::now() < until, "run stayed {status:?}");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}
