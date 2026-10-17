# Sessions feature

Owns Session history (`history` route) and the shared live session list.

- `HistoryPage` renders `LiveHistoryPage`: saved conversations from `conversation.list`, 50 per page with Previous and Next and the total. Search filters the current page. Each row opens its chat or agent conversation, shows its state and mode and its creation date (`formatDateTime`) in two meta columns, and can be pinned (a pin toggle of fixed size, so rows keep their columns) or have its history deleted (after a confirmation dialog that starts on Cancel and reports a failure in it; project files are kept). Locked conversations ask for the vault to be unlocked.
- `HistoryMessages` pages through stored messages (`history.list`, 20 at a time) and renders them with the chat feature's `Transcript`, so tool calls, artifacts, and plans look as they do in the conversation (rows whose result is on another page show no status). With `live` it re-reads the page every second, and once more when `live` turns off. The execution page and a conversation's "Earlier messages" use it.
- `LiveSessionSidebar` lists recent conversations (refreshed every 2 s) for the chat, agent, execution, history and checkpoints pages; `HistorySidebar` uses it.
- `pageRange.ts` formats "21–40 of 75" labels (an empty page reads "0 of N").

Tests (`pageRange.test.ts`, `node:test`) run with `bun test src/features/sessions`.
