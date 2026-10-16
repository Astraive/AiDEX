# Diff review feature

The Review changes route (`review`), composed only from `@aidex/ui`. AiDEX Core does not expose workspace diffs or test runs yet, so this route shows the live checkpoint view instead of a sample diff.

- `ReviewPage.tsx` renders `../checkpoints/CheckpointsPage`: pick a saved conversation (its id becomes the route `id`), page through the checkpoints taken before an agent changed a file, compare one with the current file, read its original content, and restore it. See `../checkpoints/README.md`.
- `ReviewSidebar.tsx` renders the live session list (`../sessions/LiveSessionSidebar`) titled Review; selecting a conversation opens its checkpoints on this route.

A per-file diff review and verification results return here once AiDEX Core reports workspace diffs and test runs; they will render through `CodeDiff` from `@aidex/ui`.
