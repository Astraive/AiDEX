//! Drive inspection and initialization for the setup wizard, without Tauri types so
//! they are unit-tested directly. TypeScript mirrors: `packages/client/src/dto/installer.ts`.
//!
//! Inspection only reads. Initialization calls `aidex_drive::init`, which creates the
//! manifest-declared directories and the manifest at the drive root and never formats,
//! repartitions, or deletes anything.

use crate::error::SetupError;
use aidex_core::ServiceErrorCode;
use aidex_drive::{
    init_with, resolve_root, status_with, volume_for, DrivePaths, DriveStatus, InitOptions,
    RemovableStatus, StatusOptions, VolumeInfo, CURRENT_SCHEMA_VERSION,
};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// `inspect_drive`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DriveInspection {
    pub root: String,
    pub volume: VolumeInfo,
    pub eligible: bool,
    pub ineligible_reason: Option<String>,
    pub health: Option<DriveHealth>,
    pub health_error: Option<SetupError>,
}

/// `init_drive`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct InitReport {
    pub root: String,
    pub previous_schema_version: Option<u32>,
    pub health: DriveHealth,
    pub directories: Vec<String>,
}

/// `aidex_drive::DriveStatus` in UI terms (same shape as the desktop's).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DriveHealth {
    pub writable: bool,
    pub free_bytes: Option<u64>,
    pub schema_version: Option<u32>,
    pub current_schema_version: u32,
    pub migration_required: bool,
    pub migration_advice: Option<String>,
}

impl From<&DriveStatus> for DriveHealth {
    fn from(status: &DriveStatus) -> Self {
        Self {
            writable: status.writable,
            free_bytes: status.free_bytes,
            schema_version: status.schema_version,
            current_schema_version: CURRENT_SCHEMA_VERSION,
            migration_required: status.migration_required(),
            migration_advice: status.migration_advice(),
        }
    }
}

/// Inspect `root` without changing anything.
pub fn inspect(root: &Path) -> Result<DriveInspection, SetupError> {
    let root = resolve_root(Some(root))?;
    let volume = volume_for(&root)?;
    let ineligible_reason = ineligible_reason(&root, &volume);
    let (health, health_error) = if volume.has_aidex_manifest {
        match status_with(
            &root,
            StatusOptions {
                require_removable: false,
            },
        ) {
            Ok(status) => (Some(DriveHealth::from(&status)), None),
            Err(error) => (None, Some(SetupError::from(error))),
        }
    } else {
        (None, None)
    };
    Ok(DriveInspection {
        root: display_path(&root),
        eligible: ineligible_reason.is_none(),
        ineligible_reason,
        volume,
        health,
        health_error,
    })
}
/// Inspect the standard per-user AiDEX target under the platform's local-data directory.
/// The target may not exist yet; inspection only reads its containing volume and manifest.
pub fn inspect_normal(base: &Path) -> Result<DriveInspection, SetupError> {
    let base_reason = match std::fs::metadata(base) {
        Ok(metadata) if metadata.is_dir() => None,
        Ok(_) => Some("The per-user local-data location exists but is not a directory.".to_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(SetupError::internal(format!(
                "cannot inspect per-user data directory {}: {error}",
                base.display()
            )));
        }
    };
    let root = normal_root(base)?;
    let volume_path = existing_directory_ancestor(base)?;
    let mut volume = volume_for(volume_path)?;
    let metadata = match std::fs::symlink_metadata(&root) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(SetupError::internal(format!(
                "cannot inspect normal install target {}: {error}",
                root.display()
            )));
        }
    };
    let target_reason = base_reason.or_else(|| {
        metadata.as_ref().and_then(|metadata| {
            if metadata.file_type().is_symlink() {
                Some("The normal install target is a symbolic link.".to_owned())
            } else if !metadata.is_dir() {
                Some("The normal install target exists but is not a directory.".to_owned())
            } else {
                None
            }
        })
    });
    let ineligible_reason = normal_ineligible_reason(volume.removable, target_reason);
    let target_is_directory = metadata
        .as_ref()
        .is_some_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink());
    let has_manifest = target_is_directory && root.join("aidex-drive.json").is_file();
    let (health, health_error) = if has_manifest {
        match status_with(
            &root,
            StatusOptions {
                require_removable: false,
            },
        ) {
            Ok(status) => (Some(DriveHealth::from(&status)), None),
            Err(error) => (None, Some(SetupError::from(error))),
        }
    } else {
        (None, None)
    };
    volume.has_aidex_manifest = has_manifest;
    volume.schema_version = health.as_ref().and_then(|health| health.schema_version);
    Ok(DriveInspection {
        root: display_path(&root),
        eligible: ineligible_reason.is_none(),
        ineligible_reason,
        volume,
        health,
        health_error,
    })
}

