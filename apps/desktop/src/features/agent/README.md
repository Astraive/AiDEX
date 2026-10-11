# Agent execution

`AgentPage` uses the live `ConversationPage` in Agent mode. `ExecutionPage` uses `LiveExecutionPage` to inspect root and child run status, limits, errors, history, cancellation, and checkpoints.

`ProcessOutputPanel` shows bounded stdout/stderr excerpts and exit codes for mediated processes in the conversation. Streams remain separate, including interleaved child processes. Final output is read from encrypted tool history; ephemeral chunks are not advertised as replayable terminal logs.

`useSessionUpdates` subscribes to scoped Core events, resumes durable cursors after connection loss, and reloads the saved snapshot when replay history has expired. Root `agent.delta` and correlated `agent.run_delta` events are ephemeral and never deduplicated by cursor equality. Active child deltas appear on their own execution cards; saved transcripts become authoritative after completion. History inspectors refetch while their selected run is active.

Runtime capabilities determine whether resume can be offered. Cancelled runs do not expose a resume action because the backend requires an interrupted execution. Legacy sample panels are not routed by the application.
