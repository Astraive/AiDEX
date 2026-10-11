use aidex_drive::{DriveLayout, DrivePaths};
use aidex_extensions::{CallScope, ExtensionService, NoSecrets, RealConnector};
use std::sync::Arc;

#[test]
fn skill_install_edit_disable_and_restart_are_real_portable_changes() {
    let dir = tempfile::tempdir().unwrap();
    let paths = DrivePaths::from_layout(dir.path().to_owned(), DriveLayout::default());
    let service = ExtensionService::with_connector(
        paths.clone(),
        Arc::new(NoSecrets),
        Arc::new(RealConnector::new(
            aidex_sandbox::Sandbox::locate(),
            paths.tmp(),
        )),
    )
    .unwrap();
    let text = "---\nname: rust-review\ndescription: Review Rust changes\n---\nRead the diff and explain concrete bugs.\n";
    let skill = service
        .import_skill(
            text,
            true,
            vec!["workspace".into()],
            vec!["supervised".into()],
        )
        .unwrap();
    assert_eq!(service.read_skill(&skill.id).unwrap().content, text);
    let scope = CallScope {
        workspace_id: "workspace".into(),
        workspace_root: dir.path().to_owned(),
        profile: "supervised".into(),
        full_access: false,
        network_allowed: true,
        selected_skill_ids: vec![],
    };
    assert_eq!(service.catalog_for(&scope).unwrap().skills.len(), 1);
    let changed = text.replace("concrete bugs", "regressions");
    service
        .save_skill(&skill.id, &changed, Some(&skill.digest))
        .unwrap();
    assert!(
        service
            .save_skill(&skill.id, text, Some(&skill.digest))
            .is_err()
    );
    service.set_skill_enabled(&skill.id, false).unwrap();
    assert!(service.catalog_for(&scope).unwrap().skills.is_empty());
    drop(service);
    let reopened = ExtensionService::with_connector(
        paths.clone(),
        Arc::new(NoSecrets),
        Arc::new(RealConnector::new(
            aidex_sandbox::Sandbox::locate(),
            paths.tmp(),
        )),
    )
    .unwrap();
    assert!(!reopened.list_skills().unwrap()[0].enabled);
    assert_eq!(reopened.read_skill(&skill.id).unwrap().content, changed);
}

#[test]
fn shipped_marketplace_checks_version_digest_trust_and_grants_before_install() {
    use aidex_extensions::MarketplaceInstall;
    let dir = tempfile::tempdir().unwrap();
    let paths = DrivePaths::from_layout(dir.path().to_owned(), DriveLayout::default());
    let service = ExtensionService::with_connector(
        paths.clone(),
        Arc::new(NoSecrets),
        Arc::new(RealConnector::for_drive(
            aidex_sandbox::Sandbox::locate(),
            &paths,
        )),
    )
    .unwrap();
    let catalog = service.marketplace_catalog().unwrap();
    assert_eq!(catalog.schema_version, 1);
    assert_eq!(catalog.entries.len(), 3);
    assert!(service.list_plugins().unwrap().is_empty());
    let entry = &catalog.entries[0];
    let request = MarketplaceInstall {
        id: entry.id.clone(),
        version: entry.version.clone(),
        package_digest: entry.package_digest.clone(),
        trusted: true,
        workspace_ids: vec!["workspace".into()],
        profiles: vec!["supervised".into()],
    };
    let mut invalid = request.clone();
    invalid.package_digest = "changed".into();
    assert!(service.install_marketplace(invalid).is_err());
    assert!(service.list_plugins().unwrap().is_empty());
    assert!(!paths.plugins().join(&entry.id).exists());
    let mut invalid = request.clone();
    invalid.trusted = false;
    assert!(service.install_marketplace(invalid).is_err());
    let mut invalid = request.clone();
    invalid.workspace_ids.clear();
    assert!(service.install_marketplace(invalid).is_err());
    let installed = service.install_marketplace(request).unwrap();
    assert!(installed.manifest.trusted);
    assert!(installed.manifest.enabled);
    assert_eq!(service.list_skills().unwrap().len(), 1);
    assert!(
        paths
            .plugins()
            .join(&entry.id)
            .join("manifest.json")
            .exists()
    );
}
