//! One preference record per vault secret, readable only by
//! `process:aidex-preferences`.
use crate::PreferencesError;
use aidex_vault::{
    ConsumerScope, SecretBytes, SecretKind, SecretName, SecretPolicy, VaultError, VaultService,
};
use std::time::Duration;

const CONSUMER: &str = "process:aidex-preferences";

/// The record's bytes, or `None` when it was never saved.
pub(crate) fn read(
    vault: &VaultService,
    name: &str,
) -> Result<Option<SecretBytes>, PreferencesError> {
    let name = SecretName::new(name)?;
    match vault.secret_meta(&name) {
        Err(VaultError::SecretNotFound(_)) => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    let consumer: ConsumerScope = CONSUMER.parse()?;
    let handle = vault.issue_handle(&name, &consumer, Duration::from_secs(10))?;
    let bytes = vault.resolve(&handle, &consumer);
    vault.revoke_handle(&handle.handle_id);
    Ok(Some(bytes?))
}

/// Replace the record atomically (the vault's own write is crash safe).
pub(crate) fn write(
    vault: &VaultService,
    name: &str,
    bytes: Vec<u8>,
) -> Result<(), PreferencesError> {
    let name = SecretName::new(name)?;
    let consumer: ConsumerScope = CONSUMER.parse()?;
    vault.put_secret(
        &name,
        SecretKind::Generic,
        SecretBytes::new(bytes),
        SecretPolicy::new([consumer])?,
    )?;
    Ok(())
}
