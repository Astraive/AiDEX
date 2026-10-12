//! Golden request bodies for image and PDF parts in every cloud protocol,
//! and the empty-content and role-alternation rules.

use std::collections::HashSet;

use base64::Engine as _;
use serde_json::{Value, json};

use super::{bedrock, requests};
use crate::parts::fixtures::{gif, jpeg, pdf, png, webp};
use crate::*;

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn model() -> ModelId {
    ModelId::new("fixture-model").unwrap()
}

fn call(id: &str) -> ToolCall {
    ToolCall {
        provider_metadata: None,
        id: id.into(),
        name: "lookup".into(),
        arguments: "{\"x\":1}".into(),
    }
}

fn with_calls(ids: &[&str]) -> ChatMessage {
    ChatMessage {
        tool_calls: ids.iter().map(|id| call(id)).collect(),
        ..ChatMessage::assistant("")
    }
}

fn body(kind: ProviderKind, messages: Vec<ChatMessage>) -> Value {
    let request = ChatRequest::new(model(), messages);
    request.validate().unwrap();
    requests::build(kind, &request).unwrap().1
}

fn refused(kind: ProviderKind, messages: Vec<ChatMessage>) -> String {
    let request = ChatRequest::new(model(), messages);
    match requests::build(kind, &request) {
        Err(ProviderError::InvalidRequest(message)) => message,
        other => panic!("expected an invalid request, got {other:?}"),
    }
}

/// Every message/content array is non-empty and roles alternate.
fn assert_well_formed(messages: &Value, content_key: &str) {
    let messages = messages.as_array().unwrap();
    assert!(!messages.is_empty());
    for pair in messages.windows(2) {
        assert_ne!(pair[0]["role"], pair[1]["role"], "roles must alternate");
    }
    for message in messages {
        let content = message[content_key].as_array().unwrap();
        assert!(!content.is_empty(), "empty {content_key}: {message}");
    }
}

fn image_and_pdf() -> Vec<ContentPart> {
    vec![
        ContentPart::image("image/png", png(4, 4)),
        ContentPart::pdf("Quarterly report.pdf", pdf(), 2),
    ]
}

#[test]
fn anthropic_places_parts_before_text_and_coalesces_turns() {
    let body = body(
        ProviderKind::Anthropic,
        vec![
            ChatMessage::system("be brief"),
            ChatMessage::user("what is this").with_parts(image_and_pdf()),
            // A failed run's placeholder between two user turns.
            ChatMessage::assistant(""),
            ChatMessage::user("and this"),
            with_calls(&["c1", "c2"]),
            ChatMessage::tool_result("c1", "one"),
            ChatMessage::tool_result("c2", "two"),
            ChatMessage::user("thanks"),
        ],
    );
    assert_eq!(
        body["messages"],
        json!([
            {"role":"user","content":[
                {"type":"image","source":{"type":"base64","media_type":"image/png","data":b64(&png(4, 4))}},
                {"type":"document","source":{"type":"base64","media_type":"application/pdf","data":b64(&pdf())}},
                {"type":"text","text":"what is this"},
                {"type":"text","text":"and this"}
            ]},
            {"role":"assistant","content":[
                {"type":"tool_use","id":"c1","name":"lookup","input":{"x":1}},
                {"type":"tool_use","id":"c2","name":"lookup","input":{"x":1}}
            ]},
            {"role":"user","content":[
                {"type":"tool_result","tool_use_id":"c1","content":"one"},
                {"type":"tool_result","tool_use_id":"c2","content":"two"},
                {"type":"text","text":"thanks"}
            ]}
        ])
    );
    assert_eq!(body["system"], json!([{"type":"text","text":"be brief"}]));
    assert_well_formed(&body["messages"], "content");
}

#[test]
fn anthropic_attachment_only_messages_and_trailing_placeholders() {
    let body = body(
        ProviderKind::Anthropic,
        vec![
            ChatMessage::user("").with_parts(vec![ContentPart::image("image/gif", gif(2))]),
            ChatMessage::assistant(""),
        ],
    );
    assert_eq!(
        body["messages"],
        json!([{"role":"user","content":[
            {"type":"image","source":{"type":"base64","media_type":"image/gif","data":b64(&gif(2))}}
        ]}])
    );
}

