//! Planning a harness-level call: argument validation, the drive's policy
//! (with the offline switch for network tools), and whether the user must
//! approve it.
use super::ExtTool;
use crate::NativeEngine;
use aidex_core::EntityId;
use aidex_native_sessions::{ArtifactKind, PermissionMode, SessionDetail, digest};
use aidex_policy::{Action, Intent, Verdict, classify_network, evaluate};
use serde::Deserialize;
use serde_json::{Value, json};

/// Longest search query.
const MAX_QUERY: usize = 512;
/// Longest URL accepted from the model.
const MAX_URL: usize = 8 * 1024;
/// Largest artifact or plan the model may write in one call (1 MiB).
pub(crate) const MAX_ARTIFACT_CONTENT: usize = 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SearchArgs {
    pub query: String,
    #[serde(default)]
    pub max_results: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FetchArgs {
    pub url: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SkillArgs {
    pub name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateArgs {
    pub name: String,
    pub kind: ArtifactKind,
    #[serde(default)]
    pub mime: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    pub content: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateArgs {
    pub id: String,
    pub content: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PlanArgs {
    pub title: String,
    pub markdown: String,
}

/// A checked harness-level call, ready to run (after approval when needed).
#[derive(Clone, Debug)]
pub(crate) struct ExtensionPlan {
    pub call_id: EntityId,
    pub tool: ExtTool,
    pub input: Value,
    pub needs_approval: bool,
    /// Binds an approval to this exact call.
    pub fingerprint: String,
    pub preview: Option<Value>,
}

fn parse<T: for<'de> Deserialize<'de>>(input: &Value, tool: &str) -> Result<T, String> {
    serde_json::from_value(input.clone())
        .map_err(|_| format!("Invalid arguments for {tool}; check the tool schema."))
}

/// Host and port of an http(s) URL, for the policy check.
pub(crate) fn url_target(text: &str) -> Result<(String, u16), String> {
    if text.len() > MAX_URL {
        return Err("The URL is too long.".into());
    }
    let url = url::Url::parse(text.trim()).map_err(|_| "The URL is not valid.".to_string())?;
    if !matches!(url.scheme(), "https" | "http") {
        return Err("Only http(s) URLs can be fetched.".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("URLs with embedded credentials are refused.".into());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "The URL has no host.".to_string())?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_owned();
    let port = url
        .port_or_known_default()
        .ok_or_else(|| "The URL has no port.".to_string())?;
    Ok((host, port))
}

impl NativeEngine {
    /// Evaluate `intent` under the drive's policy documents (with the
    /// workspace's project rules) and its policy settings.
    fn decide(
        &self,
        s: &SessionDetail,
        mut intent: Intent,
    ) -> Result<aidex_policy::Decision, String> {
        let workspace = s
            .configuration
            .workspace_id
            .as_ref()
            .map(EntityId::new)
            .transpose()
            .map_err(|e| e.to_string())?;
        let root = workspace
            .as_ref()
            .map(|id| {
                self.broker
                    .workspaces
                    .handle(id)
                    .map(|h| h.root().display_root())
            })
            .transpose()
            .map_err(|e| e.to_string())?;
        let (policies, _) = self
            .broker
            .policy_state(root.as_deref(), workspace.as_ref())
            .map_err(|e| format!("policy documents failed to load: {e}"))?;
        intent.workspace_id = workspace;
        Ok(evaluate(
            &policies,
            &self.extensions.network().context,
            &intent,
        ))
    }

    /// Evaluate `mcp.call` for `binding`: `Ok(true)` when policy asks for an
    /// approval; denials and rate limits are refusals.
    pub(crate) fn mcp_policy(
        &self,
        s: &SessionDetail,
        binding: &super::McpBinding,
    ) -> Result<bool, String> {
        let intent = Intent::new(
            Action::McpCall,
            EntityId::new(&s.id).map_err(|e| e.to_string())?,
            format!("call MCP tool {} on {}", binding.tool, binding.server_id),
        );
        let decision = self.decide(s, intent)?;
        match decision.verdict {
            Verdict::Deny => Err(format!("Policy denied the MCP call: {}.", decision.reason)),
            Verdict::RateLimit { .. } => {
                Err("MCP calls are rate-limited by policy; try later.".into())
            }
            Verdict::RequireApproval { .. } => Ok(true),
            _ => Ok(false),
        }
    }

    /// The offline switch, the emergency stop, and a `network.request`
    /// intent for `host:port` under the drive's settings. Approval verdicts
    /// refuse: the daemon's egress gate cannot wait for one either.
    pub(crate) fn network_gate(
        &self,
        s: &SessionDetail,
        host: &str,
        port: u16,
        summary: &str,
    ) -> Result<(), String> {
        if let Some(reason) = self.extensions.network().blocked {
            return Err(format!("Web access is unavailable: {reason}."));
        }
        let target = classify_network(host, port)
            .map_err(|e| format!("The destination cannot be classified: {e}."))?;
        let intent = Intent::new(
            Action::NetworkRequest,
            EntityId::new(&s.id).map_err(|e| e.to_string())?,
            summary,
        )
        .with_network(target);
        let decision = self.decide(s, intent)?;
        match decision.verdict {
            Verdict::Deny => Err(format!("Policy denied the request: {}.", decision.reason)),
            Verdict::RequireApproval { .. } => Err(format!(
                "Web requests to {host} need approval under the current policy ({}); the user \
                 can add a policy rule allowing network.request for {host} or change the \
                 network mode.",
                decision.rule
            )),
            _ => Ok(()),
        }
    }

    /// Check a harness-level call. `Err` is a refusal the model receives as
    /// the tool result (nothing ran).
    pub(crate) fn plan_extension(
        &self,
        s: &SessionDetail,
        run: &aidex_native_sessions::Run,
        call: &aidex_providers::ToolCall,
        tool: ExtTool,
    ) -> Result<ExtensionPlan, String> {
        let input: Value = serde_json::from_str(&call.arguments)
            .map_err(|_| "The tool arguments are not valid JSON.".to_string())?;
        let permission = s.configuration.permission_mode;
        let mut needs_approval = false;
        let mut preview = None;
        match &tool {
            ExtTool::WebSearch => {
                let args: SearchArgs = parse(&input, "web_search")?;
                let query = args.query.trim();
                if query.is_empty() || query.chars().count() > MAX_QUERY {
                    return Err(format!("The query must be 1-{MAX_QUERY} characters."));
                }
                if args.max_results.is_some_and(|n| !(1..=10).contains(&n)) {
                    return Err("max_results must be 1-10.".into());
                }
                let web = self.extensions.web();
                let (host, port) = web
                    .search_target
                    .clone()
                    .ok_or_else(|| "Web search is not configured.".to_string())?;
                self.network_gate(
                    s,
                    &host,
                    port,
                    &format!("search the web with {}", web.provider),
                )?;
            }
            ExtTool::WebFetch => {
                let args: FetchArgs = parse(&input, "web_fetch")?;
                let (host, port) = url_target(&args.url)?;
                self.network_gate(s, &host, port, &format!("fetch a web page at {host}"))?;
                // Reading an arbitrary page is reviewed in Supervised mode; Auto
                // and Full access allow it.
                needs_approval = matches!(
                    permission,
                    PermissionMode::Supervised | PermissionMode::AutoAcceptEdits
                );
                preview = Some(json!({"tool": "web.fetch", "url": args.url.trim(), "host": host}));
            }
            ExtTool::SkillLoad => {
                let args: SkillArgs = parse(&input, "skill_load")?;
                if args.name.trim().is_empty() {
                    return Err("A skill name is required.".into());
                }
            }
            ExtTool::ArtifactCreate | ExtTool::ArtifactUpdate | ExtTool::PlanPropose => {
                let content_len = match &tool {
                    ExtTool::ArtifactCreate => {
                        let args: CreateArgs = parse(&input, "artifact_create")?;
                        if matches!(args.kind, ArtifactKind::Plan | ArtifactKind::WebPage) {
                            return Err(
                                "Use plan_propose for plans; web_page artifacts come from web_fetch."
                                    .into(),
                            );
                        }
                        args.content.len()
                    }
                    ExtTool::ArtifactUpdate => parse::<UpdateArgs>(&input, "artifact_update")?
                        .content
                        .len(),
                    _ => {
                        let args: PlanArgs = parse(&input, "plan_propose")?;
                        if args.title.trim().is_empty() || args.title.len() > 200 {
                            return Err("The plan title must be 1-200 bytes.".into());
                        }
                        args.markdown.len()
                    }
                };
                if content_len > MAX_ARTIFACT_CONTENT {
                    return Err(format!(
                        "The content is larger than {MAX_ARTIFACT_CONTENT} bytes."
                    ));
                }
                let intent = Intent::new(
                    Action::ArtifactCreate,
                    EntityId::new(&s.id).map_err(|e| e.to_string())?,
                    "save a conversation artifact",
                );
                let decision = self.decide(s, intent)?;
                if decision.verdict.is_deny() {
                    return Err(format!("Policy denied the artifact: {}.", decision.reason));
                }
            }
            ExtTool::Mcp(binding) => {
                if !input.is_object() {
                    return Err("MCP tool arguments must be a JSON object.".into());
                }
                needs_approval = self.mcp_policy(s, binding)?;
                needs_approval |= match permission {
                    PermissionMode::FullAccess => false,
                    _ => !binding.auto_approve,
                };
                preview = Some(json!({
                    "tool": "mcp.call",
                    "server": binding.server_id,
                    "server_name": binding.server_name,
                    "server_tool": binding.tool,
                }));
            }
        }
        let target = match &tool {
            ExtTool::Mcp(binding) => json!([binding.server_id, binding.tool]),
            _ => Value::Null,
        };
        let fingerprint = digest(
            json!({
                "session": s.id,
                "run": run.id,
                "call": call.id,
                "tool": call.name,
                "target": target,
                "input": input,
                "permission": permission,
            })
            .to_string()
            .as_bytes(),
        );
        Ok(ExtensionPlan {
            call_id: EntityId::generate("call").map_err(|e| e.to_string())?,
            tool,
            input,
            needs_approval,
            fingerprint,
            preview,
        })
    }
}
