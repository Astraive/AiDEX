//! Running a planned harness-level call and recording its result.
//!
//! Results are stored as produced (JSON for web search and fetch and for
//! artifacts, so the desktop can render them); untrusted results are wrapped
//! when the next prompt is assembled.
use super::{
    ExtTool, ExtensionPlan,
    plan::{CreateArgs, FetchArgs, PlanArgs, SearchArgs, SkillArgs, UpdateArgs},
};
use crate::{NativeEngine, extensions::WebHit, untrusted::bounded};
use aidex_core::CancelToken;
use aidex_events::EventKind;
use aidex_native_sessions::{ArtifactKind, ArtifactMeta, NewArtifact, Run, RunStatus};
use serde::Serialize;
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    time::Instant,
};

/// Page text returned inline by `web_fetch`; longer pages are saved as a
/// `web_page` artifact and only this much is returned.
pub(crate) const FETCH_INLINE_BYTES: usize = 64 * 1024;
/// The failure the model receives when a tool panicked. An MCP call may have
/// reached its server first, so effects are not ruled out.
const TOOL_CRASHED: &str =
    "the tool crashed before returning a result; any effects it had are unknown";
/// Largest MCP result passed back to the model.
const MAX_MCP_TEXT: usize = 256 * 1024;
const MAX_TITLE: usize = 160;
const MAX_SNIPPET: usize = 280;
const MAX_RESULT_URL: usize = 2048;

/// `web_search` result, as the desktop renders it.
#[derive(Serialize)]
struct SearchResult<'a> {
    query: &'a str,
    provider: &'a str,
    results: Vec<WebHit>,
}

/// A saved artifact referenced from a result.
#[derive(Serialize)]
struct ArtifactRef<'a> {
    id: &'a str,
    name: &'a str,
    kind: ArtifactKind,
    size: u64,
}

/// `web_fetch` result. Field order keeps the page's identity first, so a
/// bounded prefix of the stored text still shows what was read.
#[derive(Serialize)]
struct FetchResult<'a> {
    url: &'a str,
    final_url: &'a str,
    status: u16,
    content_type: &'a str,
    title: Option<&'a str>,
    /// The page text was cut to the inline limit (the artifact has all of it).
    text_truncated: bool,
    /// The fetch itself reached the client's size limit.
    truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    artifact: Option<ArtifactRef<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    artifact_error: Option<String>,
    text: &'a str,
}

#[derive(Serialize)]
struct ArtifactResult<'a> {
    artifact: &'a ArtifactMeta,
}

#[derive(Serialize)]
struct PlanResult<'a> {
    title: &'a str,
    artifact: &'a ArtifactMeta,
}

fn cut(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.len() <= max {
        return text.to_owned();
    }
    format!("{}…", bounded(text, max))
}

fn to_json<T: Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(|e| e.to_string())
}

fn args<T: for<'de> serde::Deserialize<'de>>(plan: &ExtensionPlan) -> Result<T, String> {
    serde_json::from_value(plan.input.clone()).map_err(|_| "Invalid tool arguments.".to_string())
}

/// A page name for a saved web page: its title, else its host.
fn page_name(title: Option<&str>, url: &str) -> String {
    let base = title
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .or_else(|| url::Url::parse(url).ok()?.host_str().map(str::to_owned))
        .unwrap_or_else(|| "web page".into());
    let clean: String = base
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    bounded(clean.trim(), 200).to_owned()
}

