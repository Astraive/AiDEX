# Desktop features

Each folder owns one area of the desktop UI: its pages, its contextual sidebar, and its data. Screens are composed only from `@aidex/ui` and read live data from AiDEX Core through `../hooks` (`useAidexStatus`, `useRpc`, `useBridgeQuery`, `useSessionUpdates`). No page is sample data, so `samplePages` (`../lib/routes.ts`), which the shell badges "Sample data", is empty. What is not live yet says so where it appears: Settings → Drive shows a host cleanup report titled "Host cleanup (sample)", Settings → Shortcuts marks shortcuts no screen handles yet as Planned, actions that need a missing backend answer with a "Preview" toast, and Remote access says it is coming soon. See `../services/README.md` for wiring a new daemon-backed screen.

Live today: `launch` (drive, AiDEX Core, vault create and unlock, host and sandbox detection), the Home system status with the workspace registry and live events, conversations with the model picker, approvals, Resume, and per-conversation settings, Agents (AiDEX Native defaults and the configured runtimes), Models (providers, agent runtimes with their managed accounts, local models, model details), Plugins (skills, MCP servers, plugins, web search), Workspaces (registry, files, Git, environment, sessions, memory), Session history, Checkpoints (Review changes shows them too), execution details with live run and process output, Security (vault, sandbox, audit, approvals), and Settings (Updates, Diagnostics, Drive).

- `system`: shared live-status pieces: `CoreStatusNotice` (no drive, AiDEX Core stopped with Start, incompatible, outside the app), `RecentEvents` (`events.latest` + `events.replay`), `LiveEvents` (`events.subscribe`), and labels for host, sandbox, drive, and event data.
- `launch`: drive selection, drive and vault check, and host/runtime detection, shown before the workspace.
- `home`: the welcome dashboard, composer, and live system status.
- `workspace`: workspace list, add or import, overview (files, git, environment, sessions), and context and memory.
- `chat`: conversations with attachments, mode switch, the provider-aware model picker (`Mod+Shift+M`), inline approvals, Resume for interrupted runs, and per-conversation settings.
- `agent`: Agent mode (the live conversation) and the execution page: each root and child run with its status, steps, model and output limit, retries, context trimming, and error, its live text while it runs (`agent.run_delta`, keyed by run), queued tool calls and Cancel, the conversation's process output (`process.started`, `process.output`, `process.exited`), and each run's saved tool and model history.
- `diffs`: the Review changes route, which shows the live Checkpoints page and session list until AiDEX Core exposes workspace diffs and test runs.
- `sessions`: Session history (saved conversations, 50 per page, with search within the page, pin, and delete), the paged stored-message view (`HistoryMessages`) that the execution page and conversations share, and the live session list the chat, agent, execution, review, history, and checkpoints sidebars use.
- `checkpoints`: a conversation's checkpoints, 20 per page and newest first, re-read as the conversation's events arrive; a selected checkpoint is compared with the current file and shows its original content, and Restore (after a confirmation) is refused when the file changed since.
- `agent-center`: AiDEX Native and the configured agent runtimes with their status and the default for new chats, and AiDEX Native's defaults (model, permission, instructions, limits); runtime settings open in Models → Providers.
- `model-manager`: the shared provider and model catalog, the Providers screen (accounts, agent runtimes and their managed accounts, add wizard), local models, and model details.
- `extensions`: live skills (search, scope filter, enable, author drive skills), MCP servers (status, start and stop, tools, secrets, logs, `.mcp.json` import), plugins (install, review, trust, enable, uninstall), and web search settings with a test search.
- `security`: security overview, vault and secrets, sandbox and policies, the audit log, and the approval center.
- `approvals`: the approval center (pending and past requests) and the approval card conversations share.
- `remote-sessions`: remote access, shown as coming soon until its backend lands.
- `settings`: appearance, shortcuts, updates, diagnostics, and drive health and cleanup.
- `editor`, `tasks`, `terminal`: reserved for later screens.

## Portable chat and project context

Desktop chat opens after vault unlock, with a compact composer, a model picker popover, and modal controls for mode, project context and attachments. New chats have no project context; the globally selected project is not inherited. Project context is explicitly chosen per conversation, or carried by the user's Ask about this file action. The daemon refuses unknown or unavailable project references before saving configuration. An existing conversation can clear its project context while preserving history; checkpoint-bearing conversations retain their original project binding.

Home and search navigation show the live project registry and encrypted conversation summaries. No sample project or session IDs are submitted. Workspace file navigation reads bounded pages, search opens matching file excerpts, and Git/environment panels use real service reports. The compact top bar exposes search, model management, account settings and drive status through accessible icon actions and dialogs.
