//! A versioned shipped catalog of first-party packages. No remote downloads.
use crate::{service::hash_json, *};
use serde::Deserialize;
use std::collections::BTreeMap;

const PACKAGES: &[&str] = &[
    include_str!("../packages/coding.json"),
    include_str!("../packages/review.json"),
    include_str!("../packages/build.json"),
];
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Package {
    manifest: PluginManifest,
    files: BTreeMap<String, String>,
}
fn packages() -> Result<Vec<Package>> {
    PACKAGES
        .iter()
        .map(|source| {
            serde_json::from_str(source)
                .map_err(|_| ExtensionError::Invalid("shipped package catalog is malformed".into()))
        })
        .collect()
}
impl ExtensionService {
    pub fn marketplace_catalog(&self) -> Result<MarketplaceCatalog> {
        let entries = packages()?
            .into_iter()
            .map(|package| {
                let skill_names = package
                    .files
                    .values()
                    .map(|content| crate::skills::parse(content).map(|(name, _, _, _)| name))
                    .collect::<Result<Vec<_>>>()?;
                Ok(MarketplaceEntry {
                    id: package.manifest.id.clone(),
                    name: package.manifest.name.clone(),
                    version: package.manifest.version.clone(),
                    description: package.manifest.description.clone(),
                    package_digest: hash_json(&(&package.manifest, &package.files))?,
                    skill_names,
                    permissions: vec![],
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(MarketplaceCatalog {
            schema_version: 1,
            catalog_version: "2026.10.11.1".into(),
            entries,
        })
    }
    pub fn install_marketplace(&self, request: MarketplaceInstall) -> Result<PluginSummary> {
        let MarketplaceInstall {
            id,
            version,
            package_digest,
            trusted,
            workspace_ids,
            profiles,
        } = request;
        if !trusted || workspace_ids.is_empty() || profiles.is_empty() {
            return Err(ExtensionError::Forbidden(
                "marketplace installation requires explicit trust and scope grants".into(),
            ));
        }
        let mut package = packages()?
            .into_iter()
            .find(|package| package.manifest.id == id && package.manifest.version == version)
            .ok_or_else(|| {
                ExtensionError::Unavailable("requested shipped package version is absent".into())
            })?;
        if hash_json(&(&package.manifest, &package.files))? != package_digest {
            return Err(ExtensionError::Conflict(
                "shipped package digest changed; refresh the catalog".into(),
            ));
        }
        package.manifest.trusted = true;
        package.manifest.enabled = true;
        package.manifest.workspace_ids = workspace_ids;
        package.manifest.profiles = profiles;
        self.install_plugin(package.manifest, package.files)
    }
}