impl NativeEngine {
    /// Run `plan` (approved or not needing approval) and append its result to
    /// the run. Tool failures, panics included, become results; only store
    /// errors and cancellation are `Err`.
    pub(crate) fn execute_extension(
        &self,
        id: &str,
        call: &aidex_providers::ToolCall,
        plan: &ExtensionPlan,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        let run = self.store.run(id)?;
        if run.status == RunStatus::Cancelled {
            return Err("run cancelled".into());
        }
        self.emit(
            &run.session_id,
            EventKind::ToolStarted {
                call_id: plan.call_id.clone(),
            },
        );
        let started = Instant::now();
        // A panicking extension (an HTML extractor, an MCP client, a skill
        // loader) fails this call only; no engine lock is held while it runs.
        let outcome = catch_unwind(AssertUnwindSafe(|| self.run_extension(&run, plan, cancel)))
            .unwrap_or_else(|_| Err(TOOL_CRASHED.into()));
        if cancel.is_cancelled() {
            return Err("run cancelled".into());
        }
        let content = match outcome {
            Ok(content) => {
                self.emit(
                    &run.session_id,
                    EventKind::ToolCompleted {
                        call_id: plan.call_id.clone(),
                        duration_ms: started.elapsed().as_millis() as u64,
                        summary: "native tool completed; output in encrypted history".into(),
                    },
                );
                content
            }
            Err(message) => {
                self.emit(
                    &run.session_id,
                    EventKind::ToolFailed {
                        call_id: plan.call_id.clone(),
                        code: "extension_tool_failed".into(),
                        message: "native tool failed; details in encrypted run history".into(),
                    },
                );
                format!(
                    "Tool failed: {}",
                    aidex_events::redact_text(bounded(&message, 4096))
                )
            }
        };
        self.append_result(id, &call.id, content)
    }

