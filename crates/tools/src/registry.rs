//! The tool registry: discovery, planning, permit-checked execution, write locks,
//! and lifecycle events.

use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    fmt,
    sync::{Mutex, MutexGuard, PoisonError},
    time::Instant,
};

use aidex_core::{EntityId, WorkspacePath};
use aidex_events::{EventKind, redact_text};
use serde_json::Value;

use crate::{
    ExecutionPermit, Tool, ToolContext, ToolDescriptor, ToolError, ToolId, ToolOutput, ToolPlan,
    ToolSpec,
    locks::{LockKey, WriteLocks},
    text,
};

/// Plans remembered as executed (oldest forgotten first). Plan ids are random and
/// unique, so forgetting only weakens duplicate detection for very old plans.
const EXECUTED_CAPACITY: usize = 65_536;
/// Most paths listed in one `diff.created` event and most `file.changed` events per call.
const MAX_EVENT_PATHS: usize = 1_000;
/// Longest `tool.failed` message in bytes.
const MAX_FAILURE_MESSAGE: usize = 1_024;

#[derive(Default)]
struct Executed {
    ids: HashSet<EntityId>,
    order: VecDeque<EntityId>,
}

/// Owns the registered tools. Only the execution broker should hold a registry:
/// it plans calls, evaluates policy on the plan's facts, obtains approvals, and then
/// executes with a permit. The harness and agents talk to the broker, never to this
/// type, so no write or subprocess can bypass policy.
pub struct ToolRegistry {
    tools: BTreeMap<ToolId, Box<dyn Tool>>,
    model_names: HashMap<String, ToolId>,
    locks: WriteLocks,
    executed: Mutex<Executed>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self {
            tools: BTreeMap::new(),
            model_names: HashMap::new(),
            locks: WriteLocks::default(),
            executed: Mutex::new(Executed::default()),
        }
    }

    /// A registry with every built-in tool (see the crate README for the table).
    pub fn with_builtin_tools() -> Self {
        let mut registry = Self::new();
        for tool in crate::builtin::tools() {
            // Built-in descriptors are fixed and distinct.
            let _ = registry.register(tool);
        }
        registry
    }

    /// Register a tool. Ids and model-facing names must be unique and both schemas
    /// must describe JSON objects.
    pub fn register(&mut self, tool: Box<dyn Tool>) -> Result<(), ToolError> {
        let descriptor = tool.descriptor();
        let id = descriptor.id.clone();
        if self.tools.contains_key(&id) {
            return Err(ToolError::Registration(format!(
                "`{id}` is already registered"
            )));
        }
        let model_name = id.model_name();
        if let Some(existing) = self.model_names.get(&model_name) {
            return Err(ToolError::Registration(format!(
                "`{id}` and `{existing}` share the model name `{model_name}`"
            )));
        }
        for (which, schema) in [
            ("input", &descriptor.input_schema),
            ("output", &descriptor.output_schema),
        ] {
            if schema.get("type") != Some(&Value::String("object".into())) {
                return Err(ToolError::Registration(format!(
                    "`{id}` {which} schema must describe a JSON object"
                )));
            }
        }
        self.model_names.insert(model_name, id.clone());
        self.tools.insert(id, tool);
        Ok(())
    }

    pub fn descriptor(&self, id: &ToolId) -> Option<&ToolDescriptor> {
        self.tools.get(id).map(|tool| tool.descriptor())
    }

    /// Every descriptor (with capability and risk metadata), ordered by id.
    pub fn descriptors(&self) -> Vec<&ToolDescriptor> {
        self.tools.values().map(|tool| tool.descriptor()).collect()
    }

    /// Model-facing specs, ordered by id.
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools
            .values()
            .map(|tool| tool.descriptor().spec())
            .collect()
    }

    /// The tool id behind a model-facing name (`fs_read` → `fs.read`).
    pub fn resolve_model_name(&self, name: &str) -> Option<&ToolId> {
        self.model_names.get(name)
    }

    /// Phase one: validate `input` and compute the plan. No side effects.
    pub fn plan(
        &self,
        ctx: &ToolContext,
        tool_id: &ToolId,
        input: Value,
    ) -> Result<ToolPlan, ToolError> {
        let tool = self
            .tools
            .get(tool_id)
            .ok_or_else(|| ToolError::UnknownTool(tool_id.to_string()))?;
        crate::context::check_cancel(&ctx.cancel)?;
        let plan = tool.plan(ctx, input)?;
        if plan.tool_id() != tool_id {
            return Err(ToolError::PlanMismatch(format!(
                "`{tool_id}` returned a plan for `{}`",
                plan.tool_id()
            )));
        }
        plan.check_context(ctx)?;
        Ok(plan)
    }

    /// Phase two: execute exactly `plan`, authorized by `permit` (consumed).
    ///
    /// Refuses a permit for another plan, a plan altered after sealing, a context
    /// for another workspace/session/call/root, a read-only permit for a writing
    /// tool, and a plan that was already executed. Emits `tool.started` (if that
    /// fails nothing runs), then `file.changed`/`diff.created` for writes and
    /// `tool.completed` or `tool.failed`. Writers to overlapping paths serialize.
    pub fn execute(
        &self,
        ctx: &ToolContext,
        plan: &ToolPlan,
        permit: ExecutionPermit,
    ) -> Result<ToolOutput, ToolError> {
        let tool = self
            .tools
            .get(plan.tool_id())
            .ok_or_else(|| ToolError::UnknownTool(plan.tool_id().to_string()))?;
        permit.authorize(plan, tool.descriptor())?;
        plan.check_context(ctx)?;
        crate::context::check_cancel(&ctx.cancel)?;
        self.claim(plan.plan_id())?;
        let started = Instant::now();
        let call_id = ctx.call_id.clone();
        if let Err(error) = ctx.events.emit(
            ctx.scope(),
            EventKind::ToolStarted {
                call_id: call_id.clone(),
            },
        ) {
            self.release(plan.plan_id());
            return Err(ToolError::Events(error));
        }
        let root = plan.root().to_owned();
        let keys = tool
            .write_locks(plan)
            .iter()
            .map(|path| LockKey::new(&root, path))
            .collect();
        let result = self
            .locks
            .acquire(keys, &ctx.cancel)
            .and_then(|_guard| tool.execute(ctx, plan, &permit));
        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        match result {
            Ok(mut output) => {
                let mut warnings = record_changes(ctx, &output);
                let completed = EventKind::ToolCompleted {
                    call_id,
                    duration_ms,
                    summary: output.summary.clone(),
                };
                if let Err(error) = ctx.events.emit(ctx.scope(), completed) {
                    warnings.push(format!("tool.completed was not recorded: {error}"));
                }
                output.warnings.extend(warnings);
                Ok(output)
            }
            Err(error) => {
                let failed = EventKind::ToolFailed {
                    call_id,
                    code: error.reason().to_owned(),
                    message: text::bounded(&error.to_string(), MAX_FAILURE_MESSAGE),
                };
                // The failure is returned either way; a lost event cannot be retried.
                let _ = ctx.events.emit(ctx.scope(), failed);
                Err(error)
            }
        }
    }

    fn executed(&self) -> MutexGuard<'_, Executed> {
        self.executed.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn claim(&self, plan_id: &EntityId) -> Result<(), ToolError> {
        let mut executed = self.executed();
        if !executed.ids.insert(plan_id.clone()) {
            return Err(ToolError::AlreadyExecuted);
        }
        executed.order.push_back(plan_id.clone());
        while executed.order.len() > EXECUTED_CAPACITY {
            if let Some(oldest) = executed.order.pop_front() {
                executed.ids.remove(&oldest);
            }
        }
        Ok(())
    }

    fn release(&self, plan_id: &EntityId) {
        let mut executed = self.executed();
        executed.ids.remove(plan_id);
        executed.order.retain(|id| id != plan_id);
    }
}

