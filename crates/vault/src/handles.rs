//! Scoped secret handles (spec §15.3, §92).
//!
//! A handle names one secret, one consumer scope, and an expiry. It carries no secret
//! bytes and is safe to serialize into sessions, events, or logs. Handles live only in
//! the memory of the unlock session that issued them: locking the vault, restarting
//! the daemon, or revoking them makes them unresolvable.

use crate::{ConsumerScope, VaultError, error::HandleDenial};
use aidex_core::{EntityId, SecretRef, Timestamp};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

/// Longest lifetime a handle may be issued with.
pub const MAX_HANDLE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// Most handles tracked per unlock session (expired and revoked ones are pruned at the limit).
pub const MAX_HANDLES: usize = 4096;

/// A value-free, serializable reference that lets one consumer obtain one secret
/// from the broker until it expires or is revoked.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretHandle {
    pub handle_id: EntityId,
    pub secret_ref: SecretRef,
    pub consumer: ConsumerScope,
    pub expires_at: Timestamp,
}

struct Issued {
    secret_ref: SecretRef,
    consumer: ConsumerScope,
    expires_at: Timestamp,
    deadline: Instant,
    revoked: bool,
}

/// Handles issued by one unlock session.
#[derive(Default)]
pub(crate) struct HandleRegistry {
    issued: HashMap<EntityId, Issued>,
}

impl HandleRegistry {
    pub(crate) fn issue(
        &mut self,
        secret_ref: SecretRef,
        consumer: ConsumerScope,
        ttl: Duration,
        now: Instant,
    ) -> Result<SecretHandle, VaultError> {
        if ttl.is_zero() {
            return Err(VaultError::InvalidTtl("must be greater than zero"));
        }
        if ttl > MAX_HANDLE_TTL {
            return Err(VaultError::InvalidTtl("must be at most 24 hours"));
        }
        if self.issued.len() >= MAX_HANDLES {
            self.issued
                .retain(|_, issued| !issued.revoked && issued.deadline > now);
            if self.issued.len() >= MAX_HANDLES {
                return Err(VaultError::LimitExceeded("too many active secret handles"));
            }
        }
        let deadline = now
            .checked_add(ttl)
            .ok_or(VaultError::InvalidTtl("is out of range"))?;
        let ttl_millis = i64::try_from(ttl.as_millis())
            .map_err(|_| VaultError::InvalidTtl("is out of range"))?;
        let expires_at = Timestamp::now()
            .unix_millis()
            .checked_add(ttl_millis)
            .and_then(|millis| Timestamp::from_unix_millis(millis).ok())
            .ok_or(VaultError::InvalidTtl("is out of range"))?;
        let handle_id =
            EntityId::generate("hdl").map_err(|error| VaultError::Randomness(error.to_string()))?;
        self.issued.insert(
            handle_id.clone(),
            Issued {
                secret_ref: secret_ref.clone(),
                consumer: consumer.clone(),
                expires_at: expires_at.clone(),
                deadline,
                revoked: false,
            },
        );
        Ok(SecretHandle {
            handle_id,
            secret_ref,
            consumer,
            expires_at,
        })
    }

    /// The secret a valid handle refers to. Expiry uses the monotonic clock captured at
    /// issue time, never the caller-supplied `expires_at`.
    pub(crate) fn check(
        &self,
        handle: &SecretHandle,
        presenting: &ConsumerScope,
        now: Instant,
    ) -> Result<&SecretRef, HandleDenial> {
        let issued = self
            .issued
            .get(&handle.handle_id)
            .ok_or(HandleDenial::Unknown)?;
        if issued.secret_ref != handle.secret_ref
            || issued.consumer != handle.consumer
            || issued.expires_at != handle.expires_at
        {
            return Err(HandleDenial::Mismatch);
        }
        if issued.revoked {
            return Err(HandleDenial::Revoked);
        }
        if now >= issued.deadline {
            return Err(HandleDenial::Expired);
        }
        if &issued.consumer != presenting {
            return Err(HandleDenial::ConsumerMismatch);
        }
        Ok(&issued.secret_ref)
    }

    /// Revoke one handle; `false` when it was unknown or already revoked.
    pub(crate) fn revoke(&mut self, handle_id: &EntityId) -> bool {
        self.issued
            .get_mut(handle_id)
            .is_some_and(|issued| !std::mem::replace(&mut issued.revoked, true))
    }

    /// Revoke every handle; returns how many were still live (unrevoked and unexpired).
    pub(crate) fn revoke_all(&mut self, now: Instant) -> usize {
        let mut live = 0;
        for issued in self.issued.values_mut() {
            if !issued.revoked && issued.deadline > now {
                live += 1;
            }
            issued.revoked = true;
        }
        live
    }