    fn run_extension(
        &self,
        run: &Run,
        plan: &ExtensionPlan,
        cancel: &CancelToken,
    ) -> Result<String, String> {
        let s = self.store.get(&run.session_id)?;
        let workspace = s.configuration.workspace_id.as_deref();
        match &plan.tool {
            ExtTool::WebSearch => {
                let args: SearchArgs = args(plan)?;
                // The settings or policy may have changed while waiting.
                let web = self.extensions.web();
                let (host, port) = web
                    .search_target
                    .clone()
                    .ok_or_else(|| "Web search is not configured.".to_string())?;
                self.network_gate(
                    &s,
                    &host,
                    port,
                    &format!("search the web with {}", web.provider),
                )?;
                let query = args.query.trim();
                let (provider, hits) = self
                    .extensions
                    .web_search(query, args.max_results, cancel)
                    .map_err(|e| e.to_string())?;
                let results = hits
                    .into_iter()
                    .take(10)
                    .map(|hit| WebHit {
                        title: cut(&hit.title, MAX_TITLE),
                        url: bounded(hit.url.trim(), MAX_RESULT_URL).to_owned(),
                        snippet: cut(&hit.snippet, MAX_SNIPPET),
                        published: hit.published.map(|p| cut(&p, 64)),
                    })
                    .collect();
                to_json(&SearchResult {
                    query,
                    provider: &provider,
                    results,
                })
            }
            ExtTool::WebFetch => {
                let args: FetchArgs = args(plan)?;
                let (host, port) = super::plan::url_target(&args.url)?;
                self.network_gate(&s, &host, port, &format!("fetch a web page at {host}"))?;
                let page = self
                    .extensions
                    .web_fetch(args.url.trim(), cancel)
                    .map_err(|e| e.to_string())?;
                let long = page.text.len() > FETCH_INLINE_BYTES;
                let mut saved = None;
                let mut artifact_error = None;
                if long {
                    let name = page_name(page.title.as_deref(), &page.final_url);
                    let body = format!("Source: <{}>\n\n{}", page.final_url, page.text);
                    match self.store.create_artifact(
                        &s.id,
                        Some(&run.id),
                        NewArtifact {
                            name,
                            kind: ArtifactKind::WebPage,
                            mime: "text/markdown".into(),
                            language: None,
                            bytes: body.into_bytes(),
                        },
                    ) {
                        Ok(meta) => saved = Some(meta),
                        Err(error) => {
                            artifact_error = Some(aidex_events::redact_text(&error).into_owned())
                        }
                    }
                }
                to_json(&FetchResult {
                    url: &page.url,
                    final_url: &page.final_url,
                    status: page.status,
                    content_type: &page.content_type,
                    title: page.title.as_deref(),
                    text_truncated: long,
                    truncated: page.truncated,
                    artifact: saved.as_ref().map(|meta| ArtifactRef {
                        id: &meta.id,
                        name: &meta.name,
                        kind: meta.kind,
                        size: meta.size,
                    }),
                    artifact_error,
                    text: bounded(&page.text, FETCH_INLINE_BYTES),
                })
            }
            ExtTool::SkillLoad => {
                let args: SkillArgs = args(plan)?;
                let skill = self
                    .extensions
                    .load_skill(workspace, args.name.trim())
                    .map_err(|e| e.to_string())?;
                Ok(format!(
                    "Skill: {}\nDescription: {}\n\n{}",
                    skill.name, skill.description, skill.body
                ))
            }
            ExtTool::ArtifactCreate => {
                let args: CreateArgs = args(plan)?;
                let mime = args
                    .mime
                    .filter(|m| !m.trim().is_empty())
                    .unwrap_or_else(|| args.kind.default_mime().into());
                let meta = self.store.create_artifact(
                    &s.id,
                    Some(&run.id),
                    NewArtifact {
                        name: args.name,
                        kind: args.kind,
                        mime,
                        language: args.language,
                        bytes: args.content.into_bytes(),
                    },
                )?;
                to_json(&ArtifactResult { artifact: &meta })
            }
            ExtTool::ArtifactUpdate => {
                let args: UpdateArgs = args(plan)?;
                let existing = self
                    .store
                    .artifact(args.id.trim())
                    .ok()
                    .filter(|meta| meta.session_id == s.id)
                    .ok_or_else(|| {
                        "No artifact with that id exists in this conversation.".to_string()
                    })?;
                // An edited plan would stay active while its Plan card still
                // shows (and implements) the proposed text; a saved page keeps
                // its source.
                match existing.kind {
                    ArtifactKind::Plan => {
                        return Err("Plans cannot be edited in place; call plan_propose with \
                                    the revised plan, which replaces the active one."
                            .into());
                    }
                    ArtifactKind::WebPage => {
                        return Err("Saved web pages cannot be edited; use artifact_create \
                                    for your own document."
                            .into());
                    }
                    _ => {}
                }
                let meta = self
                    .store
                    .update_artifact(&existing.id, args.content.into_bytes())?;
                to_json(&ArtifactResult { artifact: &meta })
            }
            ExtTool::PlanPropose => {
                let args: PlanArgs = args(plan)?;
                let title = args.title.trim();
                let markdown = if args.markdown.trim_start().starts_with('#') {
                    args.markdown
                } else {
                    format!("# {title}\n\n{}", args.markdown)
                };
                let meta = self.store.create_artifact(
                    &s.id,
                    Some(&run.id),
                    NewArtifact {
                        name: title.into(),
                        kind: ArtifactKind::Plan,
                        mime: "text/markdown".into(),
                        language: None,
                        bytes: markdown.into_bytes(),
                    },
                )?;
                to_json(&PlanResult {
                    title,
                    artifact: &meta,
                })
            }
            ExtTool::Mcp(binding) => {
                // The policy may have changed while the call waited for approval.
                self.mcp_policy(&s, binding)?;
                let output = self
                    .extensions
                    .mcp_call(
                        &binding.server_id,
                        &binding.tool,
                        plan.input.clone(),
                        cancel,
                    )
                    .map_err(|e| e.to_string())?;
                let mut text = bounded(&output.text, MAX_MCP_TEXT).to_owned();
                if output.truncated || output.text.len() > MAX_MCP_TEXT {
                    text.push_str("\n[output truncated]");
                }
                if output.is_error {
                    text = format!("The MCP tool reported an error:\n{text}");
                }
                Ok(text)
            }
        }
    }
}