/// Emit `file.changed` per changed path and `diff.created` for an applied diff;
/// returns warnings for events that could not be recorded.
fn record_changes(ctx: &ToolContext, output: &ToolOutput) -> Vec<String> {
    let mut warnings = Vec::new();
    for change in output.changes.iter().take(MAX_EVENT_PATHS) {
        let event = EventKind::FileChanged {
            path: change.path.clone(),
            change: change.change,
        };
        if let Err(error) = ctx.events.emit(ctx.scope(), event) {
            warnings.push(format!("file.changed was not recorded: {error}"));
        }
    }
    if output.changes.len() > MAX_EVENT_PATHS {
        warnings.push(format!(
            "{} further file.changed events were not emitted",
            output.changes.len() - MAX_EVENT_PATHS
        ));
    }
    if let Some(diff) = &output.diff {
        let files: Vec<WorkspacePath> = diff.paths().into_iter().take(MAX_EVENT_PATHS).collect();
        let event = EntityId::generate("diff")
            .map_err(|error| error.to_string())
            .and_then(|diff_id| {
                ctx.events
                    .emit(
                        ctx.scope(),
                        EventKind::DiffCreated {
                            diff_id,
                            files,
                            additions: diff.additions(),
                            deletions: diff.deletions(),
                        },
                    )
                    .map_err(|error| error.to_string())
            });
        if let Err(error) = event {
            warnings.push(format!(
                "diff.created was not recorded: {}",
                redact_text(&error)
            ));
        }
    }
    warnings
}

impl fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolRegistry")
            .field("tools", &self.tools.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}
