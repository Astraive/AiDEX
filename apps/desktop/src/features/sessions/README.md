# Sessions feature

Owns Session history (`history` route): every chat and agent run, grouped by day (Today, Yesterday, Earlier).

- `HistoryPage` shows a local search field, a kind filter (All · Chat · Agent), and the grouped list. Each row opens its chat or agent run with the session id. An empty state offers "Clear filters".
- `HistorySidebar` holds the Filter section (with counts) and the Workspaces section.
- `HistoryMessages` pages through stored messages (`history.list`, 20 at a time) and renders them with the chat feature's `Transcript`, so tool calls, artifacts, and plans look as they do in the conversation (rows whose result is on another page show no status). Agent execution details and a conversation's "Earlier messages" use it.
- Filters live in the route so page and sidebar stay in sync: `route.tab` is the kind (`all`, `chat`, `agent`) and `route.id` is a workspace id. `historyFilters.ts` validates both, since `pageTabs` has no history entry; unknown values fall back to all sessions.

Sessions come from the shared sample data in `../../lib/sample/sessions.ts` until AiDEX Core exposes session history.
