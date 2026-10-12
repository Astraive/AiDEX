# Agent center feature

Owns the Agents catalog (`agents` route) and Agent configuration (`agent-config` route, `id` = agent id).

- `AgentCenterPage` shows built-in agents as `BuiltInAgentCard`s (Configure, Set as default) and optional external runtimes in `ExternalRuntimeList` (Configure or Install, plus Add custom runtime). The default agent is local page state for this session.
- `AgentConfigPage` edits one agent through `AgentConfigForm`: instructions, default and fallback model, permissions (`PermissionsSection`), and harness limits (`HarnessSection`). The form validates the step and timeout limits. Save shows a Preview toast; Reset restores the sample defaults. Unknown ids fall back to AiDEX Native.
- `AgentCenterSidebar` is shared by both pages. The agent being configured is the active item.
- `useAgentActions` holds navigation and Preview actions shared by the catalog and the sidebar.

Agents, runtimes, and their settings are sample data in `sample.ts` until AiDEX Core exposes the agent registry and runtime adapters. Nothing is installed or saved.