#[test]
fn gemini_inlines_data_before_text_and_coalesces_turns() {
    let body = body(
        ProviderKind::Gemini,
        vec![
            ChatMessage::system("sys"),
            // The workspace file index arrives as its own user message.
            ChatMessage::user("file index"),
            ChatMessage::user("compare").with_parts(vec![
                ContentPart::image("image/jpeg", jpeg(8, 8)),
                ContentPart::pdf("a.pdf", pdf(), 1),
            ]),
            ChatMessage::assistant(""),
            ChatMessage::user("again"),
            with_calls(&["c1"]),
            ChatMessage::tool_result("c1", "one"),
            ChatMessage::user("thanks"),
        ],
    );
    assert_eq!(
        body["contents"],
        json!([
            {"role":"user","parts":[
                {"text":"file index"},
                {"inlineData":{"mimeType":"image/jpeg","data":b64(&jpeg(8, 8))}},
                {"inlineData":{"mimeType":"application/pdf","data":b64(&pdf())}},
                {"text":"compare"},
                {"text":"again"}
            ]},
            {"role":"model","parts":[{"functionCall":{"id":"c1","name":"lookup","args":{"x":1}}}]},
            {"role":"user","parts":[
                {"functionResponse":{"id":"c1","name":"lookup","response":{"result":"one"}}},
                {"text":"thanks"}
            ]}
        ])
    );
    assert_eq!(body["systemInstruction"]["parts"][0]["text"], "sys");
    assert_well_formed(&body["contents"], "parts");
}

#[test]
fn gemini_refuses_gif_before_any_request() {
    let message = refused(
        ProviderKind::Gemini,
        vec![ChatMessage::user("x").with_parts(vec![ContentPart::image("image/gif", gif(1))])],
    );
    assert!(message.contains("GIF"), "{message}");
}

#[test]
fn bedrock_maps_parts_with_sanitized_unique_document_names() {
    let body = body(
        ProviderKind::Bedrock,
        vec![
            ChatMessage::system("sys"),
            ChatMessage::user("").with_parts(vec![
                ContentPart::image("image/jpeg", jpeg(2, 2)),
                ContentPart::pdf("my_file.v2.pdf", pdf(), 1),
                ContentPart::pdf("my file v2.pdf", pdf(), 1),
            ]),
            ChatMessage::assistant("seen"),
            ChatMessage::user("more").with_parts(vec![
                ContentPart::image("image/webp", webp(2, 2)),
                ContentPart::pdf("my-file-v2", pdf(), 1),
            ]),
        ],
    );
    let jpeg = b64(&jpeg(2, 2));
    let pdf = b64(&pdf());
    assert_eq!(
        body["messages"],
        json!([
            {"role":"user","content":[
                {"image":{"format":"jpeg","source":{"bytes":jpeg}}},
                {"document":{"format":"pdf","name":"my-file-v2","source":{"bytes":pdf}}},
                {"document":{"format":"pdf","name":"my file v2","source":{"bytes":pdf}}},
                {"text":"Attached document."}
            ]},
            {"role":"assistant","content":[{"text":"seen"}]},
            {"role":"user","content":[
                {"image":{"format":"webp","source":{"bytes":b64(&webp(2, 2))}}},
                {"document":{"format":"pdf","name":"my-file-v2 (2)","source":{"bytes":pdf}}},
                {"text":"more"}
            ]}
        ])
    );
    assert_well_formed(&body["messages"], "content");
}

