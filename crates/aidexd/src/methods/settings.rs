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

fn error(error: PreferencesError) -> RpcError {
    match error {
        PreferencesError::Vault(error) => super::vault::vault_error(&error),
        PreferencesError::Invalid(message) => RpcError::invalid_params(message),
        PreferencesError::Unavailable => {
            RpcError::new(ErrorCode::InternalError, "Preference state unavailable")
        }
    }
}
pub(crate) fn register(router: &mut Router, services: &Arc<Services>) -> Result<(), RouterError> {
    let settings = Arc::new(PreferencesService::new(services.vault().clone()));
    let shared = settings.clone();
    router.register(SETTINGS_GET, move |_, params| {
        expect_no_params(&params)?;
        to_result(&shared.get().map_err(error)?)
    })?;
    router.register(SETTINGS_SET, move |_, params| {
        let params: SetParams = parse_params(params)?;
        to_result(&settings.update(params.patch).map_err(error)?)
    })?;
    let models = Arc::new(ModelPreferencesService::new(services.vault().clone()));
    let shared = models.clone();
    router.register(MODEL_PREFERENCES_GET, move |_, params| {
        expect_no_params(&params)?;
        to_result(&shared.get().map_err(error)?)
    })?;
    router.register(MODEL_PREFERENCES_SET, move |_, params| {
        let patch: ModelPreferencesPatch = parse_params(params)?;
        to_result(&models.set(patch).map_err(error)?)
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
}
