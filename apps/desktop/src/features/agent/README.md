# Agent feature

Agent mode and run inspection, composed only from `@aidex/ui`.

- `AgentPage.tsx` (route `agent`, `id` = session id) renders the live `ConversationPage` in Agent mode.
- `ExecutionPage.tsx` (route `execution`, `id` = session id) renders `LiveExecutionPage`: each root and child run with its status, steps, model and output limit, inference retries and context trimming (`Run.inference_retries`, `Run.context_trimmed_messages`), error, queued tool calls and Cancel, plus links to the conversation and its checkpoints. "Inspect tool and model history" shows the run's saved messages through `HistoryMessages`, re-read every second while the run is active.
- Live updates come from `useSessionUpdates` (`../../hooks`). Durable events of the session reload the snapshot at once; the page also polls every second while a run is active or the stream is down, and every 5 s otherwise. While the stream reconnects a warning says that live output sent meanwhile cannot be replayed.
- `executionOutput.ts` keeps bounded live excerpts. `process.started`, `process.output` and `process.exited` build one trace per process: stdout and stderr separately, the latest 64 KiB of each, the exit code or signal, for at most eight processes (ended ones are dropped first, then the least recently active). When the stream drops, running processes are marked as unknown and their excerpts as incomplete. `agent.run_delta` text is kept per run (`agent_id`, root or child), the latest 16 KiB of up to 16 runs, and shown on the run's card while it is active; the saved transcript is authoritative once it ends.
- `ProcessOutputPanel.tsx` shows the traces, newest activity first.
- `AgentSidebar.tsx`, `ExecutionSidebar.tsx`: the live session list (`../sessions/LiveSessionSidebar`).

Tests (`executionOutput.test.ts`, `node:test`) run with `bun test src/features/agent`.
