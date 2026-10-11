//! Authenticated desktop administration; model tools use the scoped service instead.
use crate::{
    errors::{ErrorCode, RpcError},
    router::{Router, RouterError},
    rpc::{parse_params, to_result},
    services::Services,
};
use aidex_extensions::{CallScope, ExtensionError, McpServerConfig, PluginManifest};
use aidex_native_sessions::PermissionMode;
use serde::Deserialize;
use serde_json::json;
use std::{collections::BTreeMap, sync::Arc};

fn error(error: ExtensionError) -> RpcError {
    let code = match error {
        ExtensionError::Invalid(_) => ErrorCode::InvalidParams,
        ExtensionError::Forbidden(_) => ErrorCode::PolicyDenied,
        ExtensionError::Conflict(_) => ErrorCode::Conflict,
        ExtensionError::Cancelled => ErrorCode::Conflict,
        _ => ErrorCode::Unavailable,
    };
    RpcError::new(code, error.to_string())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Id {
    id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Enable {
    id: String,
    enabled: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveSkill {
    id: String,
    content: String,
    expected_digest: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportSkill {
    content: String,
    trusted: bool,
    workspace_ids: Vec<String>,
    profiles: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallPlugin {
    manifest: PluginManifest,
    files: BTreeMap<String, String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Upsert {
    config: McpServerConfig,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MarketplaceInstall {
    id: String,
    version: String,
    package_digest: String,
    trusted: bool,
    workspace_ids: Vec<String>,
    profiles: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scope {
    workspace_id: String,
    profile: String,
    full_access: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Connect {
    id: String,
    workspace_id: String,
    profile: String,
    full_access: bool,
}
fn scope(services: &Services, p: Scope) -> Result<CallScope, RpcError> {
    let permission: PermissionMode = serde_json::from_value(json!(p.profile))
        .map_err(|_| RpcError::invalid_params("invalid permission profile"))?;
    if p.full_access != (permission == PermissionMode::FullAccess) {
        return Err(RpcError::invalid_params(
            "full_access must match the selected permission profile",
        ));
    }
    let id = aidex_core::EntityId::new(&p.workspace_id)
        .map_err(|e| RpcError::invalid_params(e.to_string()))?;
    let workspace = services
        .workspaces()
        .handle(&id)
        .map_err(|e| RpcError::new(ErrorCode::NotFound, e.to_string()))?;
    Ok(CallScope {
        workspace_id: p.workspace_id,
        workspace_root: workspace.root().display_root(),
        profile: p.profile,
        full_access: p.full_access,
        network_allowed: !services.models().offline().is_offline(),
        selected_skill_ids: vec![],
    })
}
fn check_admin(services: &Services) -> Result<(), RpcError> {
    if services.vault().status().state != aidex_vault::VaultState::Unlocked {
        return Err(RpcError::new(
            ErrorCode::VaultLocked,
            "Unlock the vault before changing extensions",
        ));
    }
    Ok(())
}
pub(crate) fn register(router: &mut Router, services: &Arc<Services>) -> Result<(), RouterError> {
    let s = services.clone();
    router.register("extensions.marketplace.list", move |_, p| {
        crate::rpc::expect_no_params(&p)?;
        to_result(&s.extensions().marketplace_catalog().map_err(error)?)
    })?;
    let s = services.clone();
    router.register("extensions.marketplace.install", move |_, p| {
        check_admin(&s)?;
        let p: MarketplaceInstall = parse_params(p)?;
        to_result(
            &s.extensions()
                .install_marketplace(aidex_extensions::MarketplaceInstall {
                    id: p.id,
                    version: p.version,
                    package_digest: p.package_digest,
                    trusted: p.trusted,
                    workspace_ids: p.workspace_ids,
                    profiles: p.profiles,
                })
                .map_err(error)?,
        )
    })?;
    let s = services.clone();
    router.register("extensions.skills.list", move |_, p| {
        crate::rpc::expect_no_params(&p)?;
        to_result(&s.extensions().list_skills().map_err(error)?)
    })?;
    let s = services.clone();
    router.register("extensions.skills.read", move |_, p| {
        let p: Id = parse_params(p)?;
        to_result(&s.extensions().read_skill(&p.id).map_err(error)?)
    })?;
    let s = services.clone();
    router.register("extensions.skills.import", move |_, p| {
        check_admin(&s)?;
        let p: ImportSkill = parse_params(p)?;
        to_result(
            &s.extensions()
                .import_skill(&p.content, p.trusted, p.workspace_ids, p.profiles)
                .map_err(error)?,
        )
    })?;
    let s = services.clone();
    router.register("extensions.skills.save", move |_, p| {
        check_admin(&s)?;
        let p: SaveSkill = parse_params(p)?;
        to_result(
            &s.extensions()
                .save_skill(&p.id, &p.content, p.expected_digest.as_deref())
                .map_err(error)?,
        )
    })?;
    let s = services.clone();
    router.register("extensions.skills.enable", move |_, p| {
        check_admin(&s)?;
        let p: Enable = parse_params(p)?;
        to_result(
            &s.extensions()
                .set_skill_enabled(&p.id, p.enabled)
                .map_err(error)?,
        )
    })?;
    let s = services.clone();
    router.register("extensions.skills.remove", move |_, p| {
        check_admin(&s)?;
        let p: Id = parse_params(p)?;
        Ok(json!({"removed":s.extensions().remove_skill(&p.id).map_err(error)?}))
    })?;
    let s = services.clone();
    router.register("extensions.plugins.list", move |_, p| {
        crate::rpc::expect_no_params(&p)?;
        to_result(&s.extensions().list_plugins().map_err(error)?)
    })?;
    let s = services.clone();
    router.register("extensions.plugins.install", move |_, p| {
        check_admin(&s)?;
        let p: InstallPlugin = parse_params(p)?;
        to_result(
            &s.extensions()
                .install_plugin(p.manifest, p.files)
                .map_err(error)?,
        )
    })?;
    let s = services.clone();
    router.register("extensions.plugins.enable", move |_, p| {
        check_admin(&s)?;
        let p: Enable = parse_params(p)?;
        to_result(
            &s.extensions()
                .set_plugin_enabled(&p.id, p.enabled)
                .map_err(error)?,
        )
    })?;
    let s = services.clone();
    router.register("extensions.plugins.remove", move |_, p| {
        check_admin(&s)?;
        let p: Id = parse_params(p)?;
        Ok(json!({"removed":s.extensions().remove_plugin(&p.id).map_err(error)?}))
    })?;
    let s = services.clone();
    router.register("extensions.mcp.list", move |_, p| {
        crate::rpc::expect_no_params(&p)?;
        to_result(&s.extensions().list_mcp().map_err(error)?)
    })?;
    let s = services.clone();
    router.register("extensions.mcp.upsert", move |_, p| {
        check_admin(&s)?;
        let p: Upsert = parse_params(p)?;
        to_result(&s.extensions().upsert_mcp(p.config).map_err(error)?)
    })?;
    let s = services.clone();
    router.register("extensions.mcp.remove", move |_, p| {
        check_admin(&s)?;
        let p: Id = parse_params(p)?;
        Ok(json!({"removed":s.extensions().remove_mcp(&p.id).map_err(error)?}))
    })?;
    for method in ["extensions.mcp.connect", "extensions.mcp.discover"] {
        let discover = method.ends_with("discover");
        let s = services.clone();
        router.register(method, move |_, p| {
            check_admin(&s)?;
            let p: Connect = parse_params(p)?;
            let call_scope = scope(
                &s,
                Scope {
                    workspace_id: p.workspace_id,
                    profile: p.profile,
                    full_access: p.full_access,
                },
            )?;
            let service = s.extensions();
            let result = if discover {
                service.discover_mcp_async(&p.id, &call_scope)
            } else {
                service.connect_mcp_async(&p.id, &call_scope)
            };
            to_result(&result.map_err(error)?)
        })?;
    }
    let s = services.clone();
    router.register("extensions.mcp.disconnect", move |_, p| {
        check_admin(&s)?;
        let p: Id = parse_params(p)?;
        s.extensions().disconnect_mcp(&p.id).map_err(error)?;
        to_result(&s.extensions().server_summary(&p.id).map_err(error)?)
    })?;
    let s = services.clone();
    router.register("extensions.mcp.cancel", move |_, p| {
        check_admin(&s)?;
        let p: Id = parse_params(p)?;
        s.extensions().disconnect_mcp(&p.id).map_err(error)?;
        to_result(&s.extensions().server_summary(&p.id).map_err(error)?)
    })?;
    let s = services.clone();
    router.register("extensions.catalog", move |_, p| {
        let p: Scope = parse_params(p)?;
        to_result(&s.extensions().catalog_for(&scope(&s, p)?).map_err(error)?)
    })?;
    Ok(())
}
