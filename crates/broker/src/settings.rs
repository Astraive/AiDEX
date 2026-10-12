//! The drive's active policy settings: profile, privacy mode, host trust, network
//! mode. Persisted as a small closed-schema TOML file (`private_config/policy.toml`)
//! written atomically; changing it is a `policy.change` that only the local user can
//! make through [`crate::Broker::change_policy`].

use std::{
    fs::{self, File},
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};

use aidex_policy::{EvaluationContext, HostTrust, NetworkMode, PrivacyMode, Profile};
use serde::{Deserialize, Serialize};

use crate::BrokerError;

/// File name of the persisted settings inside `DrivePaths::config()`.
pub const SETTINGS_FILE: &str = "policy.toml";
/// Version of the settings file format.
pub const SETTINGS_VERSION: u32 = 1;
/// Largest settings file accepted.
const MAX_SETTINGS_BYTES: u64 = 16 * 1024;

/// Session-wide policy inputs (see `aidex_policy::EvaluationContext`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicySettings {
    pub profile: Profile,
    pub privacy: PrivacyMode,
    /// Drive-wide until per-host trust records exist (spec §61).
    pub host_trust: HostTrust,
    pub network_mode: NetworkMode,
}

impl Default for PolicySettings {
    /// Balanced profile, hybrid privacy, known host, normal network. Balanced asks
    /// before remote writes, secrets, package installs, and risky shell commands;
    /// Safe is stricter but needs `restricted` sandboxing, which Windows cannot
    /// enforce (process tools then fail closed).
    fn default() -> Self {
        Self {
            profile: Profile::Balanced,
            privacy: PrivacyMode::Hybrid,
            host_trust: HostTrust::Known,
            network_mode: NetworkMode::Normal,
        }
    }
}

impl PolicySettings {
    pub fn context(&self) -> EvaluationContext {
        EvaluationContext::new(
            self.profile,
            self.privacy,
            self.host_trust,
            self.network_mode,
        )
    }
}

/// A requested settings change.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum PolicyChange {
    Profile(Profile),
    Privacy(PrivacyMode),
    HostTrust(HostTrust),
    NetworkMode(NetworkMode),
    /// Re-read the policy documents in `DrivePaths::policies()`.
    ReloadDocuments,
}

impl PolicyChange {
    pub(crate) fn apply(self, settings: &mut PolicySettings) {
        match self {
            Self::Profile(profile) => settings.profile = profile,
            Self::Privacy(privacy) => settings.privacy = privacy,
            Self::HostTrust(trust) => settings.host_trust = trust,
            Self::NetworkMode(mode) => settings.network_mode = mode,
            Self::ReloadDocuments => {}
        }
    }

    pub(crate) fn describe(self) -> String {
        match self {
            Self::Profile(profile) => format!("policy profile set to {profile}"),
            Self::Privacy(privacy) => format!("privacy mode set to {privacy}"),
            Self::HostTrust(trust) => format!("host trust set to {trust}"),
            Self::NetworkMode(mode) => format!("network mode set to {mode}"),
            Self::ReloadDocuments => "policy documents reloaded".to_owned(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsFile {
    version: u32,
    profile: Profile,
    privacy: PrivacyMode,
    host_trust: HostTrust,
    network_mode: NetworkMode,
}

/// Read settings from `path`; a missing file yields `None`.
pub(crate) fn load(path: &Path) -> Result<Option<PolicySettings>, BrokerError> {
    let error = |message: String| BrokerError::Settings(format!("{}: {message}", path.display()));
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(error("must be a regular file, not a link".into()));
        }
        Ok(metadata) if metadata.len() > MAX_SETTINGS_BYTES => {
            return Err(error("is larger than 16 KiB".into()));
        }
        Ok(_) => {}
        Err(io) if io.kind() == ErrorKind::NotFound => return Ok(None),
        Err(io) => return Err(error(io.to_string())),
    }
    let text = fs::read_to_string(path).map_err(|io| error(io.to_string()))?;
    let file: SettingsFile = toml::from_str(&text).map_err(|parse| error(parse.to_string()))?;
    if file.version != SETTINGS_VERSION {
        return Err(error(format!(
            "unsupported version {} (expected {SETTINGS_VERSION})",
            file.version
        )));
    }
    Ok(Some(PolicySettings {
        profile: file.profile,
        privacy: file.privacy,
        host_trust: file.host_trust,
        network_mode: file.network_mode,
    }))
}

/// Write settings atomically: exclusive temporary sibling, fsync, rename.
pub(crate) fn store(path: &Path, settings: &PolicySettings) -> Result<(), BrokerError> {
    let error = |message: String| BrokerError::Settings(format!("{}: {message}", path.display()));
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(error("must be a regular file, not a link".into()));
    }
    // Every value is a closed enum identifier, so no TOML escaping is needed.
    let text = format!(
        "# AiDEX policy settings. Change with `aidex policy profile <name>`.\n\
         version = {SETTINGS_VERSION}\n\
         profile = \"{}\"\n\
         privacy = \"{}\"\n\
         host_trust = \"{}\"\n\
         network_mode = \"{}\"\n",
        settings.profile, settings.privacy, settings.host_trust, settings.network_mode
    );
    let temporary = temporary_sibling(path);
    let written = (|| {
        let mut file = File::options()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        sync_parent(path)
    })();
    if let Err(io) = written {
        let _ = fs::remove_file(&temporary);
        return Err(error(io.to_string()));
    }
    Ok(())
}

fn temporary_sibling(path: &Path) -> PathBuf {
    let name = path.file_name().map_or_else(
        || "policy".into(),
        |name| name.to_string_lossy().into_owned(),
    );
    path.with_file_name(format!(".{name}.{}.tmp", std::process::id()))
}

#[cfg(unix)]
fn sync_parent(path: &Path) -> std::io::Result<()> {
    match path.parent() {
        Some(parent) => File::open(parent)?.sync_all(),
        None => Ok(()),
    }
}

#[cfg(not(unix))]
fn sync_parent(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_reject_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SETTINGS_FILE);
        assert_eq!(load(&path).unwrap(), None);
        let settings = PolicySettings {
            profile: Profile::Safe,
            ..PolicySettings::default()
        };
        store(&path, &settings).unwrap();
        assert_eq!(load(&path).unwrap(), Some(settings));
        fs::write(&path, "version = 1\nprofile = \"safe\"\nextra = 1\n").unwrap();
        assert!(load(&path).is_err());
        fs::write(&path, "version = 1\nprofile = \"reckless\"\n").unwrap();
        assert!(load(&path).is_err());
    }
}
