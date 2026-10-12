//! Conservative metadata for well-known model families.
//!
//! Many vendors list model ids without limits or capabilities (Anthropic,
//! OpenAI, xAI, DeepSeek, ...). After a listing is parsed, this table fills
//! only the fields the vendor did not report, and only for families whose
//! limits are stable and documented. Values are lower bounds: a model may
//! accept more than the table says, never less. Descriptors whose context
//! window comes from this table are marked [`crate::CapabilitySource::Catalog`].
//!
//! Matching uses the lowercase model id with any vendor or route prefix
//! (`anthropic/`, `openai/`, `x-ai/`, `google/`, `models/`, ...) and any
//! `:variant`/`@version` suffix removed, so gateways benefit too.

use crate::{DiscoveredModel, ToolCallSupport};

/// What the table knows about a family. `None` leaves a field alone.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Hint {
    pub context_window: Option<u32>,
    pub tool_calls: Option<ToolCallSupport>,
    pub json_schema_output: Option<bool>,
    pub vision: Option<bool>,
    pub chat: Option<bool>,
    pub embeddings: Option<bool>,
}

impl Hint {
    const fn chat_model(context_window: Option<u32>) -> Self {
        Self {
            context_window,
            tool_calls: Some(ToolCallSupport::Native),
            json_schema_output: None,
            vision: None,
            chat: None,
            embeddings: None,
        }
    }

    const fn with_json(mut self) -> Self {
        self.json_schema_output = Some(true);
        self
    }

    const fn with_vision(mut self) -> Self {
        self.vision = Some(true);
        self
    }

    const fn not_chat() -> Self {
        Self {
            context_window: None,
            tool_calls: None,
            json_schema_output: None,
            vision: None,
            chat: Some(false),
            embeddings: None,
        }
    }
}

/// Fill unreported fields of `model` from its family's hint. Returns `true`
/// when the context window came from the table.
pub(crate) fn apply(model: &mut DiscoveredModel) -> bool {
    let Some(hint) = hint_for(model.id.as_str()) else {
        return false;
    };
    fill(&mut model.chat, hint.chat);
    fill(&mut model.embeddings, hint.embeddings);
    if model.chat == Some(false) || model.embeddings == Some(true) {
        // Chat limits and tool support mean nothing for non-chat models.
        return false;
    }
    fill(&mut model.tool_calls, hint.tool_calls);
    fill(&mut model.json_schema_output, hint.json_schema_output);
    fill(&mut model.vision, hint.vision);
    if model.context_window.is_none() && hint.context_window.is_some() {
        model.context_window = hint.context_window;
        return true;
    }
    false
}

fn fill<T>(field: &mut Option<T>, hint: Option<T>) {
    if field.is_none() {
        *field = hint;
    }
}

/// The matching key: lowercase, last `/` segment, without `:`/`@` suffixes.
pub(crate) fn family_key(id: &str) -> String {
    let id = id.to_ascii_lowercase();
    let name = id.rsplit('/').next().unwrap_or_default();
    let end = name.find([':', '@']).unwrap_or(name.len());
    name[..end].to_owned()
}

