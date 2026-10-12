//! Stable vendor identities describe supported protocols, not configured accounts.
//!
//! A preset is the starting point for adding an account: its protocol kind,
//! endpoint, how it authenticates, and where the user gets a key. Accounts
//! keep their own `ProviderId`; the preset id is only recorded as account
//! metadata (see `ProviderRegistry::add_provider_with`).
use std::fmt;

use crate::ProviderKind;
use serde::{Deserialize, Serialize};

/// How the provider presets are grouped for presentation.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresetCategory {
    /// A model vendor's own API.
    Cloud,
    /// A service routing to many vendors' models.
    Gateway,
    /// A server on this computer.
    Local,
    /// A cloud platform account (Azure, AWS).
    Enterprise,
    /// Any other compatible endpoint.
    #[default]
    Custom,
}

/// One way an account of a preset can authenticate.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    /// An API key stored in the vault.
    ApiKey,
    /// ChatGPT sign-in (`auth.start {method: "chatgpt"}`).
    OauthChatgpt,
    /// AWS SigV4 credentials stored in the vault as JSON.
    AwsSigv4,
    /// No credential (local servers).
    None,
}

/// A link a preset can open in the browser.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresetLink {
    /// Where the user creates an API key.
    ApiKey,
    /// The vendor's documentation.
    Docs,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderPreset {
    pub id: String,
    pub display_name: String,
    pub kind: ProviderKind,
    pub base_url: Option<String>,
    pub discover_models: bool,
    pub tool_calls: bool,
    pub json_schema_output: bool,
    /// Legacy summary of `auth_methods`: `api_key_vault`, `aws_sigv4_vault`,
    /// or `none` for key-less local presets.
    pub authentication: String,
    /// ChatGPT sign-in is available (the `openai` preset only).
    pub oauth_supported: bool,
    /// Brand glyph key used by the UI.
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub category: PresetCategory,
    /// One user-facing sentence.
    #[serde(default)]
    pub description: String,
    /// Supported authentication, preferred first.
    #[serde(default)]
    pub auth_methods: Vec<AuthMethod>,
    #[serde(default)]
    pub docs_url: Option<String>,
    #[serde(default)]
    pub api_key_url: Option<String>,
    /// What a key for this vendor looks like (`sk-...`), never a real key.
    #[serde(default)]
    pub key_placeholder: Option<String>,
}

impl ProviderPreset {
    /// The URL stored for `link`, if the preset has one.
    pub fn link(&self, link: PresetLink) -> Option<&str> {
        match link {
            PresetLink::ApiKey => self.api_key_url.as_deref(),
            PresetLink::Docs => self.docs_url.as_deref(),
        }
    }
}

/// Why [`preset_link`] could not resolve a link.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresetLinkError {
    /// No preset has this id.
    UnknownPreset,
    /// The preset has no such link.
    Missing,
    /// The stored link is not an `https://` URL (never opened).
    Insecure,
}

impl fmt::Display for PresetLinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnknownPreset => "unknown provider preset",
            Self::Missing => "this provider preset has no such link",
            Self::Insecure => "this provider preset link is not an HTTPS URL",
        })
    }
}

impl std::error::Error for PresetLinkError {}

/// The HTTPS URL a preset stores for `link`. Only URLs from the built-in
/// table are ever returned, so callers can open them without accepting a URL
/// from a client.
pub fn preset_link(preset_id: &str, link: PresetLink) -> Result<String, PresetLinkError> {
    let preset = preset(preset_id).ok_or(PresetLinkError::UnknownPreset)?;
    let url = preset.link(link).ok_or(PresetLinkError::Missing)?;
    if !url.starts_with("https://") || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(PresetLinkError::Insecure);
    }
    Ok(url.to_owned())
}

/// The preset with this id.
pub fn preset(id: &str) -> Option<ProviderPreset> {
    ROWS.iter().find(|row| row.id == id).map(Row::preset)
}

/// Every preset, in presentation order.
pub fn provider_presets() -> Vec<ProviderPreset> {
    ROWS.iter().map(Row::preset).collect()
}

