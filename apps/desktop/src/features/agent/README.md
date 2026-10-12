# Agent feature

Agent mode and run inspection, composed only from `@aidex/ui`. Nothing executes yet; the run is a local, scripted preview.

- `AgentPage.tsx` (route `agent`, `id` = session id): run header (task name, status, workspace, branch, autonomy), Plan and Activity split, tools and permissions, and a docked composer whose instructions are appended to the log.
- `AgentPanels.tsx`: the Plan, Activity, and Tools and permissions sections.
- `useDemoRun.ts`: local run state for Pause/Resume, Stop, Advance demo, and instructions.
- `useTaskName.ts`: task name from a handed-over prompt, the session, or the sample task; shared by page and sidebar.
- `ExecutionPage.tsx` (route `execution`, tabs `execution` · `graph`): task progress, tool calls table, and terminal output; or the task graph with a subagent list (`ExecutionPanels.tsx`).
- `AgentSidebar.tsx`, `ExecutionSidebar.tsx`: contextual navigation for the two pages.
- `sample.ts`: plan, log, tool calls, terminal output, and subagents until AiDEX Core exposes live runs.
