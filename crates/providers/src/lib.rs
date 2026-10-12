//! Model providers for AiDEX: provider-neutral chat contracts (with
//! request-only image and PDF [`ContentPart`]s), the synchronous
//! [`ModelProvider`] trait, a deterministic [`FakeProvider`], an
//! OpenAI-compatible HTTP provider, request-time vault credentials, the
//! offline egress gate, input-capability helpers, and the
//! [`ProviderRegistry`].
//!
//! Provider failures (unavailable, unauthorized, rate limited, context
//! overflow, offline refusal, ...) are explicit [`ProviderError`] outcomes.
//! Credentials are resolved per request through a [`CredentialResolver`] and
//! never stored in configuration, events, errors, `Debug` output, or model
//! context.

mod chat;
mod cloud;
mod config;
mod credentials;
mod egress;
mod error;
mod fake;
mod hints;
mod ids;
mod input;
mod model;
mod observer;
mod openai;
mod parts;
mod presets;
mod provider;
mod registry;

pub use aidex_core::{CancelReason, CancelToken};
pub use chat::{
    ChatMessage, ChatRequest, ChatResponse, FinishReason, MAX_MESSAGES, MAX_STOP_SEQUENCES,
    MAX_TOOLS, ResponseFormat, Role, StreamEvent, ToolCall, ToolChoice, ToolSpec, Usage,
    is_tool_name,
};
pub use config::{
    AccountMetadata, AuthConfig, CONFIG_VERSION, ConfigError, MAX_ACCOUNT_NAME_CHARS, ModelConfig,
    NETWORK_FILE, NetworkConfig, PROVIDER_ACCOUNTS_FILE, PROVIDERS_FILE, ProviderAccountMeta,
    ProviderAccountsConfig, ProviderConfig, ProviderKind, ProvidersConfig, config_file,
    is_account_name, is_preset_id,
};
pub use credentials::{
    CredentialResolver, NoVault, SecretBytes, StaticCredentials, is_secret_name,
};
pub use egress::{EgressGate, OfflineMode};
pub use error::{CredentialError, ProviderError, ProviderErrorKind};
pub use fake::{FakeOutcome, FakeProvider, FakeScript, FakeTurn};
pub use ids::{InvalidId, ModelId, ModelRef, ProviderId};
pub use input::{
    ManyImages, ModelInput, PartLimits, accepts_image_mime, accepts_images, accepts_part,
    accepts_pdf, model_input, part_limits,
};
pub use model::{
    ASSUMED_CONTEXT_WINDOW, CapabilitySource, CostHint, Locality, ModelCapabilities,
    ModelDescriptor, ModelRole, ToolCallSupport,
};
pub use observer::{FallbackEvent, ModelEvent, ModelObserver, NoopObserver, RecordingObserver};
pub use openai::{
    BaseUrl, DiscoveredModel, HttpOptions, InvalidProvider, InvalidUrl, OpenAiClient,
    OpenAiCompatibleProvider, Scheme,
};
pub use parts::{
    ContentPart, IMAGE_MIMES, MAX_IMAGE_TOKENS, MAX_PARTS_PER_MESSAGE, PDF_MIME, PDF_PAGE_TOKENS,
    image_dimensions, image_mime, is_animated_gif,
};
pub use presets::{
    AuthMethod, PresetCategory, PresetLink, PresetLinkError, ProviderPreset, preset, preset_link,
    provider_presets,
};
pub use provider::{HealthStatus, ModelProvider, ProviderDescriptor, ProviderHealth};
pub use registry::{
    CatalogStatus, ModelListing, ProviderEntry, ProviderFailure, ProviderOrigin, ProviderPatch,
    ProviderRegistry, RegistryError,
};
