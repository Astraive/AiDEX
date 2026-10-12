//! Per-protocol model listings: Anthropic and Gemini pagination, Gemini
//! generation-method filtering, and the ChatGPT plan catalog envelope.

use serde_json::Value;

use crate::{
    DiscoveredModel, ModelId, ProviderError,
    openai::wire::{check_list_size, model_label, parse_model_list, positive_u32},
};

/// Most pages followed for one listing; a longer chain is cut off here.
pub(super) const MAX_PAGES: usize = 10;

/// One parsed page and the cursor for the next one, if any.
pub(super) struct Page {
    pub models: Vec<DiscoveredModel>,
    pub next: Option<String>,
}

fn malformed() -> ProviderError {
    ProviderError::Protocol("malformed model list".into())
}

fn parse(body: &str) -> Result<Value, ProviderError> {
    serde_json::from_str(body).map_err(|_| malformed())
}

/// A non-empty, bounded pagination cursor.
fn cursor(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|cursor| !cursor.is_empty() && cursor.len() <= 1024)
        .map(str::to_owned)
}

/// `GET models?limit=1000[&after_id=...]`: `data[]` with `display_name`,
/// continued while `has_more` with `last_id`.
pub(super) fn anthropic_page(body: &str) -> Result<Page, ProviderError> {
    let value = parse(body)?;
    let models = parse_model_list(&value)?;
    let next = if value.get("has_more").and_then(Value::as_bool) == Some(true) {
        cursor(value.get("last_id"))
    } else {
        None
    };
    Ok(Page { models, next })
}

/// `GET models?pageSize=1000[&pageToken=...]`: `models[]` filtered to entries
/// that generate content (chat) or embed content (embeddings only).
pub(super) fn gemini_page(body: &str) -> Result<Page, ProviderError> {
    let value = parse(body)?;
    let entries = value
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(malformed)?;
    check_list_size(entries.len())?;
    let models = entries.iter().filter_map(gemini_entry).collect();
    Ok(Page {
        models,
        next: cursor(value.get("nextPageToken")),
    })
}

fn gemini_entry(entry: &Value) -> Option<DiscoveredModel> {
    let methods = entry
        .get("supportedGenerationMethods")
        .and_then(Value::as_array)?;
    let has = |name: &str| methods.iter().any(|method| method.as_str() == Some(name));
    let (generates, embeds) = (has("generateContent"), has("embedContent"));
    if !generates && !embeds {
        return None;
    }
    let raw = entry.get("name")?.as_str()?;
    let id = ModelId::new(raw.strip_prefix("models/").unwrap_or(raw)).ok()?;
    let mut model = DiscoveredModel::new(id);
    model.display_name = model_label(entry.get("displayName"), &model.id);
    model.context_window = positive_u32(entry.get("inputTokenLimit"));
    model.max_output_tokens = positive_u32(entry.get("outputTokenLimit"));
    if !generates {
        model.embeddings = Some(true);
        model.chat = Some(false);
    }
    Some(model)
}

/// The ChatGPT plan-token catalog (`models[]` of `slug`, `display_name`,
/// `visibility`); only entries with `visibility: "list"` are offered, in
/// server order.
pub(super) fn chatgpt_catalog(value: &Value) -> Result<Vec<DiscoveredModel>, ProviderError> {
    let entries = value
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(malformed)?;
    check_list_size(entries.len())?;
    Ok(entries
        .iter()
        .filter(|entry| entry.get("visibility").and_then(Value::as_str) == Some("list"))
        .filter_map(|entry| {
            let id = ModelId::new(entry.get("slug")?.as_str()?).ok()?;
            let mut model = DiscoveredModel::new(id);
            model.display_name = model_label(entry.get("display_name"), &model.id);
            model.context_window = positive_u32(entry.get("context_window"))
                .or_else(|| positive_u32(entry.get("context_length")));
            Some(model)
        })
        .collect())
}
