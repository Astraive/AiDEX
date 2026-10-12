//! Which image and PDF input a model accepts through its provider account,
//! and the vendor limits on that input.
//!
//! Everything here is computed from descriptors and never persisted:
//! `providers.toml` and `ModelCapabilities` stay readable by older daemons.
//! The harness uses these to gate and budget attachments per attempt; the
//! daemon reports [`ModelInput`] in `model.list`.

use serde::{Deserialize, Serialize};

use crate::{
    AuthConfig, ChatRequest, ContentPart, IMAGE_MIMES, ModelDescriptor, PDF_PAGE_TOKENS,
    ProviderDescriptor, ProviderError, ProviderKind, parts::is_animated_gif,
};

/// Whether a model reads images and PDFs through its provider account.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct ModelInput {
    pub images: bool,
    pub pdf: bool,
}

/// Vendor limits on the image and PDF parts of one request.
///
/// Byte budgets on whole requests are measured on the base64 encoding that
/// reaches the vendor ([`ContentPart::encoded_len`]), plus any attachment
/// text the caller inlines; per-part sizes are raw bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PartLimits {
    /// Most encoded bytes of image and PDF parts in one request, history
    /// included.
    pub max_request_encoded_bytes: u64,
    /// Largest image.
    pub max_image_bytes: u64,
    /// Largest PDF.
    pub max_pdf_bytes: u64,
    /// Longest image edge in pixels the vendor accepts at all.
    pub max_image_edge_px: Option<u32>,
    /// Most images in one message.
    pub max_images_per_message: Option<usize>,
    /// Most PDFs in one message.
    pub max_pdfs_per_message: Option<usize>,
    /// Most PDF pages in one request.
    pub max_pdf_pages_per_request: Option<u32>,
    /// Most images in one request, history included (PDFs do not count).
    pub max_images_per_request: Option<usize>,
    /// The many-image rule, when the vendor has one.
    pub many_images: Option<ManyImages>,
    /// Prompt tokens one PDF page is estimated to cost on this family
    /// ([`ChatRequest::estimated_input_tokens_with`]).
    pub pdf_page_tokens: u32,
}

/// When a request carries more than `above` image blocks (PDF blocks too
/// when `counts_pdfs`), every image's longest edge must be at most
/// `max_edge_px`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ManyImages {
    pub above: usize,
    pub max_edge_px: u32,
    pub counts_pdfs: bool,
}

/// Context windows from this size up get the larger PDF page and image
/// allowances.
const LARGE_CONTEXT_WINDOW: u32 = 1_000_000;

fn limits_for(kind: ProviderKind, context_window: Option<u32>) -> PartLimits {
    // Claude reads 100 images and 100 PDF pages per request on smaller
    // context windows, 600 of each from 1M tokens up.
    let claude: u32 = if context_window.is_some_and(|w| w >= LARGE_CONTEXT_WINDOW) {
        600
    } else {
        100
    };
    match kind {
        ProviderKind::Anthropic => PartLimits {
            max_request_encoded_bytes: 30_000_000,
            // 10 MB of base64.
            max_image_bytes: 7_500_000,
            max_pdf_bytes: 22_500_000,
            max_image_edge_px: Some(8000),
            max_images_per_message: None,
            max_pdfs_per_message: None,
            max_pdf_pages_per_request: Some(claude),
            max_images_per_request: Some(claude as usize),
            many_images: Some(ManyImages {
                above: 20,
                max_edge_px: 2000,
                counts_pdfs: false,
            }),
            pdf_page_tokens: PDF_PAGE_TOKENS,
        },
        ProviderKind::Bedrock => PartLimits {
            max_request_encoded_bytes: 20_000_000,
            max_image_bytes: 3_750_000,
            max_pdf_bytes: 4_500_000,
            max_image_edge_px: Some(8000),
            max_images_per_message: Some(20),
            max_pdfs_per_message: Some(5),
            max_pdf_pages_per_request: Some(claude),
            max_images_per_request: Some(claude as usize),
            many_images: Some(ManyImages {
                above: 20,
                max_edge_px: 2000,
                counts_pdfs: true,
            }),
            pdf_page_tokens: PDF_PAGE_TOKENS,
        },
        ProviderKind::Gemini => PartLimits {
            // The whole request must stay under 20 MB; leave room for text.
            max_request_encoded_bytes: 18_000_000,
            max_image_bytes: 13_500_000,
            max_pdf_bytes: 13_500_000,
            max_image_edge_px: None,
            max_images_per_message: None,
            max_pdfs_per_message: None,
            max_pdf_pages_per_request: Some(1000),
            max_images_per_request: None,
            many_images: None,
            // 258 per page on Gemini 2.x, 560 at Gemini 3's default media
            // resolution.
            pdf_page_tokens: 560,
        },
        ProviderKind::OpenaiResponses
        | ProviderKind::OpenaiCompatible
        | ProviderKind::AzureOpenai
        | ProviderKind::LlamaCpp
        | ProviderKind::Fake => PartLimits {
            max_request_encoded_bytes: 32_000_000,
            max_image_bytes: 20_000_000,
            max_pdf_bytes: 24_000_000,
            max_image_edge_px: None,
            max_images_per_message: None,
            max_pdfs_per_message: None,
            max_pdf_pages_per_request: Some(100),
            max_images_per_request: None,
            many_images: None,
            pdf_page_tokens: PDF_PAGE_TOKENS,
        },
    }
}