/// Why AiDEX will not initialize `root` on `volume`, or `None` when it will.
pub fn ineligible_reason(root: &Path, volume: &VolumeInfo) -> Option<String> {
    if volume.system {
        return Some("This is the system drive that holds the operating system.".into());
    }
    match volume.removable {
        RemovableStatus::Fixed => {
            return Some(
                "This is an internal disk. AiDEX installs only on removable or USB drives.".into(),
            );
        }
        RemovableStatus::Unknown => {
            return Some(
                "The drive type could not be determined, so AiDEX will not use it.".into(),
            );
        }
        RemovableStatus::Removable | RemovableStatus::UsbFixed => {}
    }
    if !same_path(root, &volume.root) {
        return Some(format!(
            "AiDEX installs at the root of a drive; choose {} instead.",
            display_path(&volume.root)
        ));
    }
    None
}

/// Initialize `root` (removable media required) and report the result.
pub fn initialize(root: &Path) -> Result<InitReport, SetupError> {
    initialize_with(root, InitOptions::default())
}

/// [`initialize`] with explicit options (tests pass `require_removable: false`).
pub fn initialize_with(root: &Path, options: InitOptions) -> Result<InitReport, SetupError> {
    let root = resolve_root(Some(root))?;
    if options.require_removable {
        let volume = volume_for(&root)?;
        if let Some(reason) = ineligible_reason(&root, &volume) {
            return Err(SetupError::new(
                ServiceErrorCode::InvalidArgument,
                "not_eligible",
                reason,
            ));
        }
    }
    // Missing, malformed, or unsupported manifests have no previous schema; init reports
    // the malformed and unsupported ones without rewriting them.
    let previous_schema_version = status_with(
        &root,
        StatusOptions {
            require_removable: false,
        },
    )
    .ok()
    .and_then(|status| status.schema_version);
    let status = init_with(&root, options)?;
    let directories = DrivePaths::open(&root)?
        .layout()
        .entries()
        .map(|(_, relative)| relative.as_str().to_owned())
        .collect();
    Ok(InitReport {
        root: display_path(&status.root),
        previous_schema_version,
        health: DriveHealth::from(&status),
        directories,
    })
}
/// Initialize the per-user target after checking its location and refusing links/files.
pub fn initialize_normal(base: &Path) -> Result<InitReport, SetupError> {
    let inspection = inspect_normal(base)?;
    if let Some(reason) = inspection.ineligible_reason {
        return Err(SetupError::new(
            ServiceErrorCode::InvalidArgument,
            "not_eligible",
            reason,
        ));
    }
    std::fs::create_dir_all(base).map_err(|error| {
        SetupError::internal(format!(
            "cannot create per-user AiDEX directory {}: {error}",
            base.display()
        ))
    })?;
    let base = std::fs::canonicalize(base).map_err(|error| {
        SetupError::internal(format!(
            "cannot resolve per-user data directory {}: {error}",
            base.display()
        ))
    })?;
    let root = base.join("AiDEX");
    match std::fs::symlink_metadata(&root) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(SetupError::new(
                ServiceErrorCode::InvalidArgument,
                "not_eligible",
                "The normal install target must be an ordinary directory, not a link or file.",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(&root).map_err(|error| {
                SetupError::internal(format!(
                    "cannot create normal install target {}: {error}",
                    root.display()
                ))
            })?;
        }
        Err(error) => {
            return Err(SetupError::internal(format!(
                "cannot inspect normal install target {}: {error}",
                root.display()
            )));
        }
    }
    let canonical_root = std::fs::canonicalize(&root).map_err(|error| {
        SetupError::internal(format!(
            "cannot resolve normal install target {}: {error}",
            root.display()
        ))
    })?;
    if canonical_root.parent() != Some(base.as_path()) {
        return Err(SetupError::new(
            ServiceErrorCode::InvalidArgument,
            "not_eligible",
            "The normal install target must remain inside the per-user data directory.",
        ));
    }
    initialize_with(
        &canonical_root,
        InitOptions {
            require_removable: false,
        },
    )
}

