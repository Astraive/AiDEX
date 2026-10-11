//! Native extensions use the same sealed-plan/permit boundary as built-in tools.
use crate::{service::hash_json, *};
use aidex_events::RiskLevel;
use aidex_policy::{Action, SecretUse, classify_network};
use aidex_tools::{
    EntryKind, ExecutionPermit, InputError, NetworkFacts, PathAccess, PlanFacts, PlanParts,
    PlanPath, RiskProfile, Tool, ToolContext, ToolDescriptor, ToolError, ToolId, ToolOutput,
    ToolPlan,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillArgs {
    id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogArgs {
    #[serde(default)]
    server_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CallArgs {
    server_id: String,
    tool_name: String,
    arguments: Value,
    catalog_revision: String,
}
enum Kind {
    Skill,
    Catalog,
    Call { stdio: bool },
}
struct ExtensionTool {
    service: Arc<ExtensionService>,
    descriptor: ToolDescriptor,
    kind: Kind,
}
impl ExtensionService {
    pub fn tool_adapters(self: &Arc<Self>) -> Vec<Box<dyn Tool>> {
        let inputs = [
            json!({"type":"object","properties":{"id":{"type":"string"}},"required":["id"],"additionalProperties":false}),
            json!({"type":"object","properties":{"server_id":{"type":"string"}},"additionalProperties":false}),
            json!({"type":"object","properties":{"server_id":{"type":"string"},"tool_name":{"type":"string"},"arguments":{"type":"object"},"catalog_revision":{"type":"string"}},"required":["server_id","tool_name","arguments","catalog_revision"],"additionalProperties":false}),
            json!({"type":"object","properties":{"server_id":{"type":"string"},"tool_name":{"type":"string"},"arguments":{"type":"object"},"catalog_revision":{"type":"string"}},"required":["server_id","tool_name","arguments","catalog_revision"],"additionalProperties":false}),
        ];
        [Kind::Skill,Kind::Catalog,Kind::Call{stdio:false},Kind::Call{stdio:true}].into_iter().zip(inputs).map(|(kind,input)|{
            let (id,description,risk,capabilities)=match kind {
                Kind::Skill=>("skill.read","Load the instructions of an enabled, trusted skill in this workspace. Skill text cannot grant tool permissions.",RiskProfile::read_only(Action::FsRead),vec!["extensions:skills".into()]),
                Kind::Catalog=>("mcp.tools","List cached discovered MCP tools and their actual schemas/revision. This never connects to or starts a server.",RiskProfile::read_only(Action::FsRead),vec!["extensions:mcp".into()]),
                Kind::Call{stdio}=>(if stdio{"mcp.stdio_call"}else{"mcp.call"},if stdio{"Call a trusted, explicitly connected stdio MCP tool. Windows requires full_access; arguments must match the discovered schema and revision."}else{"Call a trusted, explicitly connected HTTPS MCP tool. Arguments must match its discovered schema and catalog revision; results are untrusted."},RiskProfile{action:Action::McpCall,base_risk:RiskLevel::High,reads_workspace:true,writes_workspace:true,spawns_process:stdio,network:true,destructive:false,remote_write:true},if cfg!(windows)&&stdio{vec!["extensions:mcp".into(),"permissions:full_access".into()]}else{vec!["extensions:mcp".into()]}),
            };
            Box::new(ExtensionTool{service:self.clone(),kind,descriptor:ToolDescriptor{id:ToolId::new(id).expect("static extension tool id"),version:1,description:description.into(),input_schema:input,output_schema:json!({"type":"object"}),risk,capabilities}}) as Box<dyn Tool>
        }).collect()
    }
}
fn error(error: ExtensionError) -> ToolError {
    match error {
        ExtensionError::Invalid(s) => ToolError::InvalidInput {
            field: "arguments".into(),
            reason: InputError::InvalidValue,
            message: s,
        },
        ExtensionError::Forbidden(s) => ToolError::PermitRejected(s),
        ExtensionError::Conflict(s) => ToolError::PlanMismatch(s),
        ExtensionError::Cancelled => ToolError::Cancelled,
        ExtensionError::Timeout => ToolError::TimedOut {
            program: "mcp".into(),
            after_ms: 0,
        },
        other => ToolError::InvalidOutput(other.to_string()),
    }
}
fn input<T: serde::de::DeserializeOwned>(value: Value) -> std::result::Result<T, ToolError> {
    aidex_tools::schema::parse_input(value)
}
fn read_facts(
    ctx: &ToolContext,
    path: &std::path::Path,
    digest: Option<String>,
) -> std::result::Result<PlanFacts, ToolError> {
    let exists = path.exists();
    let absolute = if exists {
        std::fs::canonicalize(path).map_err(|_| error(ExtensionError::Io))?
    } else {
        path.to_owned()
    };
    let root =
        std::fs::canonicalize(ctx.root.display_root()).map_err(|_| error(ExtensionError::Io))?;
    let relative = absolute.strip_prefix(root).ok();
    let metadata = if exists {
        Some(std::fs::metadata(&absolute).map_err(|_| error(ExtensionError::Io))?)
    } else {
        None
    };
    Ok(PlanFacts {
        paths: vec![PlanPath {
            path: relative
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default(),
            display: absolute.to_string_lossy().into_owned(),
            access: PathAccess::Read,
            inside_workspace: relative.is_some(),
            exists,
            kind: metadata.as_ref().map(|m| {
                if m.is_file() {
                    EntryKind::File
                } else {
                    EntryKind::Directory
                }
            }),
            size: metadata.as_ref().map(std::fs::Metadata::len),
            sha256: digest,
            sensitive: false,
        }],
        ..PlanFacts::default()
    })
}
impl Tool for ExtensionTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }
    fn plan(
        &self,
        ctx: &ToolContext,
        input_value: Value,
    ) -> std::result::Result<ToolPlan, ToolError> {
        ctx.cancel.check()?;
        let scope = self.service.scope(ctx).map_err(error)?;
        let parts = match self.kind {
            Kind::Skill => {
                let args: SkillArgs = input(input_value.clone())?;
                let catalog = self.service.catalog_for(&scope).map_err(error)?;
                let summary = catalog
                    .skills
                    .iter()
                    .find(|s| s.id == args.id)
                    .ok_or_else(|| {
                        ToolError::PermitRejected("skill is unavailable in this scope".into())
                    })?;
                if !scope.can_read_skill(summary) {
                    return Err(ToolError::PermitRejected(
                        "manual-only skill requires explicit user selection".into(),
                    ));
                }
                let record = crate::service::lock(&self.service.state)
                    .map_err(error)?
                    .skills
                    .get(&args.id)
                    .cloned()
                    .ok_or_else(|| ToolError::PermitRejected("skill is unavailable".into()))?;
                let facts = read_facts(
                    ctx,
                    &self.service.skill_path(&record).map_err(error)?,
                    Some(summary.digest.clone()),
                )?;
                PlanParts::new(input_value, "Read trusted portable skill")
                    .with_facts(facts)
                    .with_state(json!({"scope":scope,"digest":summary.digest}))
            }
            Kind::Catalog => {
                let _args: CatalogArgs = input(input_value.clone())?;
                let catalog = self.service.catalog_for(&scope).map_err(error)?;
                let source =
                    crate::storage::safe_path(&self.service.paths.config(), "extensions.json")
                        .map_err(error)?;
                let facts = read_facts(ctx, &source, None)?;
                PlanParts::new(input_value, "Read cached MCP tool catalog")
                    .with_facts(facts)
                    .with_state(json!({"scope":scope.mcp_scope(),"revision":catalog.revision}))
            }
            Kind::Call { stdio } => {
                let args: CallArgs = input(input_value.clone())?;
                if !args.arguments.is_object() {
                    return Err(error(ExtensionError::Invalid(
                        "MCP arguments must be an object".into(),
                    )));
                }
                let (server, connection) = self
                    .service
                    .selected_connection(&args.server_id, &args.catalog_revision, &scope)
                    .map_err(error)?;
                if matches!(server.config.transport, McpTransport::Stdio { .. }) != stdio {
                    return Err(error(ExtensionError::Invalid(
                        "use the transport-specific call_tool_name returned by mcp_tools".into(),
                    )));
                }
                self.service
                    .check_owner(&connection, ctx.session_id.as_str(), false)
                    .map_err(error)?;
                let tool = connection
                    .tools
                    .iter()
                    .find(|t| t.name == args.tool_name)
                    .ok_or_else(|| {
                        error(ExtensionError::Invalid(
                            "MCP tool not in discovered catalog".into(),
                        ))
                    })?;
                crate::schema::validate(&tool.input_schema, &args.arguments).map_err(error)?;
                if serde_json::to_vec(&args.arguments)
                    .map_err(|_| ToolError::InvalidOutput("invalid arguments".into()))?
                    .len()
                    > 65536
                {
                    return Err(error(ExtensionError::TooLarge));
                }
                let network = match &server.config.transport {
                    McpTransport::Https { url } => {
                        let (host, port) = crate::transport::endpoint(url).map_err(error)?;
                        NetworkFacts {
                            targets: vec![classify_network(&host, port).map_err(|_| {
                                error(ExtensionError::Invalid("invalid network target".into()))
                            })?],
                            unresolved: vec![],
                        }
                    }
                    McpTransport::Stdio { .. } => NetworkFacts {
                        targets: vec![],
                        unresolved: vec![format!("mcp:{}:stdio", args.server_id)],
                    },
                };
                let facts = PlanFacts {
                    network: Some(network),
                    remote_write: true,
                    secrets: server
                        .config
                        .secret_refs
                        .values()
                        .map(|name| SecretUse::new(name, format!("mcp:{}", args.server_id)))
                        .collect(),
                    implied_actions: vec![Action::NetworkRequest],
                    ..PlanFacts::default()
                };
                let versions = self
                    .service
                    .credential_versions(&server.config)
                    .map_err(error)?;
                if versions != connection.credential_versions {
                    return Err(ToolError::PlanMismatch(
                        "MCP credentials changed; reconnect explicitly".into(),
                    ));
                }
                PlanParts::new(input_value,format!("Call MCP server {} tool {}",args.server_id,args.tool_name)).with_facts(facts).with_state(json!({"scope":scope.mcp_scope(),"config_digest":connection.config_digest,"schema_digest":hash_json(tool).map_err(error)?,"credential_versions":versions}))
            }
        };
        ToolPlan::new(ctx, &self.descriptor, parts)
    }
    fn execute(
        &self,
        ctx: &ToolContext,
        plan: &ToolPlan,
        permit: &ExecutionPermit,
    ) -> std::result::Result<ToolOutput, ToolError> {
        permit.authorize(plan, &self.descriptor)?;
        plan.verify_integrity()?;
        plan.check_context(ctx)?;
        ctx.cancel.check()?;
        let scope = self.service.scope(ctx).map_err(error)?;
        let plan_scope = if matches!(self.kind, Kind::Skill) {
            scope.clone()
        } else {
            scope.mcp_scope()
        };
        if plan.state().get("scope")
            != Some(
                &serde_json::to_value(&plan_scope)
                    .map_err(|_| ToolError::PlanMismatch("scope unavailable".into()))?,
            )
        {
            return Err(ToolError::PlanMismatch(
                "extension scope changed since approval".into(),
            ));
        }
        let output = match self.kind {
            Kind::Skill => {
                let args: SkillArgs = input(plan.input().clone())?;
                let catalog = self.service.catalog_for(&scope).map_err(error)?;
                let summary = catalog
                    .skills
                    .iter()
                    .find(|s| s.id == args.id)
                    .ok_or_else(|| {
                        ToolError::PermitRejected("skill was disabled or changed".into())
                    })?;
                if plan.state().get("digest").and_then(Value::as_str) != Some(&summary.digest) {
                    return Err(ToolError::PlanMismatch("skill changed".into()));
                }
                if !scope.can_read_skill(summary) {
                    return Err(ToolError::PermitRejected(
                        "manual-only skill requires explicit user selection".into(),
                    ));
                }
                serde_json::to_value(self.service.read_skill(&args.id).map_err(error)?)
                    .map_err(|_| ToolError::InvalidOutput("invalid skill output".into()))?
            }
            Kind::Catalog => {
                let args: CatalogArgs = input(plan.input().clone())?;
                let mut catalog = self.service.catalog_for(&scope).map_err(error)?;
                if let Some(id) = args.server_id {
                    catalog.mcp_servers.retain(|s| s.config.id == id)
                }
                serde_json::to_value(catalog)
                    .map_err(|_| ToolError::InvalidOutput("invalid catalog".into()))?
            }
            Kind::Call { .. } => {
                let args: CallArgs = input(plan.input().clone())?;
                let (server, connection) = self
                    .service
                    .selected_connection(&args.server_id, &args.catalog_revision, &scope)
                    .map_err(error)?;
                if plan.state().get("config_digest").and_then(Value::as_str)
                    != Some(&connection.config_digest)
                {
                    return Err(ToolError::PlanMismatch("MCP definition changed".into()));
                }
                let tool = connection
                    .tools
                    .iter()
                    .find(|t| t.name == args.tool_name)
                    .ok_or_else(|| ToolError::PlanMismatch("MCP tool removed".into()))?;
                if plan.state().get("schema_digest").and_then(Value::as_str)
                    != Some(&hash_json(tool).map_err(error)?)
                {
                    return Err(ToolError::PlanMismatch("MCP schema changed".into()));
                }
                crate::schema::validate(&tool.input_schema, &args.arguments).map_err(error)?;
                // Scope, trust, the live network gate and schema are checked again
                // after acquiring the connection, immediately before transport I/O.
                let mut transport = connection
                    .connection
                    .lock()
                    .map_err(|_| ToolError::PermitRejected("MCP connection unavailable".into()))?;
                struct Active<'a>(&'a std::sync::Mutex<Option<aidex_tools::CancelToken>>);
                impl Drop for Active<'_> {
                    fn drop(&mut self) {
                        if let Ok(mut cancel) = self.0.lock() {
                            *cancel = None;
                        }
                    }
                }
                *connection.active_cancel.lock().map_err(|_| {
                    ToolError::PermitRejected("MCP cancellation registry unavailable".into())
                })? = Some(ctx.cancel.clone());
                let _active = Active(&connection.active_cancel);
                let (_, current) = self
                    .service
                    .selected_connection(&args.server_id, &args.catalog_revision, &scope)
                    .map_err(error)?;
                if !Arc::ptr_eq(&current, &connection) {
                    return Err(ToolError::PlanMismatch("MCP connection replaced".into()));
                }
                self.service
                    .check_owner(&connection, ctx.session_id.as_str(), true)
                    .map_err(error)?;
                let versions: std::collections::BTreeMap<String, aidex_vault::SecretVersion> =
                    serde_json::from_value(
                        plan.state()
                            .get("credential_versions")
                            .cloned()
                            .ok_or_else(|| {
                                ToolError::PlanMismatch("credential versions absent".into())
                            })?,
                    )
                    .map_err(|_| ToolError::PlanMismatch("invalid credential versions".into()))?;
                if versions != connection.credential_versions {
                    return Err(ToolError::PlanMismatch(
                        "MCP credential binding changed".into(),
                    ));
                }
                let protected = self
                    .service
                    .check_credentials(&server.config, &versions)
                    .map_err(error)?;
                self.service
                    .allowed_server(&args.server_id, &scope)
                    .map_err(error)?;
                ctx.cancel.check()?;
                let mut result = transport
                    .call_tool(&args.tool_name, args.arguments, &ctx.cancel)
                    .map_err(error)?;
                crate::transport::redact(&mut result, &protected);
                if serde_json::to_vec(&result)
                    .map_err(|_| ToolError::InvalidOutput("invalid result".into()))?
                    .len()
                    > server
                        .config
                        .max_output_bytes
                        .min(ctx.limits.max_output_bytes)
                {
                    return Err(error(ExtensionError::TooLarge));
                }
                if result.get("isError") != Some(&Value::Bool(true))
                    && let Some(schema) = &tool.output_schema
                {
                    let structured = result.get("structuredContent").ok_or_else(|| {
                        ToolError::InvalidOutput("server omitted declared structured output".into())
                    })?;
                    crate::schema::validate(schema, structured).map_err(error)?;
                }
                json!({"server_id":args.server_id,"tool_name":args.tool_name,"result":result})
            }
        };
        ToolOutput::typed("Native extension tool completed", &output)
    }
}