/// The hint for a model id, if its family is known.
pub(crate) fn hint_for(id: &str) -> Option<Hint> {
    let key = family_key(id);
    let key = key.as_str();
    if key.is_empty() {
        return None;
    }
    if let Some(hint) = non_chat(key) {
        return Some(hint);
    }
    if let Some(rest) = key.strip_prefix("claude-") {
        // Claude 3 and later; claude-1, claude-2.x and claude-instant are older.
        let legacy = rest.starts_with("instant")
            || rest
                .split(['-', '.'])
                .next()
                .is_some_and(|major| major == "1" || major == "2");
        return (!legacy).then(|| Hint::chat_model(Some(200_000)).with_vision());
    }
    if is_gpt_family(key) {
        return Some(Hint::chat_model(Some(128_000)).with_json().with_vision());
    }
    if let Some((major, suffix)) = o_series(key) {
        let hint = Hint::chat_model(Some(128_000)).with_json();
        // o1 (not -mini/-preview), o1-pro, o3, o3-pro, o4-mini and later
        // models read images; o3-mini and later -mini models are text-only.
        let vision = major == 1 || major == 4 || !suffix.starts_with("-mini");
        return Some(if vision { hint.with_vision() } else { hint });
    }
    if key.starts_with("gemini-") {
        return Some(Hint::chat_model(None).with_json().with_vision());
    }
    if let Some(rest) = key.strip_prefix("grok-") {
        let major = leading_number(rest);
        let vision = key.contains("vision") || major.is_some_and(|major| major >= 4);
        if major.is_some_and(|major| major >= 2) {
            let hint = Hint::chat_model(Some(131_072)).with_json();
            return Some(if vision { hint.with_vision() } else { hint });
        }
        if vision {
            return Some(Hint::default().with_vision());
        }
    }
    // Llama 3.2 Vision and the natively multimodal Llama 4.
    if ((key.starts_with("llama-3.2") || key.starts_with("llama3.2")) && key.contains("vision"))
        || key.starts_with("llama-4")
        || key.starts_with("llama4")
    {
        return Some(Hint::default().with_vision());
    }
    match key {
        "deepseek-chat" => return Some(Hint::chat_model(Some(64_000))),
        "deepseek-reasoner" => {
            return Some(Hint {
                context_window: Some(64_000),
                ..Hint::default()
            });
        }
        _ => {}
    }
    if [
        "mistral-large",
        "mistral-medium",
        "mistral-small",
        "codestral",
        "magistral",
        "devstral",
        "pixtral",
    ]
    .iter()
    .any(|family| key.starts_with(family))
    {
        let hint = Hint::chat_model(Some(32_000));
        return Some(if key.starts_with("pixtral") {
            hint.with_vision()
        } else {
            hint
        });
    }
    if key.starts_with("kimi-k2") {
        return Some(Hint::chat_model(Some(128_000)));
    }
    if key.starts_with("kimi-") || key.starts_with("moonshot-") {
        return Some(Hint::chat_model(None));
    }
    None
}

/// OpenAI families (and audio/realtime/search/image variants of any family)
/// that do not accept chat completions.
fn non_chat(key: &str) -> Option<Hint> {
    if key.starts_with("text-embedding") {
        return Some(Hint {
            embeddings: Some(true),
            ..Hint::not_chat()
        });
    }
    let families = [
        "dall-e",
        "whisper",
        "tts",
        "omni-moderation",
        "text-moderation",
        "babbage",
        "davinci",
        "sora",
        "gpt-image",
    ];
    let variants = ["-realtime", "-audio", "-transcribe", "-tts", "-search"];
    (families.iter().any(|family| key.starts_with(family))
        || variants.iter().any(|variant| key.contains(variant)))
    .then(Hint::not_chat)
}

/// `gpt-4o*`, `gpt-4.1*`, and `gpt-5*` through `gpt-9*` (mini/nano included).
fn is_gpt_family(key: &str) -> bool {
    let Some(rest) = key.strip_prefix("gpt-") else {
        return false;
    };
    rest.starts_with("4o")
        || rest.starts_with("4.1")
        || leading_number(rest).is_some_and(|major| (5..=9).contains(&major))
}

/// `o1` (not `o1-mini`/`o1-preview`), `o3*`, `o4*`, and later o-series
/// models, as `(major, suffix)` (`"o3-mini"` gives `(3, "-mini")`).
fn o_series(key: &str) -> Option<(u32, &str)> {
    let rest = key.strip_prefix('o')?;
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let major = rest[..digits].parse::<u32>().ok()?;
    let suffix = &rest[digits..];
    if !(suffix.is_empty() || suffix.starts_with('-')) {
        return None;
    }
    let listed = if major == 1 {
        !(suffix.starts_with("-mini") || suffix.starts_with("-preview"))
    } else {
        major >= 3
    };
    listed.then_some((major, suffix))
}

