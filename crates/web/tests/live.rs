use aidex_drive::{DriveLayout, DrivePaths};
use aidex_tools::CancelToken;
use aidex_vault::{KdfParams, SecretString, VaultService, VaultServiceConfig};
use aidex_web::{FetchInput, WebService};
use std::sync::Arc;

#[test]
#[ignore = "requires live public DNS, HTTPS and outbound connectivity"]
fn fetches_a_public_page_through_the_production_transport() {
    let root = tempfile::tempdir().unwrap();
    let paths = DrivePaths::from_layout(root.path().to_owned(), DriveLayout::default());
    let vault = Arc::new(
        VaultService::open(
            root.path().join("vault"),
            VaultServiceConfig {
                kdf: KdfParams::new(8, 1, 1).unwrap(),
                idle_timeout: None,
            },
        )
        .unwrap(),
    );
    vault
        .init(
            &SecretString::new("synthetic live probe vault passphrase".into()),
            false,
        )
        .unwrap();
    let service = WebService::open(&paths, vault, Arc::new(|| true)).unwrap();
    let output = service
        .fetch(
            &FetchInput {
                url: "https://example.com/".into(),
            },
            &CancelToken::new(),
        )
        .unwrap();
    assert!(output.content.contains("Example Domain"));
    assert!(!output.truncated);
    assert!(!output.content.contains("<script"));
}