    /// Handles that are neither revoked nor expired.
    pub(crate) fn active(&self, now: Instant) -> usize {
        self.issued
            .values()
            .filter(|issued| !issued.revoked && issued.deadline > now)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret_ref(id: &str) -> SecretRef {
        SecretRef {
            vault_id: EntityId::new("vault_t").unwrap(),
            secret_id: EntityId::new(id).unwrap(),
        }
    }

    #[test]
    fn handles_check_identity_revocation_expiry_and_consumer() {
        let mut registry = HandleRegistry::default();
        let openai = ConsumerScope::provider("openai").unwrap();
        let other = ConsumerScope::provider("other").unwrap();
        let start = Instant::now();
        let handle = registry
            .issue(
                secret_ref("sec_a"),
                openai.clone(),
                Duration::from_secs(60),
                start,
            )
            .unwrap();
        assert_eq!(
            registry.check(&handle, &openai, start),
            Ok(&secret_ref("sec_a"))
        );
        assert_eq!(
            registry.check(&handle, &other, start),
            Err(HandleDenial::ConsumerMismatch)
        );
        assert_eq!(
            registry.check(&handle, &openai, start + Duration::from_secs(60)),
            Err(HandleDenial::Expired)
        );
        let mut forged = handle.clone();
        forged.consumer = other.clone();
        assert_eq!(
            registry.check(&forged, &other, start),
            Err(HandleDenial::Mismatch)
        );
        let mut extended = handle.clone();
        extended.expires_at = Timestamp::from_unix_millis(i64::from(i32::MAX) * 1000).unwrap();
        assert_eq!(
            registry.check(&extended, &openai, start),
            Err(HandleDenial::Mismatch)
        );
        let mut unknown = handle.clone();
        unknown.handle_id = EntityId::new("hdl_unknown").unwrap();
        assert_eq!(
            registry.check(&unknown, &openai, start),
            Err(HandleDenial::Unknown)
        );

        assert_eq!(registry.active(start), 1);
        assert!(registry.revoke(&handle.handle_id));
        assert!(!registry.revoke(&handle.handle_id));
        assert_eq!(
            registry.check(&handle, &openai, start),
            Err(HandleDenial::Revoked)
        );
        assert_eq!(registry.active(start), 0);
    }

    #[test]
    fn revoke_all_counts_only_live_revocations() {
        let mut registry = HandleRegistry::default();
        let scope = ConsumerScope::mcp("github").unwrap();
        let now = Instant::now();
        for _ in 0..3 {
            registry
                .issue(
                    secret_ref("sec_a"),
                    scope.clone(),
                    Duration::from_secs(5),
                    now,
                )
                .unwrap();
        }
        assert_eq!(registry.revoke_all(now), 3);
        assert_eq!(registry.revoke_all(now), 0);
    }

    #[test]
    fn revocation_reclaims_capacity_without_waiting_for_expiry() {
        let mut registry = HandleRegistry::default();
        let scope = ConsumerScope::provider("openai").unwrap();
        let now = Instant::now();
        let mut handles = Vec::with_capacity(MAX_HANDLES);
        for _ in 0..MAX_HANDLES {
            handles.push(
                registry
                    .issue(secret_ref("sec_a"), scope.clone(), MAX_HANDLE_TTL, now)
                    .unwrap(),
            );
        }
        assert!(matches!(
            registry.issue(secret_ref("sec_a"), scope.clone(), MAX_HANDLE_TTL, now),
            Err(VaultError::LimitExceeded(_))
        ));
        assert!(registry.revoke(&handles[0].handle_id));
        let replacement = registry
            .issue(secret_ref("sec_a"), scope.clone(), MAX_HANDLE_TTL, now)
            .unwrap();
        assert_eq!(registry.active(now), MAX_HANDLES);
        assert!(registry.check(&handles[0], &scope, now).is_err());
        assert!(registry.check(&replacement, &scope, now).is_ok());

        assert_eq!(registry.revoke_all(now), MAX_HANDLES);
        for _ in 0..MAX_HANDLES {
            registry
                .issue(secret_ref("sec_a"), scope.clone(), MAX_HANDLE_TTL, now)
                .unwrap();
        }
        assert_eq!(registry.active(now), MAX_HANDLES);
        assert!(registry.check(&replacement, &scope, now).is_err());
    }

    #[test]
    fn lifetimes_are_bounded() {
        let mut registry = HandleRegistry::default();
        let scope = ConsumerScope::process("git.push").unwrap();
        let now = Instant::now();
        for ttl in [Duration::ZERO, MAX_HANDLE_TTL + Duration::from_secs(1)] {
            let error = registry
                .issue(secret_ref("sec_a"), scope.clone(), ttl, now)
                .unwrap_err();
            assert!(matches!(error, VaultError::InvalidTtl(_)));
        }
        registry
            .issue(secret_ref("sec_a"), scope, MAX_HANDLE_TTL, now)
            .unwrap();
    }
}