struct Row {
    id: &'static str,
    display_name: &'static str,
    kind: ProviderKind,
    base_url: Option<&'static str>,
    icon: &'static str,
    category: PresetCategory,
    auth_methods: &'static [AuthMethod],
    api_key_url: Option<&'static str>,
    key_placeholder: Option<&'static str>,
    json_schema_output: bool,
    discover_models: bool,
    docs_url: Option<&'static str>,
    description: &'static str,
}

impl Row {
    fn preset(&self) -> ProviderPreset {
        let authentication = if self.auth_methods.contains(&AuthMethod::AwsSigv4) {
            "aws_sigv4_vault"
        } else if self.auth_methods.first() == Some(&AuthMethod::None) {
            "none"
        } else {
            "api_key_vault"
        };
        ProviderPreset {
            id: self.id.into(),
            display_name: self.display_name.into(),
            kind: self.kind,
            base_url: self.base_url.map(str::to_owned),
            discover_models: self.discover_models,
            tool_calls: true,
            json_schema_output: self.json_schema_output,
            authentication: authentication.into(),
            oauth_supported: self.auth_methods.contains(&AuthMethod::OauthChatgpt),
            icon: self.icon.into(),
            category: self.category,
            description: self.description.into(),
            auth_methods: self.auth_methods.to_vec(),
            docs_url: self.docs_url.map(str::to_owned),
            api_key_url: self.api_key_url.map(str::to_owned),
            key_placeholder: self.key_placeholder.map(str::to_owned),
        }
    }
}

use AuthMethod::{ApiKey, AwsSigv4, None as NoAuth, OauthChatgpt};
use PresetCategory::{Cloud, Custom, Enterprise, Gateway, Local};

const fn row(
    id: &'static str,
    display_name: &'static str,
    kind: ProviderKind,
    base_url: Option<&'static str>,
    icon: &'static str,
    category: PresetCategory,
    auth_methods: &'static [AuthMethod],
) -> Row {
    Row {
        id,
        display_name,
        kind,
        base_url,
        icon,
        category,
        auth_methods,
        api_key_url: None,
        key_placeholder: None,
        json_schema_output: true,
        discover_models: true,
        docs_url: None,
        description: "",
    }
}

impl Row {
    const fn key(mut self, api_key_url: &'static str, placeholder: Option<&'static str>) -> Self {
        self.api_key_url = Some(api_key_url);
        self.key_placeholder = placeholder;
        self
    }

    const fn docs(mut self, url: &'static str) -> Self {
        self.docs_url = Some(url);
        self
    }

    const fn about(mut self, description: &'static str) -> Self {
        self.description = description;
        self
    }

    const fn no_json(mut self) -> Self {
        self.json_schema_output = false;
        self
    }

    const fn declared_only(mut self) -> Self {
        self.discover_models = false;
        self
    }
}

const OPENAI_COMPATIBLE: ProviderKind = ProviderKind::OpenaiCompatible;

