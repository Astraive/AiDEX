//! Bounded, confined inspection explicitly requested by the desktop user.
use crate::{NativeEngine, launcher::NativeLauncher};
use aidex_core::EntityId;
use aidex_tools::{ExecutionPermit, PermitIssuer, ToolContext, ToolId};
use std::sync::Arc;
impl NativeEngine {
    pub fn inspect(
        &self,
        workspace: Option<&str>,
        tool: &str,
        input: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let workspace = match workspace {
            Some(w) => EntityId::new(w).map_err(|e| e.to_string())?,
            None => {
                self.broker
                    .workspaces
                    .selected()
                    .map_err(|e| e.to_string())?
                    .ok_or("no workspace selected")?
                    .id
            }
        };
        let handle = self
            .broker
            .workspaces
            .handle(&workspace)
            .map_err(|e| e.to_string())?;
        let context = ToolContext::new(
            EntityId::new("desktop_inspection").map_err(|e| e.to_string())?,
            workspace,
            EntityId::generate("call").map_err(|e| e.to_string())?,
            Arc::new(handle.root().clone()),
            Arc::new(aidex_events::NullEventSink),
        );
        let tool = ToolId::new(tool).map_err(|e| e.to_string())?;
        let descriptor = self
            .broker
            .tools
            .descriptor(&tool)
            .ok_or("unknown inspection tool")?;
        if !descriptor.risk.is_read_only() {
            return Err("desktop inspection accepts read-only tools only".into());
        }
        let plan = self
            .broker
            .tools
            .plan(&context, &tool, input)
            .map_err(|e| e.to_string())?;
        let launcher = Arc::new(NativeLauncher::new(
            self.broker.sandbox.clone(),
            self.broker
                .workspaces
                .paths()
                .sessions()
                .join("inspection-processes"),
            plan.fingerprint(),
            aidex_sandbox::SandboxLevel::Brokered,
        ));
        let context = context.with_launcher(launcher);
        let output = self
            .broker
            .tools
            .execute(
                &context,
                &plan,
                ExecutionPermit::for_plan(&plan, PermitIssuer::ReadOnly),
            )
            .map_err(|e| e.to_string())?;
        Ok(output.content)
    }
}
