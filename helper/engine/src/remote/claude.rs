//! Anthropic Claude `POST https://api.anthropic.com/v1/messages` (facts from Anthropic's API
//! reference given by the lead for this session, 2026-10-20).
//!
//! - Headers `x-api-key`, `anthropic-version: 2023-06-01`; for the models with a server-side
//!   refusal fallback also `anthropic-beta: server-side-fallback-2026-07-01` and the body field
//!   `"fallbacks": "default"`.
//! - Body `{model, max_tokens: 4000, system, messages: [{role: user, content}], output_config:
//!   {effort: low}}`. No `temperature`, `top_p` or `top_k` (removed on Opus 5.5, a 400 for
//!   non-default values on Sonnet 5.5 and Haiku 5.5), no `thinking` (Opus 5.5 cannot disable
//!   it; effort `low` keeps it short), no assistant prefill (a 400).
//! - Answer: `stop_reason` first (`refusal`: refused, `max_tokens`: truncated), then the
//!   `content[]` blocks of type `text` only (thinking blocks may come with empty text).
//! - Errors: `{"type": "error", "error": {"type", "message"}}`: 400, 401, 403, 404 (unknown
//!   model), 413, 429 and 529 (retryable, `retry-after`), 500.

use serde_json::{json, Value};

use super::{Failure, FallbackKind, HttpRequest, Message, Parsed, RemoteConfig};

/// Models that take `output_config.effort` (the 4.6 generation and later; Haiku 4.5 and older
/// reject it).
pub fn takes_effort(model: &str) -> bool {
    const P: [&str; 9] = [
        "claude-opus-5",
        "claude-sonnet-5",
        "claude-haiku-5",
        "claude-fable-5",
        "claude-mythos-5",
        "claude-opus-4-6",
        "claude-opus-4-7",
        "claude-opus-4-8",
        "claude-sonnet-4-6",
    ];
    P.iter().any(|p| model.starts_with(p))
}

/// Models with the server-side refusal fallback (`fallbacks: "default"`); Haiku 5.5 has none
/// (the parameter would change nothing or be a 400).
pub fn takes_fallbacks(model: &str) -> bool {
    ["claude-opus-5-5", "claude-sonnet-5-5", "claude-opus-5", "claude-fable-5-1"].contains(&model)
}

pub fn request(cfg: &RemoteConfig, msg: &Message) -> HttpRequest {
    let mut body = json!({
        "model": cfg.model,
        "max_tokens": 4000,
        "system": msg.system,
        "messages": [{"role": "user", "content": msg.user}],
    });
    if takes_effort(&cfg.model) {
        body["output_config"] = json!({"effort": "low"});
    }
    let mut headers = vec![("x-api-key", cfg.key.expose().to_string()), ("anthropic-version", "2023-06-01".to_string())];
    if takes_fallbacks(&cfg.model) {
        body["fallbacks"] = json!("default");
        headers.push(("anthropic-beta", "server-side-fallback-2026-07-01".to_string()));
    }
    HttpRequest { url: cfg.endpoint.clone(), headers, body: serde_json::to_vec(&body).unwrap_or_default() }
}

pub fn parse(body: &[u8]) -> Parsed {
    let Ok(v) = serde_json::from_slice::<Value>(body) else { return Parsed::Fail(Failure::new(FallbackKind::BadAnswer, "not JSON")) };
    match v.get("stop_reason").and_then(Value::as_str) {
        Some("refusal") => {
            let cat = v.pointer("/stop_details/category").and_then(Value::as_str).unwrap_or("");
            return Parsed::Fail(Failure::new(FallbackKind::Refused, cat));
        }
        Some("max_tokens") => return Parsed::Fail(Failure::new(FallbackKind::Truncated, "")),
        _ => {}
    }
    let text: String = v
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| blocks.iter().filter(|b| b.get("type").and_then(Value::as_str) == Some("text")).filter_map(|b| b.get("text").and_then(Value::as_str)).collect())
        .unwrap_or_default();
    if text.trim().is_empty() {
        return Parsed::Fail(Failure::new(FallbackKind::BadAnswer, "empty answer"));
    }
    Parsed::Text(text)
}