#[test]
fn bedrock_coalesces_same_role_turns_with_tool_results_first() {
    // [user, assistant(''), user]
    let body1 = body(
        ProviderKind::Bedrock,
        vec![
            ChatMessage::user("a"),
            ChatMessage::assistant(""),
            ChatMessage::user("b"),
        ],
    );
    assert_eq!(
        body1["messages"],
        json!([{"role":"user","content":[{"text":"a"},{"text":"b"}]}])
    );
    // [assistant(tool_calls), tool, user]
    let body2 = body(
        ProviderKind::Bedrock,
        vec![
            ChatMessage::user("go"),
            with_calls(&["c1"]),
            ChatMessage::tool_result("c1", "one"),
            ChatMessage::user("next"),
        ],
    );
    assert_eq!(
        body2["messages"][2],
        json!({"role":"user","content":[
            {"toolResult":{"toolUseId":"c1","content":[{"text":"one"}]}},
            {"text":"next"}
        ]})
    );
    // [file-index user, transcript user]
    let body3 = body(
        ProviderKind::Bedrock,
        vec![
            ChatMessage::user("file index"),
            ChatMessage::user("question"),
        ],
    );
    assert_eq!(
        body3["messages"],
        json!([{"role":"user","content":[{"text":"file index"},{"text":"question"}]}])
    );
    for body in [&body1, &body2, &body3] {
        assert_well_formed(&body["messages"], "content");
    }
}

#[test]
fn bedrock_document_names_follow_the_converse_character_rules() {
    let mut used = HashSet::new();
    let mut name = |raw: &str| bedrock::document_name(raw, &mut used);
    assert_eq!(name("my_file.v2.pdf"), "my-file-v2");
    assert_eq!(name("a  b\t\tc.pdf"), "a b c");
    assert_eq!(name("a  b\t\tc.PDF"), "a b c (2)");
    assert_eq!(name("a b c"), "a b c (3)");
    assert_eq!(name("Report (final) [v2].pdf"), "Report (final) [v2]");
    assert_eq!(name("___.pdf"), "document");
    assert_eq!(name(".pdf"), "pdf");
    assert_eq!(name("   "), "document (2)");
    assert_eq!(name("résumé – draft.pdf"), "r-sum- - draft");
    let long = name(&"x".repeat(300));
    assert_eq!(long.len(), 200);
    let again = name(&"x".repeat(300));
    assert_eq!(again.len(), 200);
    assert!(again.ends_with(" (2)"));
    for name in &used {
        assert!(!name.is_empty() && name.len() <= 200, "{name}");
        assert!(!name.contains("  "), "{name}");
        assert!(
            name.chars().all(
                |c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '(' | ')' | '[' | ']')
            ),
            "{name}"
        );
    }
}

#[test]
fn bedrock_and_shared_limits_refuse_before_sending() {
    let six_pdfs: Vec<ContentPart> = (0..6)
        .map(|i| ContentPart::pdf(format!("{i}.pdf"), pdf(), 1))
        .collect();
    let message = refused(
        ProviderKind::Bedrock,
        vec![ChatMessage::user("x").with_parts(six_pdfs)],
    );
    assert!(message.contains("more images or PDFs"), "{message}");
    // Two messages of three PDFs each coalesce into one six-document turn.
    let three = || {
        (0..3)
            .map(|i| ContentPart::pdf(format!("{i}.pdf"), pdf(), 1))
            .collect::<Vec<_>>()
    };
    let message = refused(
        ProviderKind::Bedrock,
        vec![
            ChatMessage::user("a").with_parts(three()),
            ChatMessage::user("b").with_parts(three()),
        ],
    );
    assert!(message.contains("5 documents"), "{message}");
    let mut big = png(4, 4);
    big.resize(3_750_001, 0);
    let message = refused(
        ProviderKind::Bedrock,
        vec![ChatMessage::user("x").with_parts(vec![ContentPart::image("image/png", big.clone())])],
    );
    assert!(message.contains("larger"), "{message}");
    assert!(!message.contains("x.png"));
    // The same image fits Anthropic's per-image limit.
    body(
        ProviderKind::Anthropic,
        vec![ChatMessage::user("x").with_parts(vec![ContentPart::image("image/png", big)])],
    );
    // OpenAI families take still GIFs only.
    for kind in [ProviderKind::OpenaiResponses, ProviderKind::AzureOpenai] {
        body(
            kind,
            vec![ChatMessage::user("x").with_parts(vec![ContentPart::image("image/gif", gif(1))])],
        );
        let message = refused(
            kind,
            vec![ChatMessage::user("x").with_parts(vec![ContentPart::image("image/gif", gif(3))])],
        );
        assert!(message.contains("animated"), "{message}");
    }
}

