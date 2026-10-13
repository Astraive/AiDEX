# Checkpoints feature

Owns Checkpoints and changes (`checkpoints` route, `id` = session id): snapshots of files taken before an agent changed them, which can be inspected and restored.

- `CheckpointsPage` picks a saved conversation and pages through its checkpoints with `checkpoint.list`, 20 at a time. It opens on the newest page (and keeps following it as checkpoints are added) and lists each page newest first; Older and Newer move between pages without overlap. The paging rules are pure functions in `checkpointPages.ts`.
- The list is re-read on the conversation's durable events (`useSessionUpdates`, `../../hooks`), and every 5 s while that stream is down.
- Selecting a checkpoint reads the current file through `workspace.files.read`, checks it with `checkpoint.compare`, and pages its original content through `checkpoint.content`. Answers are bound to the selection (`latestRequest.ts`), so a late read for an earlier checkpoint, page, or conversation never replaces what is shown.
- Restore needs a confirmation and the checkpoint's digest; the daemon refuses it when the file changed afterwards (`conflict`). After a restore the conversation and the list are re-read.
- `CheckpointsSidebar` is the live session list.

Tests (`checkpointPages.test.ts`, `latestRequest.test.ts`, `node:test`) run with `bun test src/features/checkpoints`.
