/** Mirrors aidex-providers and aidexd::methods::models; never contains key values. */
export type ProviderKind =
  | "openai-compatible"
  | "openai-responses"
  | "anthropic"
  | "gemini"
  | "azure-openai"
  | "bedrock"
  | "llama-cpp"
  | "fake";
export type ModelRef = { provider_id: string; model_id: string };
export type Locality = "local" | "remote";
export type ToolCallSupport = "native" | "structured" | "none";
/** Where a model's limits and capabilities came from; `assumed` models need an explicit choice. */
export type CapabilitySource = "configured" | "discovered" | "catalog" | "assumed";
export type ModelRole =
  | "code"
  | "plan"
  | "summarize"
  | "classify"
  | "embed"
  | "verify"
  | "general";

/** How a preset's account authenticates. */
export type AuthMethod = "api_key" | "oauth_chatgpt" | "aws_sigv4" | "none";
export type PresetCategory = "cloud" | "gateway" | "local" | "enterprise" | "custom";

/** A supported vendor protocol and its defaults, not a configured account. */
export type ProviderPreset = {
  id: string;
  display_name: string;
  kind: ProviderKind;
  base_url: string | null;
  discover_models: boolean;
  tool_calls: boolean;
  json_schema_output: boolean;
  /** Legacy summary of `auth_methods`: `api_key_vault`, `aws_sigv4_vault`, or `none`. */
  authentication: string;
  oauth_supported: boolean;
  /** Brand glyph key for `BrandIcon`. */
  icon: string;
  category: PresetCategory;
  description: string;
  auth_methods: AuthMethod[];
  docs_url: string | null;
  api_key_url: string | null;
  key_placeholder: string | null;
};

export type ProviderErrorKind =
  | "unavailable"
  | "unauthorized"
  | "credential_unavailable"
  | "rate_limited"
  | "context_overflow"
  | "offline_refused"
  | "policy_denied"
  | "cancelled"
  | "timeout"
  | "protocol"
  | "http"
  | "invalid_request"
  | "model_not_found";

export type ProviderFailure = {
  provider_id: string;
  kind: ProviderErrorKind;
  message: string;
};

/** The last model listing the registry recorded for a provider; reading it opens no connection. */
export type CatalogStatus = {
  /** RFC 3339 UTC timestamp. */
  fetched_at: string;
  model_count: number;
  error: ProviderFailure | null;
};

export type CostHint = { input_per_million: number; output_per_million: number };

/** One model declared for a provider in providers.toml; declarations override discovered metadata. */
export type ModelConfig = {
  id: string;
  display_name?: string | null;
  context_window: number;
  max_output_tokens?: number | null;
  tool_calls?: ToolCallSupport;
  json_schema_output?: boolean;
  chat?: boolean;
  streaming?: boolean;
  vision?: boolean;
  embeddings?: boolean;
  roles?: ModelRole[];
  cost?: CostHint | null;
  min_ram_bytes?: number | null;
};

export type ProviderAuth = { type: "none" } | { type: "vault_secret"; secret_name: string };

export type ProviderAccount = {
  descriptor: {
    id: string;
    kind: ProviderKind;
    base_url: string | null;
    locality: Locality;
    auth: ProviderAuth;
  };
  origin: "configured" | "runtime";
  display_name: string | null;
  preset_id: string | null;
  enabled: boolean;
  discover_models: boolean;
  declared_models: ModelConfig[];
  catalog: CatalogStatus | null;
};

export type ProviderModel = {
  model: ModelRef;
  display_name: string;
  locality: Locality;
  context_window: number;
  max_output_tokens: number | null;
  capabilities: {
    chat: boolean;
    streaming: boolean;
    tool_calls: ToolCallSupport;
    json_schema_output: boolean;
    vision: boolean;
    embeddings: boolean;
  };
  roles: ModelRole[];
  cost: CostHint | null;
  min_ram_bytes: number | null;
  source: CapabilitySource;
  /**
   * Image and PDF input the model accepts through its provider account, computed on every
   * `model.list` (never persisted). Absent from daemons that predate attachments.
   */
  input?: ModelInput;
};

/** Whether a model reads image and PDF attachments. */
export type ModelInput = { images: boolean; pdf: boolean };

export type NetworkState = "online" | "offline";

/** Result of `network.status` and `network.offline` (`aidexd::methods::models::NetworkStatus`). */
export type NetworkStatus = { offline: boolean; network: NetworkState };

export type ModelList = {
  models: ProviderModel[];
  failures: ProviderFailure[];
  default_model: ModelRef | null;
  network: NetworkState;
};

/** `model.list` params; omit them for the cached listing of every enabled provider. */
export type ModelListParams = {
  /** Drop cached listings first. */
  refresh?: boolean;
  /** List only this provider. */
  provider_id?: string;
};

export type ProviderAddParams = {
  id: string;
  kind: ProviderKind;
  base_url: string;
  secret_name?: string;
  locality?: Locality;
  discover_models: boolean;
  models?: ModelConfig[];
  display_name?: string | null;
  preset_id?: string | null;
  enabled?: boolean;
};

/** `provider.patch`: change only account metadata; omitted fields stay as they are. */
export type ProviderPatchParams = {
  id: string;
  /** `null` clears the display name. */
  display_name?: string | null;
  enabled?: boolean;
};

export type HealthStatus = "ready" | "auth_required" | "unavailable" | "refused";

export type ProviderHealth = {
  provider_id: string;
  status: HealthStatus;
  error?: ProviderErrorKind;
  message?: string;
  latency_ms?: number;
  /** RFC 3339 UTC timestamp. */
  checked_at: string;
};

export type DefaultModel = {
  default_model: ModelRef | null;
  model?: ProviderModel;
};
