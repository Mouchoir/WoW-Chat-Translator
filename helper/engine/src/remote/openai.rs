//! OpenAI-compatible `POST {base}/chat/completions` (developers.openai.com API reference, read
//! 2026-10-20). Works with OpenAI, OpenRouter, Mistral, Groq, DeepSeek, LM Studio and Ollama:
//! the body holds only fields every one of them accepts.
//!
//! - `Authorization: Bearer <key>` (no header without a key: a loopback server).
//! - `{"model", "messages": [system, user], "stream": false}`. No `temperature` (reasoning models
//!   accept only the default). An output limit of `OUTPUT_LIMIT` tokens (as Claude and Gemini;
//!   reasoning tokens count toward it), so a chat line written to make the model ramble cannot
//!   run up the player's bill: `max_completion_tokens` on OpenAI's own host (its reasoning
//!   models reject the deprecated `max_tokens`), `max_tokens` elsewhere (OpenRouter, Mistral,
//!   Groq, DeepSeek, LM Studio and Ollama all read it).
//! - `reasoning_effort` only on OpenAI's own host for its reasoning models: `none` for
//!   gpt-6-luna (the only one that accepts it), `low` for the others (gpt-6.1-sol accepts neither
//!   `none` nor `minimal`).
//! - Answer: `choices[0].message.content`; `message.refusal` or `finish_reason`
//!   `content_filter` is a refusal, `length` a truncation.
//! - Errors: `{"error": {"message", "type", "code"}}`; a 429 with code `insufficient_quota` is
//!   not retried.

use serde_json::{json, Value};

use super::{Failure, FallbackKind, HttpRequest, Message, Parsed, RemoteConfig};

/// The `reasoning_effort` sent, if any.
pub fn reasoning_effort(cfg: &RemoteConfig) -> Option<&'static str> {
    if cfg.endpoint.host != "api.openai.com" {
        return None;
    }
    let m = cfg.model.as_str();
    if m.starts_with("gpt-6-luna") {
        Some("none")
    } else if m.starts_with("gpt-6") || m.starts_with("gpt-5") || m.starts_with("o1") || m.starts_with("o3") || m.starts_with("o4") {
        Some("low")
    } else {
        None
    }
}

/// The most output tokens (reasoning included) one chat line may cost.
pub const OUTPUT_LIMIT: u32 = 4000;

pub fn request(cfg: &RemoteConfig, msg: &Message) -> HttpRequest {
    let mut body = json!({
        "model": cfg.model,
        "messages": [
            {"role": "system", "content": msg.system},
            {"role": "user", "content": msg.user},
        ],
        "stream": false,
    });
    let limit_field = if cfg.endpoint.host == "api.openai.com" { "max_completion_tokens" } else { "max_tokens" };
    body[limit_field] = json!(OUTPUT_LIMIT);
    if let Some(e) = reasoning_effort(cfg) {
        body["reasoning_effort"] = json!(e);
    }
    let mut headers = Vec::new();
    if !cfg.key.is_empty() {
        headers.push(("Authorization", format!("Bearer {}", cfg.key.expose())));
    }
    HttpRequest { url: cfg.endpoint.clone(), headers, body: serde_json::to_vec(&body).unwrap_or_default() }
}

/// A 429 that means "no credit left" rather than "too fast".
pub fn is_quota(body: &[u8]) -> bool {
    serde_json::from_slice::<Value>(body).ok().and_then(|v| v.pointer("/error/code").and_then(Value::as_str).map(|c| c == "insufficient_quota")).unwrap_or(false)
}

pub fn parse(body: &[u8]) -> Parsed {
    let Ok(v) = serde_json::from_slice::<Value>(body) else { return Parsed::Fail(Failure::new(FallbackKind::BadAnswer, "not JSON")) };
    let Some(choice) = v.pointer("/choices/0") else { return Parsed::Fail(Failure::new(FallbackKind::BadAnswer, "no choice in the answer")) };
    if let Some(r) = choice.pointer("/message/refusal").and_then(Value::as_str).filter(|r| !r.is_empty()) {
        return Parsed::Fail(Failure::new(FallbackKind::Refused, r));
    }
    match choice.get("finish_reason").and_then(Value::as_str) {
        Some("content_filter") => return Parsed::Fail(Failure::new(FallbackKind::Refused, "content filter")),
        Some("length") => return Parsed::Fail(Failure::new(FallbackKind::Truncated, "")),
        _ => {}
    }
    match choice.pointer("/message/content").and_then(Value::as_str) {
        Some(t) if !t.trim().is_empty() => Parsed::Text(t.to_string()),
        _ => Parsed::Fail(Failure::new(FallbackKind::BadAnswer, "empty answer")),
    }
}
