# Workspace feature

Screens for finding, adding, and inspecting workspaces. Everything is composed from `@aidex/ui`; the feature has no styles of its own. Data comes from sample files until AiDEX Core exposes workspace APIs, and every backend action shows a "Preview: …" toast instead.

## Routes

| Route | Page · Sidebar | What it shows |
| --- | --- | --- |
| `workspaces` | `WorkspacesPage` · `WorkspacesSidebar` | Searchable list of workspaces with an All · Pinned · On drive · This computer filter. The filter lives in `route.tab`, so the sidebar's "All workspaces" item can reset it. Rows open the workspace and make it the default for chats and agent runs. Remove registration asks in a confirmation dialog (Cancel first; a failure shows inside it; project files are kept), then moves focus to the row that took its place, or to the page title once none is left (`useRemovalFocus`). |
| `workspace-create` | `CreateWorkspacePage` · `CreateWorkspaceSidebar` | Add a local folder, a git clone, or a drive project, with inline validation. The current setup step (`source`, `details`, `start`) is mirrored into `route.tab` so the sidebar shows progress; its step items move focus to that part of the form. |
| `workspace` (id = workspace id) | `WorkspaceOverviewPage` · `WorkspaceOverviewSidebar` | Workspace header (branch, location, languages) and the tabs from `pageTabs.workspace`, bound to `route.tab`. The sidebar switches workspace and tab and lists recent sessions. |

## Workspace tabs

- `OverviewPanel`: stat tiles, file tree preview, recent activity, environment summary.
- `FilesPanel`: selectable file tree with a code preview, copy, and "Ask about this file" (hands a prompt to chat).
- `GitPanel`: branch, changed files (open Review), recent commits.
- `EnvironmentPanel`: detected toolchains table and runtime restrictions.
- `SessionsPanel`: this workspace's chats and agent runs.
- `MemoryPanel` (Context & memory): index status and Reindex, `RetrievalTest` (local term-overlap scoring in `retrieval.ts`), `MemoryEntries` (edit or hide entries locally), and context settings switches.

## Supporting files

- `sample.ts`: index status, activity, memory entries, and drive projects.
- `sampleProjects.ts`: per-language project profiles (tree, file snippets, git changes, commits, toolchains); a workspace uses the profile of its primary language.
- `createWorkspaceForm.ts`: form model, validation, suggested names, and step derivation.
- `workspaceFilters.ts`, `workspaceTabs.ts`: filter options and tab labels/icons shared by pages and sidebars.