/// Limits no stricter than either `a` or `b` (`None` is no limit).
fn looser(a: PartLimits, b: PartLimits) -> PartLimits {
    fn larger<T: Ord>(a: Option<T>, b: Option<T>) -> Option<T> {
        a.zip(b).map(|(a, b)| a.max(b))
    }
    PartLimits {
        max_request_encoded_bytes: a.max_request_encoded_bytes.max(b.max_request_encoded_bytes),
        max_image_bytes: a.max_image_bytes.max(b.max_image_bytes),
        max_pdf_bytes: a.max_pdf_bytes.max(b.max_pdf_bytes),
        max_image_edge_px: larger(a.max_image_edge_px, b.max_image_edge_px),
        max_images_per_message: larger(a.max_images_per_message, b.max_images_per_message),
        max_pdfs_per_message: larger(a.max_pdfs_per_message, b.max_pdfs_per_message),
        max_pdf_pages_per_request: larger(a.max_pdf_pages_per_request, b.max_pdf_pages_per_request),
        max_images_per_request: larger(a.max_images_per_request, b.max_images_per_request),
        many_images: a.many_images.zip(b.many_images).map(|(a, b)| ManyImages {
            above: a.above.max(b.above),
            max_edge_px: a.max_edge_px.max(b.max_edge_px),
            counts_pdfs: a.counts_pdfs && b.counts_pdfs,
        }),
        pdf_page_tokens: a.pdf_page_tokens.min(b.pdf_page_tokens),
    }
}

/// The loosest limits [`part_limits`] gives any model reached through a
/// `kind` account: every context window, and on OpenAI-compatible accounts
/// every family an OpenRouter model routes to ([`upstream_kind`]). The
/// adapters' last check uses these, so it never refuses a request the
/// harness built within the model's own limits.
fn protocol_limits(kind: ProviderKind) -> PartLimits {
    let families: &[ProviderKind] = if kind == ProviderKind::OpenaiCompatible {
        &[
            ProviderKind::OpenaiCompatible,
            ProviderKind::Anthropic,
            ProviderKind::Gemini,
        ]
    } else {
        &[kind]
    };
    families
        .iter()
        .flat_map(|&family| {
            [None, Some(LARGE_CONTEXT_WINDOW)].map(|window| limits_for(family, window))
        })
        .reduce(looser)
        .unwrap_or_else(|| limits_for(kind, None))
}

/// A gateway model id, lowercased, whose vendor prefix names the upstream.
/// OpenRouter's moving aliases (`~anthropic/claude-sonnet-latest`) are not
/// valid [`ModelId`](crate::ModelId)s, so discovery drops them today; the `~`
/// is stripped anyway so they route like the ids they follow if that changes.
fn routed_id(model: &ModelDescriptor) -> String {
    route_key(model.model.model_id.as_str())
}

fn route_key(id: &str) -> String {
    id.trim_start_matches('~').to_ascii_lowercase()
}

/// The protocol family whose limits apply upstream: an OpenRouter model id
/// names the vendor that finally serves it. [`protocol_limits`] lists every
/// family this can return.
fn upstream_kind(
    provider: &ProviderDescriptor,
    preset_id: Option<&str>,
    model: &ModelDescriptor,
) -> ProviderKind {
    if provider.kind == ProviderKind::OpenaiCompatible && preset_id == Some("openrouter") {
        let id = routed_id(model);
        if id.starts_with("anthropic/") {
            return ProviderKind::Anthropic;
        }
        if id.starts_with("google/gemini") {
            return ProviderKind::Gemini;
        }
    }
    provider.kind
}

/// A ChatGPT plan account (its access token is an `oauth.` vault secret).
fn chatgpt_plan(provider: &ProviderDescriptor) -> bool {
    matches!(&provider.auth, AuthConfig::VaultSecret { secret_name } if secret_name.starts_with("oauth."))
}

/// xAI reads only PNG and JPEG images, directly or through the OpenRouter
/// and Vercel gateways.
fn png_and_jpeg_only(
    provider: &ProviderDescriptor,
    preset_id: Option<&str>,
    model: &ModelDescriptor,
) -> bool {
    provider.kind == ProviderKind::OpenaiCompatible
        && match preset_id {
            Some("xai") => true,
            Some("openrouter") => routed_id(model).starts_with("x-ai/"),
            Some("vercel") => {
                let id = routed_id(model);
                id.starts_with("xai/") || id.starts_with("spacexai/")
            }
            _ => false,
        }
}

