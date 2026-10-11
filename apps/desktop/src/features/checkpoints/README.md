# Checkpoints and recovery

`CheckpointsPage` reads paginated `checkpoint.list` records for the selected saved conversation. Original content is paged through `checkpoint.content`, current workspace content is read through the confined workspace API, and `checkpoint.compare` checks whether restoration would conflict with later edits.

Restore requires explicit confirmation and the saved digest. The backend refuses conflicting changes and owns restoration. Async content responses are bound to the selected conversation/checkpoint so a late read cannot replace another selection. Review routes to this same live page. External vendor file writes only have checkpoints when the runtime supplies verified capture support; a generic external run is not presented as a complete workspace snapshot.
