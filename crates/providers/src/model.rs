//! Model descriptors: locality, limits, capabilities, and roles.

use serde::{Deserialize, Serialize};

use crate::ModelRef;

/// Where a model's inference runs, as far as data egress is concerned.
///
/// `Local` means the endpoint is on this host's loopback interface; anything
/// reachable over a network (including a LAN server) is `Remote`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Locality {
    Local,
    Remote,
}

impl Locality {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
        }
    }
}

/// How a model can call tools.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallSupport {
    /// Provider-native `tool_calls` in chat responses.
    Native,
    /// Tool use through JSON-schema (or grammar) constrained output; the harness
    /// drives its structured action protocol.
    Structured,
    /// No tool calling.
    #[default]
    None,
}

impl ToolCallSupport {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Structured => "structured",
            Self::None => "none",
        }
    }

    pub const fn is_supported(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// Work a model is suited for (spec §10.5).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRole {
    Code,
    Plan,
    Summarize,
    Classify,
    Embed,
    Verify,
    General,
}

impl ModelRole {
    pub const ALL: [Self; 7] = [
        Self::Code,
        Self::Plan,
        Self::Summarize,
        Self::Classify,
        Self::Embed,
        Self::Verify,
        Self::General,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Code => "code",
            Self::Plan => "plan",
            Self::Summarize => "summarize",
            Self::Classify => "classify",
            Self::Embed => "embed",
            Self::Verify => "verify",
            Self::General => "general",
        }
    }
}

impl std::str::FromStr for ModelRole {
    type Err = String;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|role| role.as_str() == text)
            .ok_or_else(|| {
                "unknown model role (use code, plan, summarize, classify, embed, verify, or general)"
                    .to_owned()
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelCapabilities {
    pub chat: bool,
    pub streaming: bool,
    pub tool_calls: ToolCallSupport,
    /// Accepts `response_format` JSON-schema constrained output.
    pub json_schema_output: bool,
    pub vision: bool,
    pub embeddings: bool,
}

impl Default for ModelCapabilities {
    /// A streaming chat model without tools, structured output, vision, or embeddings.
    fn default() -> Self {
        Self {
            chat: true,
            streaming: true,
            tool_calls: ToolCallSupport::None,
            json_schema_output: false,
            vision: false,
            embeddings: false,
        }
    }
}

/// Indicative price in US dollars per million tokens; informational only.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CostHint {
    pub input_per_million: f64,
    pub output_per_million: f64,
}

/// Where a descriptor's limits and capabilities came from.
///
/// The declaration order is the router's preference order: configured
/// metadata ranks before discovered, discovered before catalog, and catalog
/// before assumed.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilitySource {
    /// Declared by the user in `providers.toml` or by a runtime provider that
    /// knows its models (for example a verified local model registry).
    Configured,
    /// Reported by the provider's model listing (for example a context length).
    Discovered,
    /// The provider listed the model without limits, and the built-in table of
    /// well-known model families supplied conservative ones. Eligible for
    /// automatic routing like discovered metadata, but ranked after it. The
    /// values are lower bounds, so the harness does not enforce a Catalog
    /// context window as a hard limit.
    Catalog,
    /// Defaults for a listed model without metadata. The router does not select
    /// such models automatically; only an explicit user override can.
    Assumed,
}

impl CapabilitySource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Configured => "configured",
            Self::Discovered => "discovered",
            Self::Catalog => "catalog",
            Self::Assumed => "assumed",
        }
    }
}

/// Context window assumed for listed models that report no limit.
pub const ASSUMED_CONTEXT_WINDOW: u32 = 4096;

/// One model a provider can serve.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelDescriptor {
    pub model: ModelRef,
    pub display_name: String,
    pub locality: Locality,
    /// Total tokens (prompt + completion) the model accepts.
    pub context_window: u32,
    pub max_output_tokens: Option<u32>,
    pub capabilities: ModelCapabilities,
    pub roles: Vec<ModelRole>,
    pub cost: Option<CostHint>,
    /// Minimum host RAM to run a local model, when known.
    pub min_ram_bytes: Option<u64>,
    pub source: CapabilitySource,
}

impl ModelDescriptor {
    /// A descriptor with default capabilities, the `general` role, and no cost,
    /// RAM, or output limits.
    pub fn new(model: ModelRef, locality: Locality, context_window: u32) -> Self {
        Self {
            display_name: model.model_id.to_string(),
            model,
            locality,
            context_window,
            max_output_tokens: None,
            capabilities: ModelCapabilities::default(),
            roles: vec![ModelRole::General],
            cost: None,
            min_ram_bytes: None,
            source: CapabilitySource::Configured,
        }
    }

    #[must_use]
    pub fn with_capabilities(mut self, capabilities: ModelCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    #[must_use]
    pub fn with_tool_calls(mut self, support: ToolCallSupport) -> Self {
        self.capabilities.tool_calls = support;
        self
    }

    #[must_use]
    pub fn with_json_schema_output(mut self, supported: bool) -> Self {
        self.capabilities.json_schema_output = supported;
        self
    }

    #[must_use]
    pub fn with_roles(mut self, roles: impl IntoIterator<Item = ModelRole>) -> Self {
        self.roles = roles.into_iter().collect();
        self
    }

    #[must_use]
    pub fn with_source(mut self, source: CapabilitySource) -> Self {
        self.source = source;
        self
    }

    #[must_use]
    pub fn with_min_ram_bytes(mut self, bytes: u64) -> Self {
        self.min_ram_bytes = Some(bytes);
        self
    }

    #[must_use]
    pub fn with_max_output_tokens(mut self, tokens: u32) -> Self {
        self.max_output_tokens = Some(tokens);
        self
    }

    #[must_use]
    pub fn with_display_name(mut self, name: impl Into<String>) -> Self {
        self.display_name = name.into();
        self
    }

    pub fn has_role(&self, role: ModelRole) -> bool {
        self.roles.contains(&role)
    }
}
