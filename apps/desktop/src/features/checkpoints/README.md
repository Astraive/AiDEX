# Checkpoints feature

Owns Checkpoints and changes (`checkpoints` route, `id` = session id): snapshots of files taken before an agent changed them, which can be inspected and restored.

- `CheckpointsPage` picks a saved conversation and pages through its checkpoints with `checkpoint.list`, 20 at a time. It opens on the newest page (and keeps following it as checkpoints are added) and lists each page newest first; Older and Newer move between pages without overlap. The paging rules are pure functions in `checkpointPages.ts`.
- The list is re-read on the conversation's durable events (`useSessionUpdates`, `../../hooks`), and every 5 s while that stream is down or a read failed (so the page recovers by itself once a locked vault is unlocked again; the drive-wide `vault.unlocked` event never reaches the session stream). Polls run only while the user is present (`usePoll`).
- The open checkpoint's decrypted original and current content close when any of the page's reads is refused because the vault locked (`selectionDropped`), and when the drive or daemon changes or stops, so a locked vault leaves no decrypted content on screen; a lock from outside the app re-reads every query at once (`useVaultLocks`, `../../hooks`), so that happens even while nothing polls.
- Selecting a checkpoint reads the current file through `workspace.files.read`, checks it with `checkpoint.compare`, and pages its original content through `checkpoint.content`. Answers are bound to the selection (`latestRequest.ts`), so a late read for an earlier checkpoint, page, or conversation never replaces what is shown.
- Restore needs a confirmation and the checkpoint's digest; the daemon refuses it when the file changed afterwards (`conflict`). After a restore the conversation and the list are re-read.
- `CheckpointsSidebar` is the live session list.

Tests (`checkpointPages.test.ts`, `latestRequest.test.ts`, `node:test`) run with `bun test src/features/checkpoints`.
