# aidex-providers

Model providers for AiDEX: provider-neutral chat contracts, the synchronous `ModelProvider` trait, a deterministic `FakeProvider`, protocol-specific OpenAI Responses, Chat Completions, Anthropic Messages, Gemini, Azure OpenAI, and AWS Bedrock ConverseStream adapters, request-time vault credentials, the offline egress gate, request-only image and PDF parts with per-account input-capability helpers, and the `ProviderRegistry`. Depends only on `aidex-core` and `aidex-drive` (plus `ureq` with rustls, `toml`, `zeroize`, `hmac`, `sha2`, `crc32fast`, `base64`).

Routing between models lives in [`aidex-router`](../router/README.md); the managed local llama.cpp runtime (issue #15) implements `ModelProvider` in `aidex-inference`.

## API

| Area           | Items                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| -------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Identifiers    | `ProviderId` (1-64 `[a-z0-9_-]`, `EntityId`-backed; names the vault consumer `provider:<id>`), `ModelId` (1-256 ASCII letters/digits and `. _ - : / @ +`, so upstream names like `meta-llama/Llama-3.1-8B` work), `ModelRef { provider_id, model_id }` written `provider/model` (split at the first `/`). `aidex_core::ModelReference` is not reused because its `EntityId` model half rejects `/`.                                                                                                                                                                                                                                                                                                                             |
| Models         | `ModelDescriptor { model, display_name, locality: Local\|Remote, context_window, max_output_tokens, capabilities: ModelCapabilities { chat, streaming, tool_calls: Native\|Structured\|None, json_schema_output, vision, embeddings }, roles: [Code\|Plan\|Summarize\|Classify\|Embed\|Verify\|General], cost: Option<CostHint>, min_ram_bytes, source: Configured\|Discovered\|Catalog\|Assumed }`                                                                                                                                                                                                                                                                                                                                      |
| Chat           | `ChatRequest { model: ModelId, messages: [ChatMessage { role: System\|User\|Assistant\|Tool, content, tool_calls: [ToolCall { id, name, arguments, provider_metadata? }], tool_call_id, parts: [ContentPart] (request-only, never serialized) }], tools: [ToolSpec { name, description, parameters }], tool_choice: Auto\|None\|Required\|Tool{name}, response_format: Option<ResponseFormat::JsonSchema { name, schema, strict }>, temperature, max_tokens, stop }` with `validate()`, `estimated_input_tokens()`, and `estimated_input_tokens_with(pdf_page_tokens)`; `StreamEvent::{TextDelta{text}, ToolCallDelta{index, id, name, arguments}, Usage(Usage), Done{finish_reason}}`; `ChatResponse { message, finish_reason: Stop\|Length\|ToolCalls\|ContentFilter\|Cancelled, usage: Usage { prompt_tokens, completion_tokens } }` |
| Provider trait | `ModelProvider: Send + Sync { descriptor() -> &ProviderDescriptor; models() -> Result<Vec<ModelDescriptor>, ProviderError>; chat(&ChatRequest, &mut dyn FnMut(&StreamEvent), &CancelToken) -> Result<ChatResponse, ProviderError>; health() -> ProviderHealth; invalidate_models() (default: no-op) }`; `ProviderDescriptor { id, kind: OpenaiCompatible\|OpenaiResponses\|Anthropic\|Gemini\|AzureOpenai\|Bedrock\|LlamaCpp\|Fake, base_url, locality, auth: AuthConfig::None\|VaultSecret{secret_name} }`; `ProviderHealth { status: Ready\|AuthRequired\|Unavailable\|Refused, error, message, latency_ms, checked_at }`                                                                                                                                           |
| Parts          | `ContentPart::{Image { mime, data: Arc<[u8]> }, Pdf { name, data, pages }}` (`image`, `pdf`, `data`, `mime`, `encoded_len`, `dimensions`, `estimated_tokens`, `estimated_tokens_with(pdf_page_tokens)`; `Debug` shows type and size only); `ChatMessage::with_parts`; `MAX_PARTS_PER_MESSAGE` (20), `IMAGE_MIMES`, `PDF_MIME`, `MAX_IMAGE_TOKENS`, `PDF_PAGE_TOKENS`; `image_mime(bytes)`, `image_dimensions(bytes)`, `is_animated_gif(bytes)` |
| Input          | `accepts_images`, `accepts_pdf`, `accepts_image_mime(.., mime)`, `accepts_part(.., &ContentPart)`, `model_input -> ModelInput { images, pdf }`, `part_limits -> PartLimits { max_request_encoded_bytes, max_image_bytes, max_pdf_bytes, max_image_edge_px, max_images_per_message, max_pdfs_per_message, max_pdf_pages_per_request, max_images_per_request, many_images: Option<ManyImages { above, max_edge_px, counts_pdfs }>, pdf_page_tokens }`, each taking `(&ProviderDescriptor, preset_id: Option<&str>, &ModelDescriptor)`; `ProviderRegistry::{model_input, accepts_part, part_limits}(&ModelDescriptor)` look the account up |
| Cancellation   | `CancelToken` (re-exported from `aidex-core`): shared `Arc<AtomicBool>` plus optional deadline; `cancel()`, `with_timeout()`, `state() -> Option<Cancelled\|DeadlineExceeded>`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| Errors         | `ProviderError::{Unavailable{reason}, Unauthorized, Credential(CredentialError), RateLimited{retry_after}, ContextOverflow{limit, requested}, OfflineRefused{host}, PolicyDenied{reason}, Cancelled, Timeout, Protocol(String), Http{status, code}, InvalidRequest(String), ModelNotFound{model}}` with `kind()` (stable snake_case `ProviderErrorKind`), `code()` (`ServiceErrorCode`), and `is_retryable()` (unavailable, rate limited, timeout, HTTP 408/425/429/5xx only)                                                                                                                                                                                                                                                   |
| Credentials    | `CredentialResolver::resolve(&ProviderId, secret_name) -> Result<SecretBytes, CredentialError>`; `SecretBytes` (zeroizing, `Debug` = `[REDACTED]`, not `Clone`/`Serialize`/`Display`); `NoVault` (always unavailable); `StaticCredentials` (in-memory, per provider; tests and embedders holding a per-launch key)                                                                                                                                                                                                                                                                                                                                                                                                              |
| Egress         | `EgressGate::check_remote(&ProviderId, host)`; `OfflineMode` (`new`, `load(&DrivePaths)` persisted in `private_config/network.toml`, `is_offline`, `set_offline`)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| Providers      | `FakeProvider` (+ `FakeScript`, `FakeTurn::{text, tool_call, tool_calls, error}.with_delay().chunked()`, `requests()`, `set_models_error`, `set_health_error`, `with_kind(ProviderKind)` to follow another protocol family's input rules); `OpenAiCompatibleProvider::{from_config, with_options}`; the bare `OpenAiClient { list_models, chat }` over a `BaseUrl` with `HttpOptions`; `DiscoveredModel` (a listing entry and its vendor metadata)                                                                                                                                                                                                                                                                                                                                                                                                                      |
| Registry       | `ProviderRegistry::{load(&DrivePaths, resolver, gate), load_with(.., HttpOptions), in_memory, with_observer, add_provider, add_provider_with(config, AccountMetadata), update_provider, update_provider_with(config, Option<AccountMetadata>), patch_provider(&id, ProviderPatch { display_name, enabled }), remove_provider, register(Box<dyn ModelProvider>), register_shared(Arc<..>), unregister, providers() -> [ProviderEntry { descriptor, origin, display_name, preset_id, enabled, discover_models, declared_models, catalog: Option<CatalogStatus { fetched_at, model_count, error }> }], provider_entry, provider, provider_config, models() -> ModelListing { models, failures }, models_for(&id), refresh_models(Option<&id>), default_model, set_default_model, clear_default_model, health, model_input, accepts_part, part_limits}`                                                                                                                                                                                                                                                                                                                                       |
| Events         | `ModelObserver::observe(&ModelEvent)`; `ModelEvent::{Selected, Fallback(FallbackEvent { from, to, reason, message }), DefaultChanged, ProviderAdded, ProviderRemoved, NetworkModeChanged}` (dotted names `model.selected`, `model.fallback`, ...); `NoopObserver`, `RecordingObserver`                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| Presets        | `provider_presets()`, `preset(id)`, `preset_link(id, PresetLink::{ApiKey, Docs}) -> Result<String, PresetLinkError>`; `ProviderPreset { id, display_name, kind, base_url, discover_models, tool_calls, json_schema_output, authentication, oauth_supported, icon, category: Cloud\|Gateway\|Local\|Enterprise\|Custom, description, auth_methods: [ApiKey\|OauthChatgpt\|AwsSigv4\|None], docs_url, api_key_url, key_placeholder }` |
| Configuration  | `ProvidersConfig { version: 1, default_model, provider: [ProviderConfig { id, kind, base_url, locality?, auth, discover_models, model: [ModelConfig] }] }` in `private_config/providers.toml`; `ProviderAccountsConfig { version: 1, account: [ProviderAccountMeta { id, display_name?, preset_id?, enabled }] }` in `private_config/provider-accounts.toml`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |

## `providers.toml`

```toml
version = 1
default_model = "openai/gpt-4o-mini"

[[provider]]
id = "openai"
kind = "openai-compatible"
base_url = "https://api.openai.com/v1"
discover_models = true

[provider.auth]
type = "vault_secret"
secret_name = "openai-key"   # stored with `aidex vault set openai-key --consumer provider:openai`

[[provider.model]]
id = "gpt-4o-mini"
context_window = 128000
max_output_tokens = 16384
tool_calls = "native"
json_schema_output = true
roles = ["code", "general"]
```

The schema is closed (`deny_unknown_fields`, version checked): a pasted `api_key = ...` makes the file invalid rather than being used, and parse errors report a line number with quoted values removed, so a misplaced key is never echoed. Writes are atomic (temp file, fsync, rename) and refuse symlinked paths.

## Account metadata (`provider-accounts.toml`)

`providers.toml` stays byte-compatible with older daemons: `update.rollback` can restart an older build on the same drive, and its closed schema would reject new keys. Presentation metadata therefore lives in a closed-schema sidecar that older builds ignore:

```toml
version = 1

[[account]]
id = "anthropic"
display_name = "Work Claude"   # optional, 1-64 characters, no control characters
preset_id = "anthropic"        # optional, [a-z0-9-]{1,64}; unknown ids are kept
enabled = true                 # default true
```

`ProviderRegistry::load_with` reads it next to `providers.toml` (a missing file means no metadata; an unknown version or invalid entry is an error; entries for ids that are not configured are ignored and dropped on the next write). `add_provider_with(config, AccountMetadata { display_name, preset_id, enabled })` and `update_provider_with(config, Some(metadata))` write it (display names are trimmed); `add_provider`/`update_provider` keep the defaults or the existing metadata; `patch_provider(&id, ProviderPatch { display_name: Option<Option<String>>, enabled: Option<bool> })` changes one account (configured accounts only, `NotConfigured` for runtime providers); `remove_provider` drops the entry. Entries that carry only defaults are not written. The metadata file is written before `providers.toml` and restored if that write fails.

A **disabled** account stays configured and is reported by `providers()` (`enabled: false`), but `models()` skips it without a failure, `provider(id)` returns `None` (callers report the provider as unavailable), `models_for`/`set_default_model` return `NotFound`, `refresh_models(Some(id))` reports a failure, and `health` reports `Refused` with the message "provider is disabled" without any connection. Disabling the provider of the default model clears the default (`model.default_changed` with no model).

`ProviderEntry` (the `model.providers` row) carries `descriptor`, `origin`, `display_name`, `preset_id`, `enabled`, `discover_models`, `declared_models` (the account's `ModelConfig`s; empty for runtime providers), and `catalog: Option<CatalogStatus { fetched_at, model_count, error: Option<ProviderFailure> }>`, the outcome of the last listing of that provider in this process. Reading it opens no connection.

## Model discovery

Listings become `DiscoveredModel { id, context_window, display_name, max_output_tokens, tool_calls, json_schema_output, vision, embeddings, chat, cost }`; a field the vendor did not report stays `None`, unknown fields are ignored, and a malformed optional field never fails the listing.

| Source | Request | Read |
| --- | --- | --- |
| OpenAI-compatible (`openai-compatible`, OpenAI Responses API keys) | `GET models` | `data[]` or a top-level array; display name from `name`/`display_name`; context from `context_length`, `max_model_len`, `context_window`, `max_context_length`, `meta.n_ctx`, `meta.n_ctx_train`, or `top_provider.context_length`; output limit from `top_provider.max_completion_tokens`, `max_completion_tokens`, or `max_output_tokens`; tools from `supported_parameters` (`tools`) or `capabilities.function_calling`; JSON-schema output from `supported_parameters` (`structured_outputs`, `response_format`); vision from `architecture.input_modalities` (`image`) or `capabilities.vision`; embeddings from `type: embedding(s)` or an id containing `embed` (not chat); `type: image\|audio\|moderation\|rerank` and `capabilities.completion_chat: false` are not chat; OpenRouter `pricing.prompt`/`pricing.completion` (USD per token) become `CostHint` per million tokens (negative or non-finite prices are ignored); entries with `active: false` (Groq) are skipped |
| Anthropic | `GET models?limit=1000`, then `&after_id=<last_id>` while `has_more` | `data[].id`, `display_name` |
| Gemini | `GET models?pageSize=1000`, then `&pageToken=<nextPageToken>` | `models[]` whose `supportedGenerationMethods` contains `generateContent` (chat) or only `embedContent` (embeddings, not chat); `displayName`, `inputTokenLimit`, `outputTokenLimit` |
| ChatGPT plan catalog | `GET models` | `models[]` with `visibility: "list"`: `slug`, `display_name` |
| Azure OpenAI, Bedrock | none | declared models only |

Pagination follows at most 10 pages within one list timeout; cursors are percent-encoded. Declared models always win over a discovered entry with the same id.

**Known-family hints** (`src/hints.rs`) then fill only the fields a listing left unreported, for well-documented families, matching the lowercase id with any vendor/route prefix (`anthropic/`, `openai/`, `x-ai/`, `google/`, `models/`, ...) and `:variant`/`@version` suffix removed so gateways benefit too. Values are conservative minimums:

| Family | Hint |
| --- | --- |
| `claude-*` (Claude 3 and later) | 200,000 context, native tools, vision |
| `gpt-4o*`, `gpt-4.1*`, `gpt-5*` to `gpt-9*` (mini/nano included) | 128,000 context, native tools, JSON schema, vision |
| `o1` (not `o1-mini`/`o1-preview`), `o3*`, `o4*`, later o-series | 128,000 context, native tools, JSON schema; vision except `o3-mini*` and `-mini` models after o4 (`o1`, `o1-pro`, `o3`, `o3-pro`, `o4-mini` read images) |
| `gemini-*` | native tools, JSON schema, vision (context comes from the API) |
| `grok-2` and later | 131,072 context, native tools, JSON schema; vision for `grok-4` and later and any `grok-*vision*` |
| `deepseek-chat` / `deepseek-reasoner` | 64,000 context (native tools for `deepseek-chat`) |
| `mistral-large*`, `mistral-medium*`, `mistral-small*`, `codestral*`, `magistral*`, `devstral*`, `pixtral*` | 32,000 context, native tools (vision for `pixtral*`) |
| `llama-3.2-*vision*` / `llama3.2-vision`, `llama-4*` / `llama4*` | vision only |
| `kimi-k2*` / other `kimi-*`, `moonshot-*` | 128,000 context and native tools / native tools |
| `text-embedding*` | embeddings, not chat |
| `dall-e*`, `whisper*`, `tts*`, `omni-moderation*`, `text-moderation*`, `babbage*`, `davinci*`, `sora*`, `gpt-image*`, and any `*-realtime*`, `*-audio*`, `*-transcribe*`, `*-tts*`, `*-search*` | not chat |

A descriptor whose context window was reported is `Discovered`; one whose context window came only from this table is `Catalog` (router-eligible like discovered metadata, ranked after it); otherwise it is `Assumed` with a 4,096-token window. Only `Configured` and `Discovered` windows are ceilings: the harness rejects or clamps a request against them, but never against a `Catalog` minimum or an `Assumed` guess.

## Caching and refresh

Both HTTP adapters keep the last successful listing for `HttpOptions::model_cache_ttl` (60 s). The egress gate is consulted first even when a listing is cached, so an offline listing still reports `OfflineRefused`. `ModelProvider::invalidate_models()` drops the cache (a no-op for providers without one); `health()` always probes live.

`ProviderRegistry::models()` lists every enabled provider concurrently and records each outcome as that provider's `CatalogStatus`; `models_for(&id)` lists one enabled provider (and records it); `refresh_models(None)` invalidates every enabled provider before listing, and `refresh_models(Some(&id))` invalidates and lists only that provider. `set_default_model` checks the model through `models_for`. Updating, removing, or re-registering a provider clears its recorded status.

## Presets

`provider_presets()` returns the catalog in presentation order and `preset(id)` one entry. Each preset names its protocol `kind`, endpoint (`base_url`; HTTPS, or `http://127.0.0.1` for local servers; `None` where the user supplies their own resource), whether it `discover_models`, `json_schema_output`, a brand `icon` key, a `category` (`cloud`, `gateway`, `local`, `enterprise`, `custom`), a one-sentence `description`, its `auth_methods` (`api_key`, `oauth_chatgpt`, `aws_sigv4`, `none`; preferred first), and optional HTTPS `docs_url`, `api_key_url`, and `key_placeholder` (a key shape such as `sk-ant-...`, never a key). `authentication` keeps its legacy values (`api_key_vault`, `aws_sigv4_vault`, and `none` for key-less local presets) and `oauth_supported` is true only for `openai`. `preset_link(id, PresetLink::ApiKey | PresetLink::Docs)` resolves only URLs stored in this table, refusing anything that is not `https://`, so a caller never supplies the URL it opens.

| Category | Presets |
| --- | --- |
| cloud | `openai` (Responses; ChatGPT sign-in or API key), `anthropic`, `gemini`, `xai`, `mistral`, `deepseek`, `groq`, `moonshot`, `cerebras`, `qwen`, `openai-chat` (Chat Completions) |
| gateway | `openrouter`, `together`, `fireworks`, `deepinfra`, `huggingface`, `nvidia`, `vercel` |
| enterprise | `azure-openai`, `bedrock` (declared models only) |
| local | `ollama` (`127.0.0.1:11434`), `lmstudio` (`:1234`), `llamacpp` (`:8080`), `vllm` (`:8000`, optional key) |
| custom | `custom` (any OpenAI-compatible endpoint, key optional) |

## Guarantees

- **Credentials** are resolved per request, used only to build protocol-specific authentication headers or AWS signatures on a worker thread, and zeroized when dropped. They never appear in configuration, `Debug` output, errors (HTTP failures carry the status and a sanitized machine code, never server text, which can echo the key), observer events, request bodies, or model context. Tests generate the credential at run time and scan config files, `Debug`, errors, events, listings, and the bytes sent to a fake server.
- **Offline / egress**: remote providers consult the `EgressGate` before resolving a credential and before any socket is opened; offline mode returns `OfflineRefused` with zero connections (tested against a listener that counts connections). Even cached listings are gated, so an offline listing reports the refusal.
- **Locality is derived from the endpoint**: only loopback hosts (`localhost`, `127.0.0.0/8`, `::1`) can be local; LAN and cloud endpoints are remote and cannot be declared local. A loopback endpoint can be forced `remote` (for example a tunnel). Credentials are sent only over `https` or to loopback; URLs with embedded user info, queries, or fragments are rejected.
- **HTTP**: redirects are never followed (a redirect could carry the `Authorization` header elsewhere); environment proxies are ignored for loopback endpoints. Bodies, SSE lines and aggregate data events (1 MiB each), streamed text (8 MiB), tool calls (64), and arguments (1 MiB each) are bounded.
- **Failures are explicit**: unreachable servers are `Unavailable`, a stream that ends without `[DONE]`/finish reason is a protocol error (never a truncated "success"), discovery failures are errors (the registry reports them per provider next to working providers, and records them as the provider's `CatalogStatus`), and a model without reported limits is marked `Assumed` (the router will not auto-select it) unless the known-family table supplies conservative ones (`Catalog`).
- **Cancellation**: `chat` runs the exchange on a worker thread and returns `Cancelled` (after `Done(Cancelled)`) or `Timeout` within one 20 ms poll of the token firing, even while the server is silent. Tokens already stopped win before credential resolution and cause no connection; transport admission rechecks the token before starting the worker and before its send. The worker closes the connection at its next read, which stops generation on servers such as llama-server.
- `FakeProvider` is deterministic, performs no I/O, records requests, enforces its models' context windows and input capabilities, and fails an exhausted script with a non-retryable protocol error so fallback cannot hide a test bug.

## OpenAI-compatible wire behaviour

`base_url` is the API root including its version segment; an empty path becomes `/v1`. Chat always requests streaming (`stream: true`, `stream_options.include_usage`) and also accepts a plain JSON completion. Tools map to `tools`/`tool_choice` (`function` type), JSON-schema output to `response_format: {type: "json_schema", json_schema: {name, schema, strict}}`, which llama-server enforces with grammar-constrained decoding. Discovery reads `/models` (`{"data": [...]}` or a top-level array, as Together returns) and the vendor metadata described under [Model discovery](#model-discovery); llama.cpp's `meta.n_ctx_train` can exceed the server's configured context, and such overflows surface as `ContextOverflow`. Error bodies are parsed only for `code`/`type`, `n_ctx`, and `n_prompt_tokens` (llama-server's `exceed_context_size_error`, OpenAI's `context_length_exceeded`).

## Image and PDF parts

`ChatMessage::parts` carries `ContentPart::Image { mime, data }` and `ContentPart::Pdf { name, data, pages }` for one request. The field is `#[serde(skip)]`: it is never serialized or deserialized, so sealed transcripts, persisted runs, and older daemons see exactly the text-only record (and a record that names `parts` is rejected like any unknown field). The harness resolves attachment bytes per request; `Debug` prints only the type and size, never bytes or names.

`ChatRequest::validate` allows parts only on `user` messages, at most 20 per message; an image's `mime` must be PNG, JPEG, GIF, or WebP and match the bytes' signature; a PDF must start with `%PDF-`, report at least one page, and carry a 1-255 byte name without control characters. `estimated_input_tokens` counts each image as `ceil(w/28) * ceil(h/28)` tokens from its header dimensions (at most 4,800, which is also used when the size cannot be read) and each PDF page as 3,000 tokens (`PDF_PAGE_TOKENS`), never by base64 length. `estimated_input_tokens_with` takes the page cost of the model's family instead (`PartLimits::pdf_page_tokens`: 560 on Gemini, which bills 258 per page on Gemini 2.x and 560 at Gemini 3's default media resolution; 3,000 elsewhere), so a long PDF is not refused locally against an exact context window the vendor would accept it in.

Messages without parts keep their exact wire form. A user message with parts maps as:

| Protocol | Mapping |
| --- | --- |
| Chat Completions (OpenAI-compatible presets, Azure, llama.cpp) | `content: [{type: "text"}?, {type: "image_url", image_url: {url: "data:<mime>;base64,..."}}..., {type: "file", file: {filename: "<name>.pdf", file_data: "data:application/pdf;base64,..."}}...]` in part order; the text part is omitted when empty. Requests to `openrouter.ai` that carry a PDF add `plugins: [{id: "file-parser", pdf: {engine: "native"}}]`, so OpenRouter never hands the document to a third-party parser. |
| OpenAI Responses | `content: [{type: "input_text"}?, {type: "input_image", image_url: "data:...", detail: "auto"}..., {type: "input_file", filename, file_data: "data:application/pdf;base64,..."}...]`; a user message with parts is sent even when its text is empty. |
| Anthropic | `{type: "image", source: {type: "base64", media_type, data}}` and `{type: "document", source: {type: "base64", media_type: "application/pdf", data}}` before the text block (no file name is sent). |
| Gemini | `{inlineData: {mimeType, data}}` parts before the text part. |
| Bedrock Converse | `{image: {format: png\|jpeg\|gif\|webp, source: {bytes}}}` and `{document: {format: "pdf", name, source: {bytes}}}` before the text; a message with a document and no text gets the text block `Attached document.`. Document names keep ASCII letters, digits, single spaces, `-`, `()`, and `[]` (every other character, `_` and `.` included, becomes `-` or a space; runs collapse; a trailing `.pdf` is dropped), are 1-200 characters (`document` when nothing remains), and are unique within the request (` (2)`, ` (3)`, ...). |

Anthropic, Gemini, and Bedrock never receive an empty content array, and their roles alternate: a message that maps to no blocks (a failed run's empty assistant placeholder, a blank message without parts) is skipped, and consecutive same-role messages form one turn with tool results first, then the other blocks in message order. So `[user, assistant(""), user]` is one user turn, `[assistant(tool calls), tool, tool, user]` ends with one user turn holding both results and the text, and the workspace file index joins the transcript's first user message. Chat Completions keeps sending an empty assistant message as `""`, and Responses skips it, as before.

### Input capabilities

`accepts_images`, `accepts_pdf`, `accepts_image_mime`, `accepts_part`, `model_input`, and `part_limits` take the provider descriptor, the account's preset id, and the model, and are pure: nothing is added to `providers.toml` or `ModelCapabilities`, which older daemons must keep reading. `ProviderRegistry::{model_input, accepts_part, part_limits}` look the descriptor and preset up from the model's provider id (enabled or not; an unknown provider accepts nothing).

- **Images**: a chat model with `vision`. The managed llama.cpp runtime (`llama-cpp`) starts without a vision projector and never accepts images.
- **PDFs**: images, plus a protocol that carries documents: Anthropic, Gemini, Bedrock, OpenAI Responses with an API key (not a ChatGPT plan account, an `oauth.` vault secret, until plan tokens are confirmed for `input_file`), and OpenAI-compatible accounts from the `openai-chat` preset, or from `openrouter` for `anthropic/*`, `google/gemini*`, and `openai/*` models (OpenRouter's moving `~` aliases, such as `~anthropic/claude-sonnet-latest`, are not valid model ids, so discovery skips them). The decision keys on the preset id, not the endpoint, so a loopback test server under `openai-chat` receives `file` parts. Azure, other compatible presets, custom endpoints, and llama.cpp do not. `FakeProvider` follows its models' `vision` flag for both.
- **Image types**: Gemini (and OpenRouter `google/gemini*`) takes no GIF; xAI (the `xai` preset, OpenRouter `x-ai/*`, and Vercel `xai/*` and `spacexai/*`) takes only PNG and JPEG; OpenAI families take GIF only when it has one frame (`accepts_part` reads the bytes; an unreadable GIF counts as animated).
- **Per part** (`accepts_part`): also within the family's raw size and edge limits below.

`part_limits` gives the vendor limits the harness budgets against. Request budgets are measured on base64 (`ContentPart::encoded_len`):

| Family | Request (encoded) | Image | PDF | Edge | Per message | Images per request | PDF pages per request | Many images | Tokens per PDF page |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Anthropic (and OpenRouter `anthropic/*`) | 30 MB | 7.5 MB (10 MB base64) | 22.5 MB | 8,000 px | none | 100 (600 for context windows of 1M tokens or more) | 100 (600 at 1M) | over 20 images: each at most 2,000 px | 3,000 |
| Bedrock | 20 MB | 3.75 MB | 4.5 MB | 8,000 px | 20 images, 5 documents | 100 (600 at 1M) | 100 (600 at 1M) | over 20 images and documents: each image at most 2,000 px | 3,000 |
| Gemini (and OpenRouter `google/gemini*`) | 18 MB (of the 20 MB request) | 13.5 MB | 13.5 MB | none | none | none | 1,000 | none | 560 |
| OpenAI Responses, Chat Completions, Azure, llama.cpp, Fake | 32 MB | 20 MB | 24 MB | none | none | none | 100 | none | 3,000 |

Images per request count history too, and PDFs do not count toward them.

Before a request with parts leaves, every HTTP adapter re-checks the limits its protocol shares (raw sizes, per-message counts, images and PDF pages per request, the encoded request total, Gemini GIF, OpenAI animated GIF, and, for images whose header dimensions can be read, the edge limit and the many-image rule; Bedrock also counts per coalesced turn) and refuses with `InvalidRequest` naming the rule, never a file, without opening a connection. The adapter knows only the protocol, not the model's context window or OpenRouter route, so it checks the loosest limits `part_limits` gives any model of that protocol (the 1M-window allowances; on OpenAI-compatible accounts also the Anthropic and Gemini families OpenRouter routes to): it is a backstop that never refuses a request the harness built within the model's own limits. `FakeProvider` records every request (parts included) and then refuses, as `InvalidRequest`, a part its model cannot read under `accepts_part`, without consuming a scripted turn; it estimates PDF pages at its kind's `pdf_page_tokens` when checking the context window.

## Limitations / not implemented

- Only the `openai` preset advertises sign-in (`oauth_supported: true`, `auth_methods: [oauth_chatgpt, api_key]`). ChatGPT OAuth acquisition belongs to `aidex-auth`, which registers Responses instances and resolves their vault credentials. Generic device login is not implemented. Missing/rejected credentials report `auth_required` health.
- Azure and Bedrock require configured deployment/model IDs (`discover_models: false`). Bedrock has no free runtime liveness endpoint; `health()` explicitly reports an unavailable check, without making a billable inference request.
- `max_tokens` is sent as `max_tokens`; OpenAI reasoning models that require `max_completion_tokens` are not special-cased.
- `reasoning_content` deltas and multiple choices (`n > 1`) are ignored; embeddings requests are not part of the chat API yet. Image and PDF parts are inline base64 only (no vendor file-upload APIs), on user messages only; tool results stay text.
- The header value is copied into `ureq`'s request buffers, which are not zeroized.
- Other health checks use `GET /models`; Azure adds its API version. An endpoint that does not offer a listing returns the real error.
- `OfflineMode` is the only egress gate; the broker's policy-backed gate (DNS re-checks, allowlists) replaces it in #17.

## Cloud protocols and presets

`ProviderRegistry::add_provider` and `update_provider` dispatch on `ProviderKind`;
`OpenAiCompatibleProvider` accepts only `openai-compatible`. Accounts use an
instance `ProviderId`, independently of the stable preset ID returned by
`provider_presets()`. Presets describe adapter support; configured/discovered
model metadata remains authoritative for routing.

| Kind                | Base URL                                           | Wire/authentication                                                             |
| ------------------- | -------------------------------------------------- | ------------------------------------------------------------------------------- |
| `openai-compatible` | versioned API root                                 | `/chat/completions`, Bearer vault key                                           |
| `openai-responses`  | `https://api.openai.com/v1`                        | `/responses`, Bearer vault key                                                  |
| `anthropic`         | `https://api.anthropic.com/v1`                     | `/messages`, `x-api-key`, `anthropic-version: 2023-06-01`                       |
| `gemini`            | `https://generativelanguage.googleapis.com/v1beta` | `/models/<id>:streamGenerateContent?alt=sse`, `x-goog-api-key`                  |
| `azure-openai`      | `https://<resource>.openai.azure.com/openai`       | `/deployments/<id>/chat/completions?api-version=2024-10-21`, `api-key`          |
| `bedrock`           | `https://bedrock-runtime.<region>.amazonaws.com`   | `/model/<id>/converse-stream`, AWS SigV4, CRC-validated binary AWS event stream |

Bedrock's vault secret contains a JSON object with `access_key_id`,
`secret_access_key`, `region`, and optional `session_token`; all credentials
are released only for `provider:<instance-id>` and zeroized after signing.
AWS credential discovery from the environment is deliberately absent.

The presets also cover xAI, Mistral, DeepSeek, Groq, Moonshot, Cerebras, Qwen,
OpenRouter, Together, Fireworks, DeepInfra, Hugging Face, NVIDIA NIM, the Vercel
AI Gateway, local Ollama/LM Studio/llama.cpp/vLLM servers, and custom
compatible endpoints (see [Presets](#presets)). No model ID list is hard-coded;
add custom IDs through `ModelConfig` or use discovery. Configured
limits/capabilities override discoveries; unknown limits remain `Assumed`
unless a known model family supplies conservative ones (`Catalog`).

Requests map text, assistant function calls, tool results, tool schemas/choices,
token limits, temperature and supported structured output into each protocol.
Responses and Anthropic assemble split function argument events; Gemini maps
complete `functionCall` objects; Bedrock assembles tool-use input fragments.
Noncompatible endpoints must actually return their streaming protocol. Malformed,
CRC-corrupt, incomplete and invalid tool JSON responses fail explicitly.

OpenAI Responses also accepts the ChatGPT plan-token model catalog's
`models`/`slug` envelope, displaying only entries with `visibility: "list"`
and retaining their supplied labels and ordering at the provider boundary
(the known-family hints then apply as for any other listing).
API-key `data`/`id` catalogs remain supported. Responses inference uses
`store: false` and `stream: true`. Recognized ChatGPT plan eligibility and
usage failures retain sanitized status/code information and stop automatic
provider or billing fallback; callers may explicitly retry temporary
availability later.

Anthropic/Bedrock JSON-schema output is explicitly refused by this adapter.
Gemini function-call signatures are retained in optional `ToolCall.provider_metadata` and the original signed part is replayed during tool continuations. Optional non-function reasoning signatures remain outside the current neutral chat contract; image and PDF input is described under [Image and PDF parts](#image-and-pdf-parts). OpenAI Responses refuses stop sequences.
Azure API version is currently fixed to the documented 2024-10-21 deployment API.
All fixtures use loopback servers; production vendor calls require user credentials
and have not been exercised by these tests.

Official protocol references: [OpenAI streaming events](https://developers.openai.com/api/reference/resources/responses/streaming-events),
[Anthropic streaming](https://platform.claude.com/docs/en/build-with-claude/streaming),
[Gemini functions](https://ai.google.dev/gemini-api/docs/function-calling),
[Azure REST authentication](https://learn.microsoft.com/en-us/azure/foundry/openai/reference),
[Bedrock ConverseStream](https://docs.aws.amazon.com/bedrock/latest/APIReference/API_runtime_ConverseStream.html),
[AWS signing](https://docs.aws.amazon.com/IAM/latest/UserGuide/reference_sigv-create-signed-request.html).