/// OpenAI families accept GIF only when it is not animated.
fn refuses_animated_gif(kind: ProviderKind) -> bool {
    matches!(
        kind,
        ProviderKind::OpenaiResponses
            | ProviderKind::OpenaiCompatible
            | ProviderKind::AzureOpenai
            | ProviderKind::LlamaCpp
    )
}

/// The vendor limits for `model` through `provider` (an account created
/// from preset `preset_id`).
pub fn part_limits(
    provider: &ProviderDescriptor,
    preset_id: Option<&str>,
    model: &ModelDescriptor,
) -> PartLimits {
    limits_for(
        upstream_kind(provider, preset_id, model),
        Some(model.context_window),
    )
}

/// Whether the model reads images: a chat model declaring `vision`. The
/// managed llama.cpp runtime starts without a vision projector, so it never
/// does.
pub fn accepts_images(
    provider: &ProviderDescriptor,
    _preset_id: Option<&str>,
    model: &ModelDescriptor,
) -> bool {
    model.capabilities.chat && model.capabilities.vision && provider.kind != ProviderKind::LlamaCpp
}

/// Whether the model reads PDFs: it reads images, and its protocol carries
/// documents. Anthropic, Gemini, Bedrock, OpenAI Responses with an API key,
/// and OpenAI-compatible accounts from the `openai-chat` preset, or from
/// `openrouter` for models whose upstream reads files natively (`anthropic/`,
/// `google/gemini`, `openai/`, or a `~` alias of one), so OpenRouter never
/// hands the document to a third-party parser. ChatGPT plan accounts, Azure, other compatible
/// servers, and the managed llama.cpp runtime do not.
pub fn accepts_pdf(
    provider: &ProviderDescriptor,
    preset_id: Option<&str>,
    model: &ModelDescriptor,
) -> bool {
    if !accepts_images(provider, preset_id, model) {
        return false;
    }
    match provider.kind {
        ProviderKind::Anthropic
        | ProviderKind::Gemini
        | ProviderKind::Bedrock
        | ProviderKind::Fake => true,
        ProviderKind::OpenaiResponses => !chatgpt_plan(provider),
        ProviderKind::OpenaiCompatible => match preset_id {
            Some("openai-chat") => true,
            Some("openrouter") => {
                let id = routed_id(model);
                ["anthropic/", "google/gemini", "openai/"]
                    .iter()
                    .any(|prefix| id.starts_with(prefix))
            }
            _ => false,
        },
        ProviderKind::AzureOpenai | ProviderKind::LlamaCpp => false,
    }
}

/// Whether the model reads images of this MIME type. Gemini has no GIF;
/// xAI takes only PNG and JPEG; OpenAI families take GIF only when it is
/// still, which needs the bytes ([`accepts_part`]).
pub fn accepts_image_mime(
    provider: &ProviderDescriptor,
    preset_id: Option<&str>,
    model: &ModelDescriptor,
    mime: &str,
) -> bool {
    accepts_images(provider, preset_id, model)
        && IMAGE_MIMES.contains(&mime)
        && !(mime == "image/gif"
            && upstream_kind(provider, preset_id, model) == ProviderKind::Gemini)
        && (matches!(mime, "image/png" | "image/jpeg")
            || !png_and_jpeg_only(provider, preset_id, model))
}

/// Whether this part can be sent to the model at all: its type is accepted
/// ([`accepts_image_mime`] or [`accepts_pdf`]), a GIF is still where the
/// vendor requires it, and the part is within the vendor's per-part size and
/// edge limits. Request-wide budgets (totals, counts, pages, the many-image
/// rule) are the caller's, through [`part_limits`].
pub fn accepts_part(
    provider: &ProviderDescriptor,
    preset_id: Option<&str>,
    model: &ModelDescriptor,
    part: &ContentPart,
) -> bool {
    let limits = part_limits(provider, preset_id, model);
    match part {
        ContentPart::Image { mime, data } => {
            accepts_image_mime(provider, preset_id, model, mime)
                && !(mime == "image/gif"
                    && refuses_animated_gif(provider.kind)
                    && is_animated_gif(data))
                && data.len() as u64 <= limits.max_image_bytes
                && limits.max_image_edge_px.is_none_or(|max| {
                    part.dimensions()
                        .is_none_or(|(width, height)| width.max(height) <= max)
                })
        }
        ContentPart::Pdf { data, .. } => {
            accepts_pdf(provider, preset_id, model) && data.len() as u64 <= limits.max_pdf_bytes
        }
    }
}