static ROWS: [Row; 25] = [
    row(
        "openai",
        "OpenAI",
        ProviderKind::OpenaiResponses,
        Some("https://api.openai.com/v1"),
        "openai",
        Cloud,
        &[OauthChatgpt, ApiKey],
    )
    .key("https://platform.openai.com/api-keys", Some("sk-..."))
    .docs("https://platform.openai.com/docs/models")
    .about("GPT and o-series models through your ChatGPT plan or an API key."),
    row(
        "anthropic",
        "Anthropic",
        ProviderKind::Anthropic,
        Some("https://api.anthropic.com/v1"),
        "claude",
        Cloud,
        &[ApiKey],
    )
    .key(
        "https://console.anthropic.com/settings/keys",
        Some("sk-ant-..."),
    )
    .docs("https://docs.anthropic.com/en/docs/about-claude/models")
    .about("Claude models through the Anthropic API.")
    .no_json(),
    row(
        "gemini",
        "Google Gemini",
        ProviderKind::Gemini,
        Some("https://generativelanguage.googleapis.com/v1beta"),
        "gemini",
        Cloud,
        &[ApiKey],
    )
    .key("https://aistudio.google.com/app/apikey", Some("AIza..."))
    .docs("https://ai.google.dev/gemini-api/docs/models")
    .about("Gemini models through the Gemini API from Google AI Studio."),
    row(
        "xai",
        "xAI",
        OPENAI_COMPATIBLE,
        Some("https://api.x.ai/v1"),
        "xai",
        Cloud,
        &[ApiKey],
    )
    .key("https://console.x.ai", Some("xai-..."))
    .docs("https://docs.x.ai/docs/models")
    .about("Grok models through the xAI API."),
    row(
        "mistral",
        "Mistral AI",
        OPENAI_COMPATIBLE,
        Some("https://api.mistral.ai/v1"),
        "mistral",
        Cloud,
        &[ApiKey],
    )
    .key("https://console.mistral.ai/api-keys", None)
    .docs("https://docs.mistral.ai/getting-started/models/")
    .about("Mistral, Codestral, and Devstral models through La Plateforme."),
    row(
        "deepseek",
        "DeepSeek",
        OPENAI_COMPATIBLE,
        Some("https://api.deepseek.com/v1"),
        "deepseek",
        Cloud,
        &[ApiKey],
    )
    .key("https://platform.deepseek.com/api_keys", Some("sk-..."))
    .docs("https://api-docs.deepseek.com/")
    .about("DeepSeek chat and reasoning models through the DeepSeek API.")
    .no_json(),
    row(
        "groq",
        "Groq",
        OPENAI_COMPATIBLE,
        Some("https://api.groq.com/openai/v1"),
        "groq",
        Cloud,
        &[ApiKey],
    )
    .key("https://console.groq.com/keys", Some("gsk_..."))
    .docs("https://console.groq.com/docs/models")
    .about("Fast inference of open models on Groq hardware."),
    row(
        "moonshot",
        "Moonshot AI (Kimi)",
        OPENAI_COMPATIBLE,
        Some("https://api.moonshot.ai/v1"),
        "kimi",
        Cloud,
        &[ApiKey],
    )
    .key(
        "https://platform.moonshot.ai/console/api-keys",
        Some("sk-..."),
    )
    .docs("https://platform.moonshot.ai/docs")
    .about("Kimi models through the Moonshot AI platform.")
    .no_json(),
    row(
        "cerebras",
        "Cerebras",
        OPENAI_COMPATIBLE,
        Some("https://api.cerebras.ai/v1"),
        "cerebras",
        Cloud,
        &[ApiKey],
    )
    .key("https://cloud.cerebras.ai", Some("csk-..."))
    .docs("https://inference-docs.cerebras.ai/")
    .about("Fast inference of open models on Cerebras hardware."),
    row(
        "qwen",
        "Alibaba Model Studio (Qwen)",
        OPENAI_COMPATIBLE,
        Some("https://dashscope-intl.aliyuncs.com/compatible-mode/v1"),
        "qwen",
        Cloud,
        &[ApiKey],
    )
    .key(
        "https://modelstudio.console.alibabacloud.com",
        Some("sk-..."),
    )
    .docs("https://www.alibabacloud.com/help/en/model-studio/")
    .about("Qwen models through Alibaba Cloud Model Studio."),
    row(
        "openai-chat",
        "OpenAI (Chat Completions)",
        OPENAI_COMPATIBLE,
        Some("https://api.openai.com/v1"),
        "openai",
        Cloud,
        &[ApiKey],
    )
    .key("https://platform.openai.com/api-keys", Some("sk-..."))
    .docs("https://platform.openai.com/docs/api-reference/chat")
    .about("OpenAI models through the Chat Completions API with an API key."),
    row(
        "openrouter",
        "OpenRouter",
        OPENAI_COMPATIBLE,
        Some("https://openrouter.ai/api/v1"),
        "openrouter",
        Gateway,
        &[ApiKey],
    )
    .key("https://openrouter.ai/keys", Some("sk-or-..."))
    .docs("https://openrouter.ai/docs")
    .about("Hundreds of models from many vendors behind one API key."),
    row(
        "together",
        "Together AI",
        OPENAI_COMPATIBLE,
        Some("https://api.together.xyz/v1"),
        "together",
        Gateway,
        &[ApiKey],
    )
    .key("https://api.together.ai/settings/api-keys", None)
    .docs("https://docs.together.ai/docs/serverless-models")
    .about("Open models hosted by Together AI."),
    row(
        "fireworks",
        "Fireworks AI",
        OPENAI_COMPATIBLE,
        Some("https://api.fireworks.ai/inference/v1"),
        "fireworks",
        Gateway,
        &[ApiKey],
    )
    .key("https://fireworks.ai/account/api-keys", Some("fw_..."))
    .docs("https://docs.fireworks.ai/")
    .about("Open models hosted by Fireworks AI."),
    row(
        "deepinfra",
        "DeepInfra",
        OPENAI_COMPATIBLE,
        Some("https://api.deepinfra.com/v1/openai"),
        "deepinfra",
        Gateway,
        &[ApiKey],
    )
    .key("https://deepinfra.com/dash/api_keys", None)
    .docs("https://deepinfra.com/docs")
    .about("Open models hosted by DeepInfra."),
    row(
        "huggingface",
        "Hugging Face",
        OPENAI_COMPATIBLE,
        Some("https://router.huggingface.co/v1"),
        "huggingface",
        Gateway,
        &[ApiKey],
    )
    .key("https://huggingface.co/settings/tokens", Some("hf_..."))
    .docs("https://huggingface.co/docs/inference-providers")
    .about("Open models through Hugging Face Inference Providers."),
    row(
        "nvidia",
        "NVIDIA NIM",
        OPENAI_COMPATIBLE,
        Some("https://integrate.api.nvidia.com/v1"),
        "nvidia",
        Gateway,
        &[ApiKey],
    )
    .key("https://build.nvidia.com", Some("nvapi-..."))
    .docs("https://docs.api.nvidia.com/")
    .about("Models hosted as NVIDIA NIM microservices."),
    row(
        "vercel",
        "Vercel AI Gateway",
        OPENAI_COMPATIBLE,
        Some("https://ai-gateway.vercel.sh/v1"),
        "vercel",
        Gateway,
        &[ApiKey],
    )
    .key("https://vercel.com/docs/ai-gateway", None)
    .docs("https://vercel.com/docs/ai-gateway")
    .about("Models from many vendors through the Vercel AI Gateway."),
    row(
        "azure-openai",
        "Azure OpenAI",
        ProviderKind::AzureOpenai,
        None,
        "azure",
        Enterprise,
        &[ApiKey],
    )
    .key("https://portal.azure.com", None)
    .docs("https://learn.microsoft.com/azure/ai-foundry/openai/")
    .about("Your Azure OpenAI deployments; enter the resource endpoint and deployment names.")
    .declared_only(),
    row(
        "bedrock",
        "Amazon Bedrock",
        ProviderKind::Bedrock,
        None,
        "bedrock",
        Enterprise,
        &[AwsSigv4],
    )
    .key("https://console.aws.amazon.com/bedrock", None)
    .docs("https://docs.aws.amazon.com/bedrock/")
    .about("Foundation models in your AWS account; enter the regional endpoint and model IDs.")
    .no_json()
    .declared_only(),
    row(
        "ollama",
        "Ollama",
        OPENAI_COMPATIBLE,
        Some("http://127.0.0.1:11434/v1"),
        "ollama",
        Local,
        &[NoAuth],
    )
    .docs("https://ollama.com")
    .about("Models served by Ollama on this computer."),
    row(
        "lmstudio",
        "LM Studio",
        OPENAI_COMPATIBLE,
        Some("http://127.0.0.1:1234/v1"),
        "lmstudio",
        Local,
        &[NoAuth],
    )
    .docs("https://lmstudio.ai/docs")
    .about("Models served by LM Studio's local server on this computer."),
    row(
        "llamacpp",
        "llama.cpp server",
        OPENAI_COMPATIBLE,
        Some("http://127.0.0.1:8080/v1"),
        "llamacpp",
        Local,
        &[NoAuth],
    )
    .docs("https://github.com/ggml-org/llama.cpp")
    .about("A llama-server you run yourself on this computer."),
    row(
        "vllm",
        "vLLM",
        OPENAI_COMPATIBLE,
        Some("http://127.0.0.1:8000/v1"),
        "vllm",
        Local,
        &[NoAuth, ApiKey],
    )
    .docs("https://docs.vllm.ai/")
    .about("A vLLM OpenAI-compatible server on this computer, with an optional API key."),
    row(
        "custom",
        "Custom OpenAI-compatible",
        OPENAI_COMPATIBLE,
        None,
        "custom",
        Custom,
        &[ApiKey, NoAuth],
    )
    .about("Any server that speaks the OpenAI Chat Completions API."),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProviderId;
    use std::collections::HashSet;

    #[test]
    fn presets_are_unique_valid_and_link_only_https() {
        let presets = provider_presets();
        assert_eq!(presets.len(), 25);
        let mut ids = HashSet::new();
        for p in &presets {
            assert!(ids.insert(p.id.clone()), "duplicate {}", p.id);
            assert!(ProviderId::new(&p.id).is_ok(), "{}", p.id);
            assert!(!p.description.is_empty(), "{}", p.id);
            assert!(!p.icon.is_empty(), "{}", p.id);
            assert!(!p.auth_methods.is_empty(), "{}", p.id);
            for url in [&p.docs_url, &p.api_key_url].into_iter().flatten() {
                assert!(url.starts_with("https://"), "{}: {url}", p.id);
            }
            if let Some(url) = &p.base_url {
                assert!(
                    url.starts_with("https://") || url.starts_with("http://127.0.0.1:"),
                    "{}: {url}",
                    p.id
                );
            }
            assert_eq!(p.oauth_supported, p.id == "openai", "{}", p.id);
            assert_eq!(preset(&p.id).as_ref(), Some(p));
        }
        assert_eq!(presets[0].id, "openai");
        assert_eq!(
            presets[0].auth_methods,
            [AuthMethod::OauthChatgpt, AuthMethod::ApiKey]
        );
        let ollama = preset("ollama").unwrap();
        assert_eq!(ollama.authentication, "none");
        assert_eq!(ollama.category, PresetCategory::Local);
        assert_eq!(preset("bedrock").unwrap().authentication, "aws_sigv4_vault");
        assert_eq!(preset("custom").unwrap().authentication, "api_key_vault");
        assert_eq!(preset("missing"), None);
    }

    #[test]
    fn preset_links_resolve_only_stored_https_urls() {
        assert_eq!(
            preset_link("anthropic", PresetLink::ApiKey).unwrap(),
            "https://console.anthropic.com/settings/keys"
        );
        assert_eq!(
            preset_link("nope", PresetLink::Docs),
            Err(PresetLinkError::UnknownPreset)
        );
        assert_eq!(
            preset_link("ollama", PresetLink::ApiKey),
            Err(PresetLinkError::Missing)
        );
        assert_eq!(
            preset_link("https://evil.example", PresetLink::Docs),
            Err(PresetLinkError::UnknownPreset)
        );
    }

    #[test]
    fn serialized_presets_carry_every_field() {
        let value = serde_json::to_value(preset("openai").unwrap()).unwrap();
        assert_eq!(value["category"], "cloud");
        assert_eq!(value["auth_methods"][0], "oauth_chatgpt");
        assert_eq!(value["key_placeholder"], "sk-...");
        let custom = serde_json::to_value(preset("custom").unwrap()).unwrap();
        assert!(custom["base_url"].is_null());
        assert!(custom["api_key_url"].is_null());
        assert!(custom["key_placeholder"].is_null());
        let parsed: ProviderPreset = serde_json::from_value(value).unwrap();
        assert_eq!(parsed, preset("openai").unwrap());
    }
}