#[test]
fn responses_keep_attachment_only_user_messages_and_skip_placeholders() {
    let body = body(
        ProviderKind::OpenaiResponses,
        vec![
            ChatMessage::system("sys"),
            ChatMessage::user("").with_parts(vec![
                ContentPart::image("image/png", png(4, 4)),
                ContentPart::pdf("notes", pdf(), 1),
            ]),
            ChatMessage::assistant(""),
            ChatMessage::user("with text").with_parts(vec![ContentPart::pdf("b.PDF", pdf(), 1)]),
        ],
    );
    let png_url = format!("data:image/png;base64,{}", b64(&png(4, 4)));
    let pdf_url = format!("data:application/pdf;base64,{}", b64(&pdf()));
    assert_eq!(
        body["input"],
        json!([
            {"role":"developer","content":"sys"},
            {"role":"user","content":[
                {"type":"input_image","image_url":png_url,"detail":"auto"},
                {"type":"input_file","filename":"notes.pdf","file_data":pdf_url}
            ]},
            {"role":"user","content":[
                {"type":"input_text","text":"with text"},
                {"type":"input_file","filename":"b.PDF","file_data":pdf_url}
            ]}
        ])
    );
}

#[test]
fn azure_uses_chat_completion_content_parts() {
    let body = body(
        ProviderKind::AzureOpenai,
        vec![
            ChatMessage::user("see").with_parts(vec![ContentPart::image("image/jpeg", jpeg(2, 2))]),
        ],
    );
    assert_eq!(
        body["messages"][0]["content"],
        json!([
            {"type":"text","text":"see"},
            {"type":"image_url","image_url":{"url":format!("data:image/jpeg;base64,{}", b64(&jpeg(2, 2)))}}
        ])
    );
}

#[test]
fn text_only_requests_keep_their_wire_form() {
    let messages = || {
        vec![
            ChatMessage::system("sys"),
            ChatMessage::user("hello"),
            ChatMessage::assistant("hi"),
            ChatMessage::user("bye"),
        ]
    };
    assert_eq!(
        body(ProviderKind::Anthropic, messages())["messages"],
        json!([
            {"role":"user","content":[{"type":"text","text":"hello"}]},
            {"role":"assistant","content":[{"type":"text","text":"hi"}]},
            {"role":"user","content":[{"type":"text","text":"bye"}]}
        ])
    );
    assert_eq!(
        body(ProviderKind::Gemini, messages())["contents"],
        json!([
            {"role":"user","parts":[{"text":"hello"}]},
            {"role":"model","parts":[{"text":"hi"}]},
            {"role":"user","parts":[{"text":"bye"}]}
        ])
    );
    assert_eq!(
        body(ProviderKind::Bedrock, messages())["messages"],
        json!([
            {"role":"user","content":[{"text":"hello"}]},
            {"role":"assistant","content":[{"text":"hi"}]},
            {"role":"user","content":[{"text":"bye"}]}
        ])
    );
    assert_eq!(
        body(ProviderKind::OpenaiResponses, messages())["input"],
        json!([
            {"role":"developer","content":"sys"},
            {"role":"user","content":"hello"},
            {"role":"assistant","content":"hi"},
            {"role":"user","content":"bye"}
        ])
    );
    let azure = body(ProviderKind::AzureOpenai, messages());
    assert_eq!(
        azure["messages"][1],
        json!({"role":"user","content":"hello"})
    );
}

#[test]
fn blank_messages_without_parts_never_become_empty_arrays() {
    let messages = || {
        vec![
            ChatMessage::user(""),
            ChatMessage::user("question"),
            ChatMessage::assistant(""),
        ]
    };
    for (kind, key, content) in [
        (ProviderKind::Anthropic, "messages", "content"),
        (ProviderKind::Gemini, "contents", "parts"),
        (ProviderKind::Bedrock, "messages", "content"),
    ] {
        let body = body(kind, messages());
        assert_eq!(body[key].as_array().unwrap().len(), 1, "{kind:?}: {body}");
        assert_well_formed(&body[key], content);
    }
}