/// [`accepts_images`] and [`accepts_pdf`] together, as `model.list` reports
/// them.
pub fn model_input(
    provider: &ProviderDescriptor,
    preset_id: Option<&str>,
    model: &ModelDescriptor,
) -> ModelInput {
    ModelInput {
        images: accepts_images(provider, preset_id, model),
        pdf: accepts_pdf(provider, preset_id, model),
    }
}

/// The last check before a request with parts leaves for a vendor: the
/// limits every model of the protocol shares (raw sizes, edges and the
/// many-image rule for images whose header dimensions can be read, Gemini
/// GIF, OpenAI animated GIF, per-message counts, images and PDF pages per
/// request, and the encoded request total). Anything the caller should have
/// turned into a placeholder fails here as an invalid request instead of
/// being uploaded and rejected. The adapter knows only the protocol, so it
/// checks [`protocol_limits`], never stricter than [`part_limits`] for any
/// model of that protocol. Messages name the rule, never a file.
pub(crate) fn check_request(
    kind: ProviderKind,
    request: &ChatRequest,
) -> Result<(), ProviderError> {
    if request
        .messages
        .iter()
        .all(|message| message.parts.is_empty())
    {
        return Ok(());
    }
    let refuse = |rule: &str| Err(ProviderError::InvalidRequest(rule.to_owned()));
    let limits = protocol_limits(kind);
    let mut total: u64 = 0;
    let (mut request_images, mut pages) = (0usize, 0u32);
    // Blocks the many-image rule counts, and whether any image exceeds its
    // edge limit.
    let (mut many, mut beyond_many_edge) = (0usize, false);
    for message in &request.messages {
        let (mut images, mut pdfs) = (0usize, 0usize);
        for part in &message.parts {
            total = total.saturating_add(part.encoded_len() as u64);
            match part {
                ContentPart::Image { mime, data } => {
                    images += 1;
                    request_images += 1;
                    many += 1;
                    if data.len() as u64 > limits.max_image_bytes {
                        return refuse("an image is larger than this provider accepts");
                    }
                    let edge = part.dimensions().map(|(width, height)| width.max(height));
                    if edge
                        .zip(limits.max_image_edge_px)
                        .is_some_and(|(edge, max)| edge > max)
                    {
                        return refuse("an image is wider or taller than this provider accepts");
                    }
                    if edge
                        .zip(limits.many_images)
                        .is_some_and(|(edge, rule)| edge > rule.max_edge_px)
                    {
                        beyond_many_edge = true;
                    }
                    if mime == "image/gif" && kind == ProviderKind::Gemini {
                        return refuse("Gemini does not accept GIF images");
                    }
                    if mime == "image/gif" && refuses_animated_gif(kind) && is_animated_gif(data) {
                        return refuse("this provider does not accept animated GIF images");
                    }
                }
                ContentPart::Pdf {
                    data,
                    pages: part_pages,
                    ..
                } => {
                    pdfs += 1;
                    pages = pages.saturating_add((*part_pages).max(1));
                    if limits.many_images.is_some_and(|rule| rule.counts_pdfs) {
                        many += 1;
                    }
                    if data.len() as u64 > limits.max_pdf_bytes {
                        return refuse("a PDF is larger than this provider accepts");
                    }
                }
            }
        }
        if limits
            .max_images_per_message
            .is_some_and(|max| images > max)
            || limits.max_pdfs_per_message.is_some_and(|max| pdfs > max)
        {
            return refuse("a message carries more images or PDFs than this provider accepts");
        }
    }
    if beyond_many_edge && limits.many_images.is_some_and(|rule| many > rule.above) {
        return refuse(
            "a request with this many images accepts only images up to 2,000 pixels on a side",
        );
    }
    if limits
        .max_images_per_request
        .is_some_and(|max| request_images > max)
    {
        return refuse("the request carries more images than this provider accepts");
    }
    if limits
        .max_pdf_pages_per_request
        .is_some_and(|max| pages > max)
    {
        return refuse("the request's PDFs have more pages than this provider accepts");
    }
    if total > limits.max_request_encoded_bytes {
        return refuse("the request's images and PDFs are larger than this provider accepts");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parts::fixtures::{gif, pdf, png, webp};
    use crate::{Locality, ModelId, ModelRef, ProviderId};
    use ProviderKind::{
        Anthropic, AzureOpenai, Bedrock, Gemini, LlamaCpp, OpenaiCompatible, OpenaiResponses,
    };
    use serde_json::json;

    fn provider(kind: ProviderKind, secret_name: &str) -> ProviderDescriptor {
        ProviderDescriptor {
            id: ProviderId::new("acct").unwrap(),
            kind,
            base_url: None,
            locality: Locality::Remote,
            auth: AuthConfig::VaultSecret {
                secret_name: secret_name.into(),
            },
        }
    }

    fn account(kind: ProviderKind) -> ProviderDescriptor {
        provider(kind, "key")
    }

    fn model(id: &str, vision: bool, window: u32) -> ModelDescriptor {
        let mut model = ModelDescriptor::new(
            ModelRef::new(ProviderId::new("acct").unwrap(), ModelId::new(id).unwrap()),
            Locality::Remote,
            window,
        );
        model.capabilities.vision = vision;
        model
    }

    #[test]
    fn pdf_support_follows_protocol_preset_and_plan() {
        let vision = model("m", true, 128_000);
        for kind in [Anthropic, Gemini, Bedrock, OpenaiResponses] {
            assert!(accepts_pdf(&account(kind), None, &vision), "{kind:?}");
        }
        let plan = provider(OpenaiResponses, "oauth.acct.access");
        assert!(accepts_images(&plan, Some("openai"), &vision));
        assert!(!accepts_pdf(&plan, Some("openai"), &vision));
        let compatible = account(OpenaiCompatible);
        assert!(accepts_pdf(&compatible, Some("openai-chat"), &vision));
        for preset in [None, Some("xai"), Some("ollama"), Some("custom")] {
            assert!(!accepts_pdf(&compatible, preset, &vision), "{preset:?}");
            assert!(accepts_images(&compatible, preset, &vision), "{preset:?}");
        }
        for (id, reads) in [
            ("anthropic/claude-sonnet-4", true),
            ("google/gemini-2.5-pro", true),
            ("openai/gpt-4o", true),
            ("OpenAI/gpt-4o-mini", true),
            ("mistralai/pixtral-large-2411", false),
            ("meta-llama/llama-4-scout", false),
            ("x-ai/grok-4", false),
        ] {
            let routed = model(id, true, 128_000);
            assert_eq!(
                accepts_pdf(&compatible, Some("openrouter"), &routed),
                reads,
                "{id}"
            );
        }
        assert!(!accepts_pdf(&account(AzureOpenai), None, &vision));
        assert!(!accepts_pdf(
            &account(Anthropic),
            None,
            &model("m", false, 1)
        ));
        let llama = ProviderDescriptor {
            auth: AuthConfig::None,
            ..account(LlamaCpp)
        };
        assert!(!accepts_images(&llama, None, &vision));
        let mut embedder = vision.clone();
        embedder.capabilities.chat = false;
        assert!(!accepts_images(&account(Anthropic), None, &embedder));
        assert_eq!(
            model_input(&account(Gemini), None, &vision),
            ModelInput {
                images: true,
                pdf: true
            }
        );
        assert_eq!(
            model_input(&plan, Some("openai"), &vision),
            ModelInput {
                images: true,
                pdf: false
            }
        );
        assert_eq!(
            model_input(&compatible, None, &model("m", false, 1)),
            ModelInput::default()
        );
        assert_eq!(
            serde_json::to_value(model_input(&plan, None, &vision)).unwrap(),
            json!({"images": true, "pdf": false})
        );
    }

    #[test]
    fn gateway_ids_route_by_their_lowercase_vendor_prefix() {
        assert_eq!(route_key("X-AI/Grok-4"), "x-ai/grok-4");
        assert_eq!(route_key("~x-ai/grok-latest"), "x-ai/grok-latest");
        assert_eq!(
            route_key("~anthropic/claude-sonnet-latest"),
            "anthropic/claude-sonnet-latest"
        );
        // OpenRouter's `~` aliases are not model ids, so discovery drops them
        // and they never reach the prefix checks.
        assert!(ModelId::new("~x-ai/grok-latest").is_err());
    }

    #[test]
    fn image_types_sizes_and_edges_follow_each_family() {
        let vision = model("m", true, 128_000);
        let gemini = account(Gemini);
        assert!(accepts_image_mime(&gemini, None, &vision, "image/png"));
        assert!(accepts_image_mime(&gemini, None, &vision, "image/webp"));
        assert!(!accepts_image_mime(&gemini, None, &vision, "image/gif"));
        assert!(!accepts_image_mime(
            &account(Anthropic),
            None,
            &vision,
            "image/svg+xml"
        ));
        assert!(!accepts_image_mime(
            &account(Anthropic),
            None,
            &vision,
            "image/heic"
        ));
        let routed = model("google/gemini-2.5-flash", true, 1);
        assert!(!accepts_image_mime(
            &account(OpenaiCompatible),
            Some("openrouter"),
            &routed,
            "image/gif"
        ));
        assert!(accepts_image_mime(
            &account(OpenaiCompatible),
            Some("openrouter"),
            &routed,
            "image/webp"
        ));
        // xAI reads only PNG and JPEG, directly or through a gateway.
        let compatible = account(OpenaiCompatible);
        let grok = model("grok-4", true, 131_072);
        for mime in IMAGE_MIMES {
            let reads = matches!(mime, "image/png" | "image/jpeg");
            assert_eq!(
                accepts_image_mime(&compatible, Some("xai"), &grok, mime),
                reads,
                "{mime}"
            );
            for (preset, id) in [
                ("openrouter", "x-ai/grok-4"),
                ("openrouter", "X-AI/grok-4.3"),
                ("vercel", "xai/grok-4"),
                ("vercel", "spacexai/grok-4.3"),
            ] {
                assert_eq!(
                    accepts_image_mime(&compatible, Some(preset), &model(id, true, 1), mime),
                    reads,
                    "{preset} {id} {mime}"
                );
            }
            for preset in [None, Some("openai-chat"), Some("groq"), Some("vercel")] {
                assert!(
                    accepts_image_mime(&compatible, preset, &grok, mime),
                    "{preset:?} {mime}"
                );
            }
            for id in ["openai/gpt-4o", "anthropic/claude-sonnet-4"] {
                for preset in ["openrouter", "vercel"] {
                    assert!(
                        accepts_image_mime(&compatible, Some(preset), &model(id, true, 1), mime),
                        "{preset} {id} {mime}"
                    );
                }
            }
        }
        assert!(!accepts_part(
            &compatible,
            Some("xai"),
            &grok,
            &ContentPart::image("image/gif", gif(1))
        ));
        assert!(!accepts_part(
            &compatible,
            Some("xai"),
            &grok,
            &ContentPart::image("image/webp", webp(64, 48))
        ));
        assert!(accepts_part(
            &compatible,
            Some("xai"),
            &grok,
            &ContentPart::image("image/png", png(64, 48))
        ));

        let still = ContentPart::image("image/gif", gif(1));
        let animated = ContentPart::image("image/gif", gif(2));
        for kind in [OpenaiResponses, OpenaiCompatible, AzureOpenai] {
            assert!(
                accepts_part(&account(kind), None, &vision, &still),
                "{kind:?}"
            );
            assert!(
                !accepts_part(&account(kind), None, &vision, &animated),
                "{kind:?}"
            );
        }
        assert!(accepts_part(&account(Anthropic), None, &vision, &animated));
        assert!(accepts_part(&account(Bedrock), None, &vision, &animated));
        assert!(!accepts_part(&gemini, None, &vision, &still));

        let mut bytes = png(4, 4);
        bytes.resize(3_750_001, 0);
        let large = ContentPart::image("image/png", bytes);
        assert!(!accepts_part(&account(Bedrock), None, &vision, &large));
        assert!(accepts_part(&account(Anthropic), None, &vision, &large));
        let wide = ContentPart::image("image/png", png(8001, 10));
        assert!(!accepts_part(&account(Anthropic), None, &vision, &wide));
        assert!(!accepts_part(&account(Bedrock), None, &vision, &wide));
        assert!(accepts_part(&gemini, None, &vision, &wide));

        let document = ContentPart::pdf("a.pdf", pdf(), 1);
        assert!(accepts_part(&account(Bedrock), None, &vision, &document));
        assert!(!accepts_part(
            &account(AzureOpenai),
            None,
            &vision,
            &document
        ));
        let mut bytes = pdf();
        bytes.resize(4_500_001, b' ');
        let long = ContentPart::pdf("a.pdf", bytes, 1);
        assert!(!accepts_part(&account(Bedrock), None, &vision, &long));
        assert!(accepts_part(&account(Anthropic), None, &vision, &long));
        assert!(!accepts_part(
            &account(Anthropic),
            None,
            &model("m", false, 1),
            &still
        ));
    }

    #[test]
    fn requests_are_refused_for_edges_and_the_many_image_rule() {
        use crate::ChatMessage;
        let request = |messages: Vec<Vec<ContentPart>>| {
            ChatRequest::new(
                ModelId::new("m").unwrap(),
                messages
                    .into_iter()
                    .map(|parts| ChatMessage::user("x").with_parts(parts))
                    .collect(),
            )
        };
        let images =
            |count: usize, edge: u32| vec![ContentPart::image("image/png", png(edge, 10)); count];
        let check = |kind: ProviderKind, messages: Vec<Vec<ContentPart>>| {
            check_request(kind, &request(messages)).map_err(|error| match error {
                ProviderError::InvalidRequest(rule) => rule,
                other => panic!("{other:?}"),
            })
        };

        let wide = check(Anthropic, vec![images(1, 8001)]).unwrap_err();
        assert!(wide.contains("wider or taller"), "{wide}");
        assert!(check(Anthropic, vec![images(1, 8000)]).is_ok());
        assert!(check(Gemini, vec![images(1, 9000)]).is_ok());

        // More than 20 images across the request (history included) allow
        // only images up to 2,000 px.
        let many = check(Anthropic, vec![images(11, 2001), images(10, 100)]).unwrap_err();
        assert!(many.contains("2,000 pixels"), "{many}");
        assert!(check(Anthropic, vec![images(10, 2001), images(10, 100)]).is_ok());
        assert!(check(Anthropic, vec![images(15, 2000), images(15, 2000)]).is_ok());
        // Unreadable dimensions are left to the caller's budget.
        let unknown = ContentPart::image("image/jpeg", vec![0xFF, 0xD8, 0xFF, 0xDA]);
        assert!(check(Anthropic, vec![vec![unknown; 20], images(1, 100)]).is_ok());
        assert!(check(OpenaiResponses, vec![images(20, 4000), images(20, 4000)]).is_ok());

        // Bedrock counts documents toward the rule; Anthropic does not.
        let mixed = || {
            let mut parts = images(16, 2001);
            parts.extend((0..5).map(|i| ContentPart::pdf(format!("{i}.pdf"), pdf(), 1)));
            parts
        };
        assert!(check(Bedrock, vec![mixed()]).is_err());
        assert!(check(Anthropic, vec![mixed()]).is_ok());
    }

    #[test]
    fn limits_follow_the_family_and_context_window() {
        let anthropic = part_limits(&account(Anthropic), None, &model("m", true, 200_000));
        assert_eq!(anthropic.max_request_encoded_bytes, 30_000_000);
        assert_eq!(anthropic.max_image_bytes, 7_500_000);
        assert_eq!(anthropic.max_pdf_pages_per_request, Some(100));
        assert_eq!(
            anthropic.many_images,
            Some(ManyImages {
                above: 20,
                max_edge_px: 2000,
                counts_pdfs: false
            })
        );
        let large_window = model("m", true, 1_000_000);
        assert_eq!(
            part_limits(&account(Anthropic), None, &large_window).max_pdf_pages_per_request,
            Some(600)
        );
        let bedrock = part_limits(&account(Bedrock), None, &model("m", true, 200_000));
        assert_eq!(
            (
                bedrock.max_image_bytes,
                bedrock.max_pdf_bytes,
                bedrock.max_images_per_message,
                bedrock.max_pdfs_per_message
            ),
            (3_750_000, 4_500_000, Some(20), Some(5))
        );
        assert!(bedrock.many_images.is_some_and(|rule| rule.counts_pdfs));
        assert_eq!(
            part_limits(&account(Gemini), None, &model("m", true, 1)).max_request_encoded_bytes,
            18_000_000
        );
        let routed = model("anthropic/claude-sonnet-4", true, 200_000);
        assert_eq!(
            part_limits(&account(OpenaiCompatible), Some("openrouter"), &routed),
            anthropic
        );
        let openai = part_limits(&account(OpenaiResponses), None, &model("m", true, 1));
        assert_eq!(openai.max_request_encoded_bytes, 32_000_000);
        assert_eq!(openai.max_pdf_pages_per_request, Some(100));
        assert_eq!(openai.many_images, None);

        // Claude reads 100 images per request below a 1M window, 600 from
        // there up; the other families set no request-wide image count.
        for kind in [Anthropic, Bedrock] {
            let at = |window| part_limits(&account(kind), None, &model("m", true, window));
            assert_eq!(at(200_000).max_images_per_request, Some(100), "{kind:?}");
            assert_eq!(at(999_999).max_images_per_request, Some(100), "{kind:?}");
            assert_eq!(at(1_000_000).max_images_per_request, Some(600), "{kind:?}");
        }
        assert_eq!(openai.max_images_per_request, None);
        assert_eq!(
            part_limits(&account(Gemini), None, &model("m", true, 1)).max_images_per_request,
            None
        );

        // PDF pages cost 560 tokens on Gemini (OpenRouter's too) and
        // PDF_PAGE_TOKENS elsewhere.
        let gemini_routed = model("google/gemini-2.5-pro", true, 1_048_576);
        assert_eq!(
            part_limits(
                &account(OpenaiCompatible),
                Some("openrouter"),
                &gemini_routed
            )
            .pdf_page_tokens,
            560
        );
        assert_eq!(
            part_limits(&account(Gemini), None, &gemini_routed).pdf_page_tokens,
            560
        );
        for kind in [
            Anthropic,
            Bedrock,
            OpenaiResponses,
            OpenaiCompatible,
            AzureOpenai,
        ] {
            assert_eq!(
                part_limits(&account(kind), None, &model("m", true, 1)).pdf_page_tokens,
                PDF_PAGE_TOKENS,
                "{kind:?}"
            );
        }
    }

    /// Whether `backstop` admits everything `limits` does.
    fn no_stricter(backstop: &PartLimits, limits: &PartLimits) -> bool {
        fn admits<T: Ord>(backstop: Option<T>, limit: Option<T>) -> bool {
            match (backstop, limit) {
                (None, _) => true,
                (Some(_), None) => false,
                (Some(backstop), Some(limit)) => backstop >= limit,
            }
        }
        backstop.max_request_encoded_bytes >= limits.max_request_encoded_bytes
            && backstop.max_image_bytes >= limits.max_image_bytes
            && backstop.max_pdf_bytes >= limits.max_pdf_bytes
            && admits(backstop.max_image_edge_px, limits.max_image_edge_px)
            && admits(
                backstop.max_images_per_message,
                limits.max_images_per_message,
            )
            && admits(backstop.max_pdfs_per_message, limits.max_pdfs_per_message)
            && admits(
                backstop.max_pdf_pages_per_request,
                limits.max_pdf_pages_per_request,
            )
            && admits(
                backstop.max_images_per_request,
                limits.max_images_per_request,
            )
            && match (backstop.many_images, limits.many_images) {
                (None, _) => true,
                (Some(_), None) => false,
                (Some(backstop), Some(rule)) => {
                    backstop.above >= rule.above
                        && backstop.max_edge_px >= rule.max_edge_px
                        && (rule.counts_pdfs || !backstop.counts_pdfs)
                }
            }
    }

    #[test]
    fn adapter_checks_are_never_stricter_than_any_models_limits() {
        for kind in [
            Anthropic,
            Bedrock,
            Gemini,
            OpenaiResponses,
            OpenaiCompatible,
            AzureOpenai,
            LlamaCpp,
            ProviderKind::Fake,
        ] {
            let backstop = protocol_limits(kind);
            for preset in [
                None,
                Some("openrouter"),
                Some("openai-chat"),
                Some("xai"),
                Some("vercel"),
            ] {
                for id in [
                    "m",
                    "anthropic/claude-sonnet-4",
                    "google/gemini-2.5-pro",
                    "x-ai/grok-4",
                    "spacexai/grok-4.3",
                ] {
                    for window in [1, 200_000, 999_999, 1_000_000, 1_048_576, u32::MAX] {
                        let limits = part_limits(&account(kind), preset, &model(id, true, window));
                        assert!(
                            no_stricter(&backstop, &limits),
                            "{kind:?} {preset:?} {id} {window}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn requests_are_refused_beyond_the_largest_page_and_image_allowances() {
        use crate::ChatMessage;
        let request = |messages: Vec<Vec<ContentPart>>| {
            ChatRequest::new(
                ModelId::new("m").unwrap(),
                messages
                    .into_iter()
                    .map(|parts| ChatMessage::user("x").with_parts(parts))
                    .collect(),
            )
        };
        let pdfs = |count: usize, pages: u32| {
            (0..count)
                .map(|i| ContentPart::pdf(format!("{i}.pdf"), pdf(), pages))
                .collect::<Vec<_>>()
        };
        let images = |messages: usize, per_message: usize| {
            vec![vec![ContentPart::image("image/png", png(100, 10)); per_message]; messages]
        };
        let check = |kind: ProviderKind, messages: Vec<Vec<ContentPart>>| {
            check_request(kind, &request(messages)).map_err(|error| match error {
                ProviderError::InvalidRequest(rule) => rule,
                other => panic!("{other:?}"),
            })
        };

        // The harness gives a 1M-window Claude model 600 pages, so the
        // adapter, which does not know the window, allows that many.
        for kind in [Anthropic, Bedrock] {
            assert!(check(kind, vec![pdfs(2, 100)]).is_ok(), "{kind:?}");
            assert!(
                check(kind, vec![pdfs(5, 100), pdfs(1, 100)]).is_ok(),
                "{kind:?}"
            );
            let pages = check(kind, vec![pdfs(5, 100), pdfs(1, 101)]).unwrap_err();
            assert!(pages.contains("more pages"), "{kind:?} {pages}");
        }
        // OpenRouter routes `google/gemini*` models (1,000 pages) through an
        // OpenAI-compatible account.
        assert!(check(OpenaiCompatible, vec![pdfs(10, 100)]).is_ok());
        assert!(check(OpenaiCompatible, vec![pdfs(10, 100), pdfs(1, 1)]).is_err());
        assert!(check(OpenaiResponses, vec![pdfs(1, 100)]).is_ok());
        assert!(check(OpenaiResponses, vec![pdfs(1, 101)]).is_err());
        assert!(check(Gemini, vec![pdfs(10, 100)]).is_ok());

        // Claude images per request, history included: up to 600 (the
        // 1M-window allowance) leave; PDFs do not count toward it.
        assert!(check(Anthropic, images(30, 20)).is_ok());
        let mut over = images(30, 20);
        over.push(images(1, 1).remove(0));
        let count = check(Anthropic, over.clone()).unwrap_err();
        assert!(count.contains("more images"), "{count}");
        assert!(check(Bedrock, over.clone()).is_err());
        let mut documents = images(30, 20);
        documents.push(pdfs(1, 1));
        assert!(check(Anthropic, documents).is_ok());
        // No request-wide image count on the other families.
        for kind in [Gemini, OpenaiResponses, OpenaiCompatible] {
            assert!(check(kind, over.clone()).is_ok(), "{kind:?}");
        }
    }
}
