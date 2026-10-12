// Mirrors the `web.*` methods in crates/aidexd/src/methods/web.rs and the `aidex_web`
// wire types (crates/web/src/settings.rs, catalog.rs, client.rs), plus the results of the
// native `web_search` and `web_fetch` tools (crates/harness/src/native_tools/exec.rs).
// Provider keys are stored only with `vault.secret.set` for consumer `process:web`;
// these methods never accept or return a key value.

export type SearchProvider = "brave" | "tavily" | "exa" | "searxng" | "duckduckgo";

/** `private_config/web.toml` (closed schema version 1). */
export type WebSettings = {
  version: number;
  /** Master switch: web tools are offered to agents only when on. */
  enabled: boolean;
  provider: SearchProvider;
  /** SearXNG base URL. */
  endpoint: string | null;
  /** Vault secret holding the provider key (consumer `process:web`). */
  secret_name: string | null;
  /** Results per search, 1-10. */
  max_results: number;
  /** Whether `web_fetch` is offered alongside search. */
  fetch_enabled: boolean;
};

/** Result of `web.settings.get` and `web.settings.set`. */
export type WebSettingsView = WebSettings & {
  /** The settings name everything a search needs (the key itself is checked when a search runs). */
  configured: boolean;
  /** What is still missing when `configured` is false. */
  problem?: string;
  /** The vault consumer the provider key must be stored for (`process:web`). */
  credential_consumer: string;
};

/**
 * Params of `web.settings.set`: omitted fields stay, `null` clears `endpoint` or
 * `secret_name`. Unknown fields (such as a key value) are refused. Changing the
 * provider clears `secret_name` unless the same patch names one.
 */
export type WebSettingsPatch = {
  enabled?: boolean;
  provider?: SearchProvider;
  endpoint?: string | null;
  secret_name?: string | null;
  max_results?: number;
  fetch_enabled?: boolean;
};

/** One `web.providers` entry. */
export type WebProviderInfo = {
  id: SearchProvider;
  display_name: string;
  /** Brand icon key. */
  icon: string;
  description: string;
  requires_key: boolean;
  accepts_key: boolean;
  requires_endpoint: boolean;
  /** The fixed search URL of a hosted provider. */
  endpoint: string | null;
  /** Where the user creates an API key. */
  key_url: string | null;
  docs_url: string;
};

/** One search result from the provider (untrusted text). */
export type WebSearchResult = {
  title: string;
  url: string;
  snippet: string;
  published?: string;
  source: SearchProvider;
};

/** Params of `web.test`; `settings` tests unsaved changes without storing them. */
export type WebTestParams = {
  query: string;
  max_results?: number;
  settings?: WebSettingsPatch;
};

/** Result of `web.test`. */
export type WebTestResult = {
  provider: SearchProvider;
  results: WebSearchResult[];
  elapsed_ms: number;
};

/** Stable `data.reason` values of `web.*` errors. */
export type WebErrorReason =
  | "invalid_request"
  | "invalid_url"
  | "target_refused"
  | "offline_refused"
  | "policy_denied"
  | "invalid_settings"
  | "not_configured"
  | "vault_locked"
  | "secret_not_found"
  | "secret_denied"
  | "secret_invalid"
  | "credential_unavailable"
  | "unauthorized"
  | "rate_limited"
  | "http_error"
  | "unsupported_content_type"
  | "too_many_redirects"
  | "protocol"
  | "timeout"
  | "cancelled"
  | "unavailable"
  | "io"
  | "https_required"
  | "browser_unavailable";

/** One hit in a `web_search` tool result. */
export type WebSearchHit = {
  title: string;
  url: string;
  snippet: string;
  published?: string;
};

/**
 * Content (JSON) of a `web_search` tool result message. Tool results are stored as
 * produced; the harness wraps them as untrusted data only in the model's prompt.
 */
export type WebSearchToolResult = {
  query: string;
  provider: string;
  results: WebSearchHit[];
};

/**
 * Content (JSON) of a `web_fetch` tool result message. Pages over 64 KiB are saved
 * as a `web_page` artifact and `text` holds only the first 64 KiB.
 */
export type WebFetchToolResult = {
  url: string;
  final_url: string;
  status: number;
  content_type: string;
  title: string | null;
  /** The text was cut to the inline limit (the artifact has all of it). */
  text_truncated: boolean;
  /** The fetch itself reached the client's size limit. */
  truncated: boolean;
  artifact?: { id: string; name: string; kind: "web_page"; size: number };
  /** Why a long page could not be saved as an artifact. */
  artifact_error?: string;
  text: string;
};
