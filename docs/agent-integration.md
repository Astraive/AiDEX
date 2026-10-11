# Complete desktop agent integration

This continues [desktop milestone #55](https://github.com/NeerajCodz/AiDEX/issues/55), synchronized with main `40ed889` including signed updates and host-aware local inference. It supersedes the older CLI-only milestone boundary. The reference is T3 Code `98beed1a226c42b85c52ff5ee9d5dbb67d776a77`. T3 normalizes installed runtime capabilities and events; its providers own many coding tools, search, skills and native subagents. AiDEX keeps its independent Rust cloud-model loop and adds portable extension and desktop paths.

## Owned boundaries

- `aidexd` composes services and thin JSON-RPC handlers. The Tauri bridge remains a transport.
- `aidex-harness` owns native run state, safe tool repair, inference retries, complete context groups, subagent joins, project instructions and external continuation.
- `aidex-tools` owns sealed plans and permits. Every credential reference adds its own `secret.use` policy intent before execution; credentials are resolved only in backend execution.
- `aidex-extensions` owns drive-resident skills, plugin manifests and explicit MCP connections. Catalog reads are passive. Plugin contributions inherit trust and scope. MCP calls recheck configuration and discovered schema revisions after approval.
- Native web tools use an explicit search-provider connection and bounded public-page fetches, with the shared offline gate and vault credentials.
- `aidex-runtime-bridge` supplies authenticated, ephemeral MCP leases to Codex and Claude Code. Calls use the same tool broker and exact once approvals; decisions signal the waiting SDK call rather than starting another process. Denial, cancellation, expiry, restart, credential changes and policy changes prevent unauthorized execution.
- Shared TypeScript contracts drive runtime selection, capabilities, durable native defaults and real extension lifecycle controls. Chat and Plan offer no agent tools; Agent requires explicit workspace/runtime selection.

## Integration sequence

1. Wire real agent/runtime defaults and runtime-specific model selection into desktop conversations; preserve native/cloud defaults separately.
2. Restore external native continuation on ordinary follow-ups. Do not silently create a fresh provider session when native resume is unavailable.
3. Deliver correlated root/child output and real tool/file/diff events. Keep tool arguments and contents in encrypted history, with bounded public projections.
4. Register skills, MCP gateways and native web tools through the existing tool broker. Add portable skill/plugin lifecycle and actual MCP initialization, discovery and invocation.
5. Add bounded inference retry, context management and safe model repair of effect-free tool mistakes. Keep uncertain effects, denied permissions and integrity failures explicit.
6. Integrate desktop extension controls, real approvals, history/reconnect and checkpoint pagination; remove sample readiness and preview actions from these paths.
7. Exercise repository editing, failing/passing test output and reviewable diffs with deterministic protocol fixtures; then verify connected accounts and packaged Windows application separately.

## Acceptance evidence

Required fixtures cover approval denial, scope and revision changes, offline admission, cancellation, malformed responses, provider/tool-call continuity, restart without repeated effects, child lifecycle, credentials absent from logs/model context/plaintext storage, and extension import integrity. Protocol fixtures prove the integration but do not establish vendor/account eligibility. Live account tasks remain pending until exercised with a configured account. Small local models without native tool-call capability remain Chat-only.

The Windows harness fixtures execute repository reads/edits, failed and passing commands, reviewable Git diffs, process output and cancellation. Real MCP stdio fixtures cover initialization, discovery, calls and relocation. The installed Codex app-server separately verifies authenticated MCP startup and encrypted native-state capture without a model turn. A public HTTPS page fetch verifies the production web transport. Brave/Tavily entitlement, paid model turns, coding tasks with user-connected accounts and the packaged app on another laptop remain **live verification pending**.

The versioned marketplace contains three actual first-party packages; it is a local catalog, not an online store. MCP support covers stdio and bounded HTTPS Streamable HTTP tools; resources, prompts, sampling, elicitation, OAuth and legacy SSE are unsupported. The SDK bridge supports one MCP client per active run; other drivers do not advertise it. Shell/filesystem work owned by vendor runtimes retains their reported Full access boundary. Native Auto falls back to Supervised because an automatic reviewer is not implemented. Projectless chats have no agent tools; projectless skill injection and arbitrary shell/runtime filesystem checkpoints are unsupported.

Generic credential mutation stops active credential consumers and emits value-free audit records. Internal encryption/settings/OAuth records require their dedicated service controls. Vault unlock reconciles stopped actors into Interrupted history and expires pending approvals without automatically resuming effects. Desktop signed updates use actual status/check/apply/rollback services, explicit local package sources and signature verification; the version switches on next launch.

Windows x64 is the release target. Unsupported runtime permissions, transport features and platform capabilities must be reported rather than silently widened. Remote access remains **Coming soon**. Existing worktrees and portable user data are preserved. Orca tools are excluded from this work.

T3 architecture is reimplemented in Rust here. If source is adapted, its MIT copyright and permission notice must accompany source/distribution; Git commits retain the repository owner's required identity.
