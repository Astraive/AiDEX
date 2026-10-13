//! Settings stored in the encrypted portable vault: `settings.*` (the
//! application preferences) and `model.preferences.*` (model picker favorites
//! and hidden models, a separate record).
use crate::{
    errors::{ErrorCode, RpcError},
    router::{Router, RouterError},
    rpc::{expect_no_params, parse_params, to_result},
    services::Services,
};
use aidex_preferences::{
    ModelPreferencesPatch, ModelPreferencesService, PreferencesError, PreferencesService,
};
use aidex_vault::VaultService;
use serde::Deserialize;
use std::sync::Arc;

pub const SETTINGS_GET: &str = "settings.get";
pub const SETTINGS_SET: &str = "settings.set";
pub const MODEL_PREFERENCES_GET: &str = "model.preferences.get";
pub const MODEL_PREFERENCES_SET: &str = "model.preferences.set";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetParams {
    patch: serde_json::Value,
}

fn error(vault: &VaultService, error: PreferencesError) -> RpcError {
    match error {
        PreferencesError::Vault(error) => super::vault::vault_session_error(vault, &error),
        PreferencesError::Invalid(message) => RpcError::invalid_params(message),
        PreferencesError::Unavailable => {
            RpcError::new(ErrorCode::InternalError, "Preference state unavailable")
        }
    }
}
pub(crate) fn register(router: &mut Router, services: &Arc<Services>) -> Result<(), RouterError> {
    let settings = Arc::new(PreferencesService::new(services.vault().clone()));
    let shared = settings.clone();
    let vault = Arc::clone(services.vault());
    router.register(SETTINGS_GET, move |_, params| {
        expect_no_params(&params)?;
        to_result(&shared.get().map_err(|e| error(&vault, e))?)
    })?;
    let vault = Arc::clone(services.vault());
    router.register(SETTINGS_SET, move |_, params| {
        let params: SetParams = parse_params(params)?;
        to_result(
            &settings
                .update(params.patch)
                .map_err(|e| error(&vault, e))?,
        )
    })?;
    let models = Arc::new(ModelPreferencesService::new(services.vault().clone()));
    let shared = models.clone();
    let vault = Arc::clone(services.vault());
    router.register(MODEL_PREFERENCES_GET, move |_, params| {
        expect_no_params(&params)?;
        to_result(&shared.get().map_err(|e| error(&vault, e))?)
    })?;
    let vault = Arc::clone(services.vault());
    router.register(MODEL_PREFERENCES_SET, move |_, params| {
        let patch: ModelPreferencesPatch = parse_params(params)?;
        to_result(&models.set(patch).map_err(|e| error(&vault, e))?)
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
    use aidex_vault::{
        ConsumerScope, KdfParams, SecretBytes, SecretKind, SecretName, SecretPolicy, SecretString,
        Vault, VaultState,
        testing::{WriteFailure, fail_next_write},
    };
    use serde_json::{Value, json};

    fn fixture() -> (tempfile::TempDir, Arc<Services>, Router, CallContext) {
        let root = crate::services::testing::temp_drive();
        let shutdown = ShutdownSignal::new();
        let services = Arc::new(Services::open(root.path(), shutdown.clone()).unwrap());
        let mut router = Router::new();
        register(&mut router, &services).unwrap();
        let context = CallContext::detached(ClientInfo::new("settings-test", "1"), shutdown);
        (root, services, router, context)
    }

    #[test]
    fn model_preferences_need_the_vault_and_update_partially() {
        let (_root, services, router, context) = fixture();
        let passphrase = SecretString::new("fixture passphrase only".into());
        services.vault().init(&passphrase, false).unwrap();
        services.vault().lock();
        let locked = router
            .call(&context, MODEL_PREFERENCES_GET, Value::Null)
            .unwrap_err();
        assert_eq!(locked.kind(), Some(ErrorCode::VaultLocked));
        services.vault().unlock(&passphrase).unwrap();

        assert_eq!(
            router
                .call(&context, MODEL_PREFERENCES_GET, Value::Null)
                .unwrap(),
            json!({"favorite_models":[],"hidden_models":[]})
        );
        let saved = router
            .call(
                &context,
                MODEL_PREFERENCES_SET,
                json!({"favorite_models":["openai/gpt-5","runtime:codex/default","openai/gpt-5"]}),
            )
            .unwrap();
        assert_eq!(
            saved,
            json!({"favorite_models":["openai/gpt-5","runtime:codex/default"],"hidden_models":[]})
        );
        let saved = router
            .call(
                &context,
                MODEL_PREFERENCES_SET,
                json!({"hidden_models":["openai/whisper-1"]}),
            )
            .unwrap();
        assert_eq!(
            saved,
            json!({"favorite_models":["openai/gpt-5","runtime:codex/default"],"hidden_models":["openai/whisper-1"]})
        );
        assert_eq!(
            router
                .call(&context, MODEL_PREFERENCES_GET, json!({}))
                .unwrap(),
            saved
        );

        let too_many: Vec<String> = (0..65).map(|i| format!("p/model-{i}")).collect();
        for refused in [
            json!({"favorite_models":too_many}),
            json!({"favorite_models":["no-slash"]}),
            json!({"hidden_models":["a/\u{7}bell"]}),
            json!({"favorites":["a/b"]}),
        ] {
            let error = router
                .call(&context, MODEL_PREFERENCES_SET, refused.clone())
                .unwrap_err();
            assert_eq!(error.kind(), Some(ErrorCode::InvalidParams), "{refused}");
        }
        assert_eq!(
            router
                .call(&context, MODEL_PREFERENCES_GET, Value::Null)
                .unwrap(),
            saved
        );
        // The application preferences are a separate record.
        assert!(
            router
                .call(&context, SETTINGS_GET, Value::Null)
                .unwrap()
                .get("favorite_models")
                .is_none()
        );
        services.vault().lock();
        let locked = router
            .call(&context, MODEL_PREFERENCES_SET, json!({"hidden_models":[]}))
            .unwrap_err();
        assert_eq!(locked.kind(), Some(ErrorCode::VaultLocked));
    }

    #[test]
    fn writes_that_drop_the_vault_session_report_session_dropped() {
        let (_root, services, router, context) = fixture();
        let passphrase = SecretString::new("fixture passphrase only".into());
        services
            .vault()
            .set_kdf_params(KdfParams::new(8, 1, 1).unwrap());
        services.vault().init(&passphrase, false).unwrap();
        // The first record also creates the records directory.
        router
            .call(&context, SETTINGS_SET, json!({"patch": {}}))
            .unwrap();

        // The new header is renamed into place, then syncing its directory fails.
        fail_next_write(services.vault().dir(), WriteFailure::AfterRename);
        let uncertain = router
            .call(&context, MODEL_PREFERENCES_SET, json!({"hidden_models":[]}))
            .unwrap_err();
        assert_eq!(
            uncertain.data,
            Some(json!({"reason": "io", "session_dropped": true}))
        );
        assert_eq!(services.vault().status().state, VaultState::Locked);

        // Another writer commits behind the daemon's session.
        services.vault().unlock(&passphrase).unwrap();
        Vault::open(services.vault().dir())
            .unwrap()
            .unlock(&passphrase)
            .unwrap()
            .put(
                &SecretName::new("other").unwrap(),
                SecretKind::Generic,
                SecretBytes::from(b"value".to_vec()),
                SecretPolicy::new([ConsumerScope::provider("openai").unwrap()]).unwrap(),
            )
            .unwrap();
        let conflict = router
            .call(&context, SETTINGS_SET, json!({"patch": {}}))
            .unwrap_err();
        assert_eq!(
            conflict.data,
            Some(json!({"reason": "conflict", "session_dropped": true}))
        );
        let locked = router
            .call(&context, SETTINGS_GET, Value::Null)
            .unwrap_err();
        assert_eq!(locked.data, Some(json!({"reason": "locked"})));
    }
}
