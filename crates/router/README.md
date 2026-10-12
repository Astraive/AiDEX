# aidex-router

Policy-aware model selection (spec §36) over the models `aidex-providers` offers, and execution with explicit, privacy-respecting fallback. Depends on `aidex-core` and `aidex-providers`.

## API

- `RouteRequest { task: TaskKind, privacy: PrivacyMode, network: NetworkState, required_context: u32, needs_tools, needs_json_schema, secret_heavy, user_override: Option<ModelRef>, preferred: Option<ModelRef>, host: HostSummary { ram_bytes, gpu } }` with `RouteRequest::new(task)` and builders (`with_privacy`, `with_network`, `with_required_context`, `needing_tools`, `needing_json_schema`, `secret_heavy`, `with_override`, `with_preferred`, `with_host`).
  - `TaskKind::{Plan, Code, Edit, Summarize, Classify, Embed, Verify, Chat}` (preferred roles plan, code, code, summarize, classify, embed, verify, general).
  - `PrivacyMode::{LocalOnly, Hybrid (default), CloudPreferred, AirGapped}` (wire names as in `aidex-policy`; `FromStr` accepts `local-only` too); `NetworkState::{Online, Offline}`.
- `Router::new().with_max_fallbacks(n)` (default 3); `Router::route(&RouteRequest, &[ModelDescriptor]) -> Result<RouteDecision, RouteError>`.
- `RouteDecision { task, privacy, primary, fallbacks, allow_remote, preferred_role, reasons: Vec<String>, rejected: Vec<Rejection { model, reason: RejectReason, explanation }> }`, `candidates()`.
- `RejectReason::{RemoteForbidden { cause: AirGapped|Offline|LocalOnly|SecretHeavy }, WrongCapability { needed }, ContextTooSmall { window, required }, NoToolSupport, NoJsonSchema, InsufficientMemory { required, available }, UnverifiedCapabilities}` (serialized flat with `reason` as the tag).
- `RouteError::{NoEligibleModel { reasons, rejected }, OverrideRejected { rejection }, OverrideNotFound { model }}` with `code()` and `rejected()`.
- `run_with_fallback(&RouteDecision, &dyn ProviderLookup, &ChatRequest, on_event: &mut dyn FnMut(&StreamEvent), &CancelToken, on_fallback: &mut dyn FnMut(&FallbackEvent)) -> Result<(ChatResponse, ModelRef), ProviderError>`; `ProviderLookup` is implemented for `ProviderRegistry` and for closures `Fn(&ProviderId) -> Option<Arc<dyn ModelProvider>>`; `is_fallback_eligible(&ProviderError)`.

## Rules

Hard constraints, never relaxed:

1. Remote models are excluded when the privacy mode is `air_gapped` or `local_only`, the network is offline, or the task is secret-heavy.
2. Embedding tasks need `embeddings`; every other task needs `chat`.
3. The context window must cover `required_context`; `needs_tools` needs native or structured tool support; `needs_json_schema` needs JSON-schema output.
4. A local model whose known `min_ram_bytes` exceeds the host's known RAM is rejected.
5. Models whose limits were *assumed* (listed without metadata) are only used when the user names them explicitly. Limits from the providers crate's known-family table (`CapabilitySource::Catalog`) count as verified, like discovered ones.

Eligible models are ranked by: the `preferred` model (the configured default), the privacy mode's locality preference (local first for hybrid, remote first for cloud-preferred), role match (preferred role, then `general`, then other), native over structured tools when tools are needed, the metadata source (configured, then discovered, then catalog), then `provider/model` for determinism.

A `user_override` is used only if it passes every hard constraint; otherwise routing fails with `OverrideRejected` (it is never silently swapped), and an override disables fallback. Every decision carries human-readable `reasons` and the full list of rejected candidates.

`run_with_fallback` tries the primary, then each fallback, rewriting the request's model id. It moves on only for `is_fallback_eligible` errors (retryable ones, an offline refusal, or a vanished model); authentication, credential, policy, protocol, context-overflow, and request errors return immediately, as do cancellation and an expired deadline. Before each next attempt it emits a `FallbackEvent { from, to, reason, message }` (`model.fallback`). A candidate is skipped without being called when its provider is not registered or when it is remote while `allow_remote` is false, so even a tampered decision never falls back from local to cloud when privacy or offline mode forbid it.

## Limitations

- No cost or latency budgets yet (`CostHint` is informational); no GPU/VRAM-aware ranking (`HostSummary::gpu` is carried but unused) and RAM checks apply only when both the model and host report sizes (the daemon does not discover host RAM yet; #15 extends `aidex-host`).
- Streamed deltas of a failed attempt are not retracted; consumers discard their buffer when they receive a fallback event.
- Retries with backoff on the *same* model belong to the harness (#19); the router only moves between candidates.