/// The decimal number at the start of `text` (`"5.1-mini"` gives 5), when
/// it is followed by the end or a separator.
fn leading_number(text: &str) -> Option<u32> {
    let digits = text.bytes().take_while(u8::is_ascii_digit).count();
    let rest = &text[digits..];
    if digits == 0 || !(rest.is_empty() || rest.starts_with(['-', '.'])) {
        return None;
    }
    text[..digits].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ModelId;

    fn hinted(id: &str) -> DiscoveredModel {
        let mut model = DiscoveredModel::new(ModelId::new(id).unwrap());
        apply(&mut model);
        model
    }

    #[test]
    fn keys_drop_vendor_prefixes_and_variant_suffixes() {
        assert_eq!(
            family_key("anthropic/Claude-3.5-Sonnet:beta"),
            "claude-3.5-sonnet"
        );
        assert_eq!(family_key("models/gemini-2.5-pro"), "gemini-2.5-pro");
        assert_eq!(family_key("x-ai/grok-4"), "grok-4");
        assert_eq!(
            family_key("claude-3-5-sonnet@20240620"),
            "claude-3-5-sonnet"
        );
    }

    #[test]
    fn vendor_prefixed_ids_match_their_families() {
        for id in [
            "anthropic/claude-3.5-sonnet",
            "claude-sonnet-4-20250514",
            "claude-3-opus-20240229",
            "claude-opus-4-1",
        ] {
            let hint = hint_for(id).unwrap_or_else(|| panic!("{id} should match"));
            assert_eq!(hint.context_window, Some(200_000), "{id}");
            assert_eq!(hint.tool_calls, Some(ToolCallSupport::Native), "{id}");
            assert_eq!(hint.vision, Some(true), "{id}");
        }
        for id in ["claude-2.1", "claude-instant-1.2", "claude-2"] {
            assert_eq!(hint_for(id), None, "{id} predates Claude 3");
        }
        let grok = hint_for("x-ai/grok-4").unwrap();
        assert_eq!(grok.context_window, Some(131_072));
        assert_eq!(grok.json_schema_output, Some(true));
        assert_eq!(
            hint_for("grok-3-mini").unwrap().context_window,
            Some(131_072)
        );
        assert_eq!(hint_for("grok-beta"), None);
        assert_eq!(hint_for("grok-1"), None);
        let gemini = hint_for("google/gemini-2.5-flash").unwrap();
        assert_eq!(gemini.context_window, None, "context comes from the API");
        assert_eq!(gemini.tool_calls, Some(ToolCallSupport::Native));
        assert_eq!(
            hint_for("openai/gpt-4o:free").unwrap().context_window,
            Some(128_000)
        );
        assert_eq!(
            hint_for("deepseek/deepseek-chat").unwrap().tool_calls,
            Some(ToolCallSupport::Native)
        );
        assert_eq!(hint_for("deepseek-reasoner").unwrap().tool_calls, None);
        assert_eq!(
            hint_for("moonshotai/kimi-k2-0905").unwrap().context_window,
            Some(128_000)
        );
        assert_eq!(hint_for("kimi-latest").unwrap().context_window, None);
        assert_eq!(
            hint_for("mistralai/mistral-large-latest")
                .unwrap()
                .context_window,
            Some(32_000)
        );
        for id in ["meta-llama/llama-3.1-8b", "qwen", "mystery", "gpt-oss-120b"] {
            assert_eq!(hint_for(id), None, "{id}");
        }
    }

    #[test]
    fn openai_families_are_split_into_chat_and_non_chat() {
        for id in [
            "gpt-4o",
            "gpt-4o-mini",
            "gpt-4.1-nano",
            "gpt-5",
            "gpt-5.1-mini",
            "gpt-6.1-sol",
        ] {
            let hint = hint_for(id).unwrap_or_else(|| panic!("{id} should match"));
            assert_eq!(hint.context_window, Some(128_000), "{id}");
            assert_eq!(hint.json_schema_output, Some(true), "{id}");
            assert_eq!(hint.vision, Some(true), "{id}");
        }
        for id in ["o1", "o1-2024-12-17", "o3", "o3-mini", "o4-mini", "o5-pro"] {
            let hint = hint_for(id).unwrap_or_else(|| panic!("{id} should match"));
            assert_eq!(hint.context_window, Some(128_000), "{id}");
            assert_eq!(hint.tool_calls, Some(ToolCallSupport::Native), "{id}");
        }
        for id in ["o1-mini", "o1-preview", "omni-moderation-latest"] {
            assert_ne!(
                hint_for(id).and_then(|hint| hint.context_window),
                Some(128_000),
                "{id}"
            );
        }
        for id in [
            "gpt-4o-realtime-preview",
            "gpt-4o-audio-preview",
            "gpt-4o-mini-transcribe",
            "gpt-4o-mini-tts",
            "gpt-4o-search-preview",
            "gpt-image-1",
            "dall-e-3",
            "whisper-1",
            "tts-1-hd",
            "omni-moderation-latest",
            "text-moderation-stable",
            "babbage-002",
            "davinci-002",
            "sora-2",
        ] {
            let model = hinted(id);
            assert_eq!(model.chat, Some(false), "{id}");
            assert_eq!(model.context_window, None, "{id}");
            assert_eq!(model.tool_calls, None, "{id}");
        }
        let embedding = hinted("text-embedding-3-small");
        assert_eq!(embedding.embeddings, Some(true));
        assert_eq!(embedding.chat, Some(false));
        assert_eq!(hint_for("codex-mini-latest"), None, "stays a chat model");
        assert_eq!(hint_for("gpt-4-turbo"), None);
    }

    #[test]
    fn vision_hints_follow_each_family() {
        let vision = |id: &str| hint_for(id).and_then(|hint| hint.vision);
        for id in [
            "o1",
            "o1-pro",
            "o1-2024-12-17",
            "o3",
            "o3-pro",
            "openai/o3-2025-04-16",
            "o4-mini",
            "o4-mini-2025-04-16",
            "o5",
            "o5-pro",
            "pixtral-large-latest",
            "mistralai/pixtral-12b-2409",
            "grok-4",
            "x-ai/grok-4-fast",
            "grok-2-vision-1212",
            "grok-vision-beta",
            "meta-llama/Llama-3.2-11B-Vision-Instruct",
            "llama3.2-vision:11b",
            "meta-llama/llama-4-scout-17b-16e-instruct",
            "llama4:maverick",
        ] {
            assert_eq!(vision(id), Some(true), "{id}");
        }
        for id in [
            "o3-mini",
            "o3-mini-2025-01-31",
            "openai/o3-mini-high",
            "o5-mini",
            "grok-3",
            "grok-3-mini",
            "mistral-large-latest",
            "codestral-latest",
            "deepseek-chat",
        ] {
            assert_ne!(vision(id), Some(true), "{id}");
        }
        for id in [
            "meta-llama/llama-3.2-3b-instruct",
            "llama-3.1-70b",
            "grok-beta",
        ] {
            assert_eq!(hint_for(id), None, "{id}");
        }
        let mut o3_mini = DiscoveredModel::new(ModelId::new("o3-mini").unwrap());
        apply(&mut o3_mini);
        assert_eq!(o3_mini.vision, None, "o3-mini does not read images");
        let llama = hint_for("llama-4-scout").unwrap();
        assert_eq!(llama.context_window, None);
        assert_eq!(llama.tool_calls, None);
    }

    #[test]
    fn hints_fill_only_unreported_fields() {
        let mut model = DiscoveredModel::new(ModelId::new("claude-sonnet-4-5").unwrap());
        model.context_window = Some(1_000_000);
        model.tool_calls = Some(ToolCallSupport::None);
        assert!(!apply(&mut model), "the reported context stays");
        assert_eq!(model.context_window, Some(1_000_000));
        assert_eq!(model.tool_calls, Some(ToolCallSupport::None));
        assert_eq!(model.vision, Some(true));

        let mut unknown = DiscoveredModel::new(ModelId::new("claude-haiku-4-5").unwrap());
        assert!(apply(&mut unknown));
        assert_eq!(unknown.context_window, Some(200_000));

        let mut embed = DiscoveredModel::new(ModelId::new("gemini-embedding-001").unwrap());
        embed.embeddings = Some(true);
        embed.chat = Some(false);
        assert!(!apply(&mut embed));
        assert_eq!(embed.tool_calls, None, "embedding models get no chat hints");
    }
}