/// A path for display: Windows verbatim disk prefixes (`\\?\H:\`) are removed.
pub fn display_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => rest.to_owned(),
        _ => text.into_owned(),
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    let normalize = |path: &Path| -> PathBuf {
        let text = display_path(path);
        PathBuf::from(if cfg!(windows) {
            text.to_ascii_lowercase()
        } else {
            text
        })
    };
    normalize(left) == normalize(right)
}
fn normal_root(base: &Path) -> Result<PathBuf, SetupError> {
    let base = match std::fs::canonicalize(base) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => base.to_path_buf(),
        Err(error) => {
            return Err(SetupError::internal(format!(
                "cannot resolve per-user data directory {}: {error}",
                base.display()
            )));
        }
    };
    Ok(base.join("AiDEX"))
}

fn normal_ineligible_reason(
    removable: RemovableStatus,
    target_reason: Option<String>,
) -> Option<String> {
    match removable {
        RemovableStatus::Removable | RemovableStatus::UsbFixed => Some(
            "The normal install location must be on this computer, not removable media.".into(),
        ),
        RemovableStatus::Fixed | RemovableStatus::Unknown => target_reason,
    }
}

fn existing_directory_ancestor(path: &Path) -> Result<PathBuf, SetupError> {
    let mut candidate = path;
    loop {
        match std::fs::metadata(candidate) {
            Ok(metadata) if metadata.is_dir() => {
                return std::fs::canonicalize(candidate).map_err(|error| {
                    SetupError::internal(format!(
                        "cannot resolve install volume for {}: {error}",
                        path.display()
                    ))
                });
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(SetupError::internal(format!(
                    "cannot inspect install volume for {}: {error}",
                    path.display()
                )));
            }
        }
        candidate = candidate.parent().ok_or_else(|| {
            SetupError::internal(format!(
                "cannot find an existing parent for install target {}",
                path.display()
            ))
        })?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn volume(root: &str, removable: RemovableStatus, system: bool) -> VolumeInfo {
        VolumeInfo {
            root: root.into(),
            label: None,
            filesystem: Some("exFAT".into()),
            total_bytes: Some(64 << 30),
            free_bytes: Some(60 << 30),
            removable,
            system,
            has_aidex_manifest: false,
            schema_version: None,
        }
    }

    #[test]
    fn only_portable_non_system_volume_roots_are_eligible() {
        let root = if cfg!(windows) { r"H:\" } else { "/media/usb" };
        let usb = volume(root, RemovableStatus::UsbFixed, false);
        assert_eq!(ineligible_reason(Path::new(root), &usb), None);
        let stick = volume(root, RemovableStatus::Removable, false);
        assert_eq!(ineligible_reason(Path::new(root), &stick), None);

        let system = volume(root, RemovableStatus::UsbFixed, true);
        assert!(ineligible_reason(Path::new(root), &system)
            .unwrap()
            .contains("system drive"));
        let internal = volume(root, RemovableStatus::Fixed, false);
        assert!(ineligible_reason(Path::new(root), &internal)
            .unwrap()
            .contains("internal disk"));
        let unknown = volume(root, RemovableStatus::Unknown, false);
        assert!(ineligible_reason(Path::new(root), &unknown).is_some());

        let nested = Path::new(root).join("AiDEX");
        assert!(ineligible_reason(&nested, &stick)
            .unwrap()
            .contains("root of a drive"));
    }

    #[test]
    fn inspecting_a_temporary_folder_is_read_only_and_not_eligible() {
        let folder = tempfile::tempdir().unwrap();
        let inspection = inspect(folder.path()).unwrap();
        assert!(!inspection.eligible);
        assert!(inspection.ineligible_reason.is_some());
        assert_eq!(fs::read_dir(folder.path()).unwrap().count(), 0);
    }

    #[test]
    fn initialization_refuses_ineligible_targets_without_writing() {
        let folder = tempfile::tempdir().unwrap();
        let error = initialize(folder.path()).unwrap_err();
        assert_eq!(error.code, "invalid_argument");
        assert_eq!(fs::read_dir(folder.path()).unwrap().count(), 0);
    }

    #[test]
    fn initialization_reports_the_layout_and_previous_schema() {
        let folder = tempfile::tempdir().unwrap();
        let options = InitOptions {
            require_removable: false,
        };
        let report = initialize_with(folder.path(), options).unwrap();
        assert_eq!(report.previous_schema_version, None);
        assert_eq!(report.health.schema_version, Some(CURRENT_SCHEMA_VERSION));
        assert!(report.health.writable);
        assert!(!report.health.migration_required);
        assert!(report.directories.contains(&"runtime".to_owned()));
        assert!(report.directories.contains(&"private/vault".to_owned()));
        for directory in &report.directories {
            assert!(folder.path().join(directory).is_dir(), "{directory}");
        }

        let again = initialize_with(folder.path(), options).unwrap();
        assert_eq!(again.previous_schema_version, Some(CURRENT_SCHEMA_VERSION));
        assert_eq!(again.directories, report.directories);
    }
    #[test]
    fn normal_target_inspection_is_read_only_and_initialization_preserves_user_files() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("AiDEX");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("notes.txt"), "keep this file").unwrap();

        let inspection = inspect_normal(base.path()).unwrap();
        assert!(inspection.eligible, "{:?}", inspection.ineligible_reason);
        assert_eq!(
            inspection.root,
            display_path(&fs::canonicalize(&root).unwrap())
        );
        assert_eq!(inspection.health, None);
        assert_eq!(
            fs::read_to_string(root.join("notes.txt")).unwrap(),
            "keep this file"
        );

        let report = initialize_normal(base.path()).unwrap();
        assert_eq!(report.previous_schema_version, None);
        assert_eq!(report.health.schema_version, Some(CURRENT_SCHEMA_VERSION));
        assert_eq!(
            fs::read_to_string(root.join("notes.txt")).unwrap(),
            "keep this file"
        );
        assert!(root.join("aidex-drive.json").is_file());
    }

    #[test]
    fn normal_target_rejects_a_file_without_overwriting_it() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("AiDEX");
        fs::write(&root, "unrelated user file").unwrap();

        let inspection = inspect_normal(base.path()).unwrap();
        assert!(!inspection.eligible);
        assert!(inspection
            .ineligible_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("not a directory")));
        let error = initialize_normal(base.path()).unwrap_err();
        assert_eq!(error.code, "invalid_argument");
        assert_eq!(fs::read_to_string(root).unwrap(), "unrelated user file");
    }
    #[test]
    fn normal_target_rejects_a_file_as_local_data_directory() {
        let parent = tempfile::tempdir().unwrap();
        let base = parent.path().join("local-data");
        fs::write(&base, "unrelated user file").unwrap();

        let inspection = inspect_normal(&base).unwrap();
        assert!(!inspection.eligible);
        assert!(inspection
            .ineligible_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("local-data location")));
        let error = initialize_normal(&base).unwrap_err();
        assert_eq!(error.code, "invalid_argument");
        assert_eq!(fs::read_to_string(base).unwrap(), "unrelated user file");
    }

    #[test]
    fn normal_target_rejects_removable_volumes() {
        for removable in [RemovableStatus::Removable, RemovableStatus::UsbFixed] {
            assert!(normal_ineligible_reason(removable, None)
                .unwrap()
                .contains("removable media"));
        }
        assert_eq!(normal_ineligible_reason(RemovableStatus::Fixed, None), None);
    }

    #[test]
    fn display_paths_drop_the_verbatim_disk_prefix_only() {
        assert_eq!(display_path(Path::new(r"\\?\H:\")), r"H:\");
        assert_eq!(
            display_path(Path::new(r"\\?\UNC\server\share")),
            r"\\?\UNC\server\share"
        );
    }
}
