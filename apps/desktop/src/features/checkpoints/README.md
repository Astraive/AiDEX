# Checkpoints feature

Owns Checkpoints & Recovery (`checkpoints` route): saved workspace states you can resume, restore, branch from, or compare with the current workspace.

- `CheckpointsPage` lists the current workspace's checkpoints in a `Timeline` beside `CheckpointDetail`. `route.id` selects a checkpoint and falls back to the most recent one. Workspaces without checkpoints show an empty state.
- `CheckpointDetail` holds the actions. Compare toggles `CheckpointComparison` (a representative diff plus every changed file). Restore first shows `RestoreConfirmation` inline.
- `CheckpointsSidebar` has a workspace picker (the shared `workspaceId` preference), the checkpoint list, and links to Review and the agent task.

Nothing is restored, resumed, branched, or saved yet. Those actions show "Preview" toasts. Checkpoints, snapshot details, and diffs are sample data in `sample.ts` until AiDEX Core exposes durable checkpoints.
