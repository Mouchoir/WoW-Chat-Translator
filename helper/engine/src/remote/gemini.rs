//! Google Gemini `POST /v1beta/models/{model}:generateContent` (ai.google.dev API reference and
//! the thinking guide, read 2026-10-20).
//!
//! - `x-goog-api-key: <key>` (never the `?key=` query form: URLs end up in proxy logs).
//! - `systemInstruction.parts[].text` (system), `contents[{role: user, parts[{text}]}]` (user).
//! - `generationConfig.maxOutputTokens` 4000 (thinking tokens count toward it) and
//!   `thinkingConfig.thinkingLevel` only for the models the thinking guide's table lists (read
//!   again 2026-10-21): `minimal` where it is supported (3.6 Flash, 3.5 Flash-Lite, 3 Flash,
//!   3.1 Flash-Lite Image), `low` on the others (4 Argon, 3.8 Flash, 3.1 Pro, 3 Pro, 2.5 Pro,
//!   2.5 Flash). Any other model (3.1 Flash-Lite, 2.5 Flash-Lite, a typed name) gets no
//!   `thinkingConfig`: its default applies and no undocumented level can turn every request into
//!   a 400. No temperature (the Gemini 3 guidance keeps the default).
//! - Safety thresholds `BLOCK_ONLY_HIGH`: chat banter is translated, not generated.
//! - Answer: `candidates[0].content.parts[]`, parts marked `thought: true` skipped. `finishReason`
//!   (API reference, read 2026-10-21): `MAX_TOKENS` and `CONTINUATION` are a truncation; the
//!   content filters (`SAFETY`, `RECITATION`, `LANGUAGE`, `BLOCKLIST`, `PROHIBITED_CONTENT`,
//!   `SPII`, `IMAGE_*`, `ESCALATION`, `PUP_LIMITED_DISABLED`) and `promptFeedback.blockReason`
//!   a refusal; anything else (`OTHER`, `MALFORMED_RESPONSE`, `MISSING_THOUGHT_SIGNATURE`, the
//!   tool-call reasons...) an unusable answer, with the reason's name.
//! - Errors: `{"error": {"code", "message", "status"}}`.

use serde_json::{json, Value};

use super::{Failure, FallbackKind, HttpRequest, Message, Parsed, RemoteConfig};

/// The thinking guide's table: model, then the lowest level it supports. Checked in order, a model
/// matching its name exactly or followed by `-` (a version suffix such as `-preview` or `-001`);
/// the Lite and Image variants come before the names they extend. `None`: no level is sent.
const THINKING: [(&str, Option<&str>); 13] = [
    ("gemini-2.5-flash-lite", None),
    ("gemini-2.5-flash", Some("low")),
    ("gemini-2.5-pro", Some("low")),
    ("gemini-3.1-flash-lite-image", Some("minimal")),
    ("gemini-3.1-flash-lite", None),
    ("gemini-3.1-pro", Some("low")),
    ("gemini-3.5-flash-lite", Some("minimal")),
    ("gemini-3.6-flash", Some("minimal")),
    ("gemini-3.8-flash", Some("low")),
    ("gemini-3-flash", Some("minimal")),
    ("gemini-3-pro", Some("low")),
    ("gemini-4-argon", Some("low")),
    ("gemini-robotics-er-2", Some("minimal")),
];

pub fn thinking_config(model: &str) -> Option<Value> {
    let hit = THINKING.iter().find(|(name, _)| model == *name || model.strip_prefix(name).is_some_and(|rest| rest.starts_with('-')));
    hit.and_then(|(_, level)| *level).map(|level| json!({"thinkingLevel": level}))
}

/// What a `finishReason` other than `STOP` means.
fn finish_failure(reason: &str) -> Failure {
    match reason {
        "MAX_TOKENS" | "CONTINUATION" => Failure::new(FallbackKind::Truncated, ""),
        "SAFETY" | "RECITATION" | "LANGUAGE" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" | "ESCALATION" | "PUP_LIMITED_DISABLED" => Failure::new(FallbackKind::Refused, reason),
        r if r.starts_with("IMAGE_") => Failure::new(FallbackKind::Refused, r),
        r => Failure::new(FallbackKind::BadAnswer, r),
    }
}

pub fn request(cfg: &RemoteConfig, msg: &Message) -> HttpRequest {
    let mut gen = json!({"maxOutputTokens": 4000});
    if let Some(t) = thinking_config(&cfg.model) {
        gen["thinkingConfig"] = t;
    }
    let safety: Vec<Value> = ["HARM_CATEGORY_HARASSMENT", "HARM_CATEGORY_HATE_SPEECH", "HARM_CATEGORY_SEXUALLY_EXPLICIT", "HARM_CATEGORY_DANGEROUS_CONTENT"]
        .iter()
        .map(|c| json!({"category": c, "threshold": "BLOCK_ONLY_HIGH"}))
        .collect();
    let body = json!({
        "systemInstruction": {"parts": [{"text": msg.system}]},
        "contents": [{"role": "user", "parts": [{"text": msg.user}]}],
        "generationConfig": gen,
        "safetySettings": safety,
    });
    HttpRequest {
        url: cfg.endpoint.clone(),
        headers: vec![("x-goog-api-key", cfg.key.expose().to_string())],
        body: serde_json::to_vec(&body).unwrap_or_default(),
    }
}

pub fn parse(body: &[u8]) -> Parsed {
    let Ok(v) = serde_json::from_slice::<Value>(body) else { return Parsed::Fail(Failure::new(FallbackKind::BadAnswer, "not JSON")) };
    let Some(cand) = v.pointer("/candidates/0") else {
        let reason = v.pointer("/promptFeedback/blockReason").and_then(Value::as_str);
        return Parsed::Fail(match reason {
            Some(r) => Failure::new(FallbackKind::Refused, r),
            None => Failure::new(FallbackKind::BadAnswer, "no candidate in the answer"),
        });
    };
    match cand.get("finishReason").and_then(Value::as_str) {
        None | Some("STOP") | Some("FINISH_REASON_UNSPECIFIED") => {}
        Some(r) => return Parsed::Fail(finish_failure(r)),
    }
    let text: String = cand
        .pointer("/content/parts")
        .and_then(Value::as_array)
        .map(|parts| parts.iter().filter(|p| p.get("thought").and_then(Value::as_bool) != Some(true)).filter_map(|p| p.get("text").and_then(Value::as_str)).collect())
        .unwrap_or_default();
    if text.trim().is_empty() {
        return Parsed::Fail(Failure::new(FallbackKind::BadAnswer, "empty answer"));
    }
    Parsed::Text(text)
}
