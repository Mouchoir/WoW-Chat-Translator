//! Provider tests with the fake transport: request shapes, answers, every HTTP error class,
//! timeouts, refusals, placeholders through the shared pipeline, and no key in any message.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::fake::{FakeReply, FakeTransport};
use super::*;
use crate::pipeline::{self, Finish, Prep};

const KEY: &str = "TPKEYSENTINEL-9f8e7d6c5b4a39281706";
/// A word of the chat line: must never reach a fallback message or a log.
const NAME: &str = "Zorbulax";
const MARKED: &str = "wts \u{2}\u{4}0070DD[Necromancer Leggings]\u{3} cheap, add Kiwi48213 Zorbulax";

fn cfg(p: Provider) -> RemoteConfig {
    RemoteConfig::new(p, "", "", Secret::new(KEY), 0).unwrap()
}

fn cfg_model(p: Provider, model: &str) -> RemoteConfig {
    RemoteConfig::new(p, model, "", Secret::new(KEY), 0).unwrap()
}

fn prep(marked: &str, pairs: &[(&str, &str)]) -> Prepared {
    match pipeline::prepare("fr", marked, pairs, crate::prompt::DEFAULT) {
        Prep::Ready(p) => p,
        _ => panic!("expected Ready"),
    }
}

fn fast() -> Policy {
    Policy { connect: Duration::from_millis(500), total: Duration::from_millis(1500), retry_wait: Duration::from_millis(5), retry_floor: Duration::from_millis(100) }
}

/// A successful answer in each provider's shape.
fn answer(p: Provider, text: &str) -> FakeReply {
    FakeReply::ok(match p {
        Provider::DeepL => {
            let xml = deepl::to_xml(text, Variant::Placeholders);
            json!({"translations": [{"detected_source_language": "EN", "text": xml}]})
        }
        Provider::OpenAi => json!({"id": "x", "choices": [{"index": 0, "message": {"role": "assistant", "content": text}, "finish_reason": "stop"}]}),
        Provider::Gemini => json!({"candidates": [{"content": {"role": "model", "parts": [{"text": "pondering", "thought": true}, {"text": text}]}, "finishReason": "STOP"}]}),
        Provider::Claude => json!({"type": "message", "content": [{"type": "thinking", "thinking": ""}, {"type": "text", "text": text}], "stop_reason": "end_turn"}),
    })
}

/// An error body in each provider's shape (the OpenAI one echoes a masked key, as OpenAI does).
fn error_body(p: Provider, message: &str) -> Value {
    match p {
        Provider::DeepL => json!({"message": message}),
        Provider::OpenAi => json!({"error": {"message": format!("{message} Incorrect API key provided: TPKEYS*****1706."), "type": "invalid_request_error", "code": "invalid_api_key"}}),
        Provider::Gemini => json!({"error": {"code": 400, "message": message, "status": "INVALID_ARGUMENT"}}),
        Provider::Claude => json!({"type": "error", "error": {"type": "authentication_error", "message": message}}),
    }
}

fn run(c: &RemoteConfig, p: &Prepared, t: &FakeTransport) -> Result<Answer, CallError> {
    call(c, &Message::of(p), t, fast())
}

fn no_secret(f: &Fallback) {
    assert!(!f.message.contains(KEY) && !f.message.contains("TPKEYS") && !f.message.contains("1706"), "{}", f.message);
    assert!(!f.message.contains(NAME));
    let dbg = format!("{f:?}");
    assert!(!dbg.contains(KEY) && !dbg.contains(&f.message) || f.message.is_empty(), "Debug never prints the message: {dbg}");
}

// ------------------------------------------------------------------ request shapes

#[test]
fn deepl_request_shape() {
    let p = prep(MARKED, &[("wts", "vend"), ("cheap", "pas cher")]);
    let t = FakeTransport::new(vec![answer(Provider::DeepL, "Vends {1} pas cher, ajoute {2} Zorbulax")]);
    run(&cfg(Provider::DeepL), &p, &t).unwrap();
    t.with_seen(|s| {
        let s = &s[0];
        assert_eq!((s.url.host.as_str(), s.url.path.as_str(), s.url.https), ("api.deepl.com", "/v2/translate", true));
        assert_eq!(s.header("Authorization"), Some(format!("DeepL-Auth-Key {KEY}").as_str()));
        let b = &s.body;
        // "cheap" was expanded into the message (V4); "wts" is keep-allowed and listed
        assert_eq!(b["text"], json!(["wts <x>{1}</x> pas cher, add <x>{2}</x> Zorbulax"]));
        assert_eq!(b["target_lang"], "FR");
        assert_eq!(b["context"], "World of Warcraft chat. Glossary: \"wts\" = \"vend\".");
        assert_eq!(b["tag_handling"], "xml");
        assert_eq!(b["ignore_tags"], json!(["x"]));
        assert_eq!(b["formality"], "prefer_less");
        assert_eq!(b["preserve_formatting"], true);
        assert!(b.get("model_type").is_none(), "the service's default model");
        assert!(s.timeouts.total <= fast().total && s.timeouts.total > fast().total - Duration::from_millis(300));
    });
    // a free key goes to api-free; a model is DeepL's model_type
    let free = RemoteConfig::new(Provider::DeepL, "latency_optimized", "", Secret::new("abc-123:fx"), 0).unwrap();
    assert_eq!(free.endpoint.host, "api-free.deepl.com");
    let t = FakeTransport::new(vec![answer(Provider::DeepL, "x")]);
    run(&free, &p, &t).unwrap();
    t.with_seen(|s| assert_eq!(s[0].body["model_type"], "latency_optimized"));
}

#[test]
fn openai_request_shape() {
    let p = prep(MARKED, &[("wts", "vend")]);
    let c = cfg(Provider::OpenAi);
    assert_eq!(c.endpoint, Url::parse("https://api.openai.com/v1/chat/completions").unwrap());
    assert_eq!(c.model, "gpt-6-luna");
    let t = FakeTransport::new(vec![answer(Provider::OpenAi, "Vends {1} pas cher")]);
    run(&c, &p, &t).unwrap();
    t.with_seen(|s| {
        let s = &s[0];
        assert_eq!(s.header("Authorization"), Some(format!("Bearer {KEY}").as_str()));
        let b = &s.body;
        assert_eq!(b["model"], "gpt-6-luna");
        assert_eq!(b["messages"][0], json!({"role": "system", "content": p.system}));
        assert_eq!(b["messages"][1], json!({"role": "user", "content": p.user}));
        assert_eq!(b["stream"], false);
        assert_eq!(b["reasoning_effort"], "none");
        // an output limit: max_completion_tokens on OpenAI's host (its reasoning models reject
        // max_tokens), max_tokens elsewhere
        assert_eq!(b["max_completion_tokens"], 4000);
        for absent in ["temperature", "top_p", "max_tokens"] {
            assert!(b.get(absent).is_none(), "{absent}");
        }
    });
    assert!(p.user.ends_with("Glossary:\n- \"wts\" = \"vend\"\n\nMessage:\nwts {1} cheap, add {2} Zorbulax"), "{}", p.user);
    assert!(p.system.starts_with("You translate chat messages from the online game World of Warcraft into French."));
    assert_eq!(openai::reasoning_effort(&cfg_model(Provider::OpenAi, "gpt-6.1-sol")), Some("low"));
    // Ollama on this PC: no key, no Authorization header, no reasoning_effort
    let local = RemoteConfig::new(Provider::OpenAi, "qwen3:4b", "http://localhost:11434/v1", Secret::new(""), 0).unwrap();
    let t = FakeTransport::new(vec![answer(Provider::OpenAi, "x")]);
    run(&local, &p, &t).unwrap();
    t.with_seen(|s| {
        assert_eq!((s[0].url.host.as_str(), s[0].url.port, s[0].url.path.as_str(), s[0].url.https), ("localhost", 11434, "/v1/chat/completions", false));
        assert!(s[0].header("Authorization").is_none());
        assert!(s[0].body.get("reasoning_effort").is_none());
        assert_eq!(s[0].body["model"], "qwen3:4b");
        assert_eq!(s[0].body["max_tokens"], 4000);
        assert!(s[0].body.get("max_completion_tokens").is_none());
    });
    // OpenRouter, Mistral, Groq, DeepSeek: any https base; a full endpoint is not doubled
    for (base, path) in [
        ("https://openrouter.ai/api/v1", "/api/v1/chat/completions"),
        ("https://api.mistral.ai/v1/", "/v1/chat/completions"),
        ("https://api.groq.com/openai/v1", "/openai/v1/chat/completions"),
        ("https://api.deepseek.com", "/chat/completions"),
        ("https://api.deepseek.com/chat/completions", "/chat/completions"),
    ] {
        let c = RemoteConfig::new(Provider::OpenAi, "m", base, Secret::new(KEY), 0).unwrap();
        assert_eq!(c.endpoint.path, path, "{base}");
        assert!(openai::reasoning_effort(&c).is_none());
    }
    // a server other than OpenAI needs a model; plain http with a key only to this PC
    assert_eq!(RemoteConfig::new(Provider::OpenAi, "", "https://openrouter.ai/api/v1", Secret::new(KEY), 0).err(), Some(ConfigError::ModelRequired));
    assert_eq!(RemoteConfig::new(Provider::OpenAi, "m", "http://192.168.1.20:1234/v1", Secret::new(KEY), 0).err(), Some(ConfigError::InsecureUrl));
    // a server on the player's own network (a NAS) works without a key, never with one over http
    assert!(RemoteConfig::new(Provider::OpenAi, "m", "http://192.168.1.20:1234/v1", Secret::new(""), 0).is_ok(), "no key: this PC or a private address");
    assert!(!key_needed(Provider::OpenAi, "http://192.168.1.69:11434/v1") && !key_needed(Provider::OpenAi, "http://localhost:1234/v1"));
    assert!(key_needed(Provider::OpenAi, "") && key_needed(Provider::OpenAi, "https://nas.example/v1") && key_needed(Provider::Claude, ""));
    assert_eq!(RemoteConfig::new(Provider::OpenAi, "m", "https://llm.example/v1", Secret::new(""), 0).err(), Some(ConfigError::NoKey));
    // *.localhost goes through DNS on Windows: neither the http exemption nor the keyless one
    assert_eq!(RemoteConfig::new(Provider::OpenAi, "m", "http://my-llm.localhost:8080/v1", Secret::new(KEY), 0).err(), Some(ConfigError::InsecureUrl));
    assert_eq!(RemoteConfig::new(Provider::OpenAi, "m", "http://my-llm.localhost:8080/v1", Secret::new(""), 0).err(), Some(ConfigError::NoKey));
    assert!(RemoteConfig::new(Provider::OpenAi, "m", "http://127.0.0.1:1234/v1", Secret::new(KEY), 0).is_ok());
    assert_eq!(RemoteConfig::new(Provider::OpenAi, "m", "ftp://x.example", Secret::new(KEY), 0).err(), Some(ConfigError::BadUrl));
    assert_eq!(RemoteConfig::new(Provider::OpenAi, "two words", "", Secret::new(KEY), 0).err(), Some(ConfigError::BadModel));
}

#[test]
fn gemini_request_shape() {
    let p = prep(MARKED, &[]);
    let c = cfg(Provider::Gemini);
    let t = FakeTransport::new(vec![answer(Provider::Gemini, "Vends {1} pas cher")]);
    run(&c, &p, &t).unwrap();
    t.with_seen(|s| {
        let s = &s[0];
        assert_eq!(s.url.host, "generativelanguage.googleapis.com");
        assert_eq!(s.url.path, "/v1beta/models/gemini-3.5-flash-lite:generateContent");
        assert!(!s.url.path.contains("key="), "the key goes in a header, never in the URL");
        assert_eq!(s.header("x-goog-api-key"), Some(KEY));
        let b = &s.body;
        assert_eq!(b["systemInstruction"], json!({"parts": [{"text": p.system}]}));
        assert_eq!(b["contents"], json!([{"role": "user", "parts": [{"text": p.user}]}]));
        assert_eq!(b["generationConfig"]["thinkingConfig"], json!({"thinkingLevel": "minimal"}));
        assert_eq!(b["generationConfig"]["maxOutputTokens"], 4000);
        assert!(b["generationConfig"].get("temperature").is_none());
        assert_eq!(b["safetySettings"][0]["threshold"], "BLOCK_ONLY_HIGH");
    });
    // the thinking guide's table (read 2026-10-21): only documented levels are sent
    assert_eq!(gemini::thinking_config("gemini-3.8-flash"), Some(json!({"thinkingLevel": "low"})));
    assert_eq!(gemini::thinking_config("gemini-4-argon"), Some(json!({"thinkingLevel": "low"})));
    assert_eq!(gemini::thinking_config("gemini-3-flash-preview"), Some(json!({"thinkingLevel": "minimal"})));
    assert_eq!(gemini::thinking_config("gemini-3.1-flash-lite-image"), Some(json!({"thinkingLevel": "minimal"})));
    assert_eq!(gemini::thinking_config("gemini-2.5-flash"), Some(json!({"thinkingLevel": "low"})));
    assert_eq!(gemini::thinking_config("gemini-2.5-pro"), Some(json!({"thinkingLevel": "low"})));
    for none in ["gemini-3.1-flash-lite", "gemini-2.5-flash-lite", "gemini-3.5-flash", "my-tuned-model"] {
        assert_eq!(gemini::thinking_config(none), None, "{none}: not in the table");
    }
    let lite = cfg_model(Provider::Gemini, "gemini-3.1-flash-lite");
    let t = FakeTransport::new(vec![answer(Provider::Gemini, "x")]);
    run(&lite, &p, &t).unwrap();
    t.with_seen(|s| assert!(s[0].body["generationConfig"].get("thinkingConfig").is_none()));
    let c = cfg_model(Provider::Gemini, "models/gemini-3.8-flash");
    assert_eq!((c.model.as_str(), c.endpoint.path.as_str()), ("gemini-3.8-flash", "/v1beta/models/gemini-3.8-flash:generateContent"));
    assert_eq!(RemoteConfig::new(Provider::Gemini, "../x?key=1", "", Secret::new(KEY), 0).err(), Some(ConfigError::BadModel));
}

#[test]
fn claude_request_shape() {
    let p = prep(MARKED, &[("wts", "vend")]);
    let t = FakeTransport::new(vec![answer(Provider::Claude, "Vends {1} pas cher")]);
    run(&cfg(Provider::Claude), &p, &t).unwrap();
    t.with_seen(|s| {
        let s = &s[0];
        assert_eq!((s.url.host.as_str(), s.url.path.as_str()), ("api.anthropic.com", "/v1/messages"));
        assert_eq!(s.header("x-api-key"), Some(KEY));
        assert_eq!(s.header("anthropic-version"), Some("2023-06-01"));
        assert_eq!(s.header("anthropic-beta"), Some("server-side-fallback-2026-07-01"));
        let b = &s.body;
        assert_eq!(b["model"], "claude-opus-5-5");
        assert_eq!(b["max_tokens"], 4000);
        assert_eq!(b["system"], json!(p.system));
        assert_eq!(b["messages"], json!([{"role": "user", "content": p.user}]));
        assert_eq!(b["output_config"], json!({"effort": "low"}));
        assert_eq!(b["fallbacks"], "default");
        for absent in ["temperature", "top_p", "top_k", "thinking"] {
            assert!(b.get(absent).is_none(), "{absent}");
        }
        assert_eq!(b["messages"].as_array().unwrap().len(), 1, "no assistant prefill");
    });
    // Haiku 5.5: effort, no server-side fallback
    let t = FakeTransport::new(vec![answer(Provider::Claude, "x")]);
    run(&cfg_model(Provider::Claude, "claude-haiku-5-5"), &p, &t).unwrap();
    t.with_seen(|s| {
        assert!(s[0].header("anthropic-beta").is_none());
        assert!(s[0].body.get("fallbacks").is_none());
        assert_eq!(s[0].body["output_config"], json!({"effort": "low"}));
    });
    assert!(claude::takes_fallbacks("claude-sonnet-5-5"));
    assert!(!claude::takes_effort("claude-haiku-4-5"));
}

// ------------------------------------------------------------------ answers and placeholders

#[test]
fn answers_go_through_the_shared_post_processing() {
    for prov in Provider::ALL {
        let p = prep(MARKED, &[]);
        let t = FakeTransport::new(vec![answer(prov, "Vends {1} pas cher, ajoute {2} Zorbulax")]);
        let a = run(&cfg(prov), &p, &t).unwrap();
        assert_eq!((a.status, a.attempts), (200, 1));
        match pipeline::finish(&p, &a.text) {
            Finish::Done(s) => assert_eq!(s, "Vends \u{2}\u{4}0070DD[Necromancer Leggings]\u{3} pas cher, ajoute Kiwi48213 Zorbulax", "{prov:?}"),
            _ => panic!("{prov:?}: expected Done"),
        }
    }
    // DeepL moved a tag and decoded entities: still the same placeholders
    let p = prep("a <b> & \u{2}[Linen Cloth]\u{3}", &[]);
    let t = FakeTransport::new(vec![FakeReply::ok(json!({"translations": [{"text": "<x>{1}</x> a &lt;b&gt; &amp;"}]}))]);
    let a = run(&cfg(Provider::DeepL), &p, &t).unwrap();
    assert_eq!(a.text, "{1} a <b> &");
    // a <think> block from an OpenAI-compatible reasoning model is dropped by post::clean
    let p = prep("wts \u{2}[Linen Cloth]\u{3}", &[]);
    let t = FakeTransport::new(vec![answer(Provider::OpenAi, "<think>hmm</think>Vends {1}")]);
    let a = run(&cfg(Provider::OpenAi), &p, &t).unwrap();
    assert!(matches!(pipeline::finish(&p, &a.text), Finish::Done(s) if s == "Vends [Linen Cloth]"));
}

#[test]
fn broken_placeholders_ask_for_the_brackets_follow_up() {
    let p = prep(MARKED, &[]);
    let t = FakeTransport::new(vec![answer(Provider::Claude, "Vends {1} pas cher, ajoute Zorbulax")]);
    let a = run(&cfg(Provider::Claude), &p, &t).unwrap();
    let fin = pipeline::finish(&p, &a.text);
    let pipeline::Step::Again(b) = pipeline::next(&p, fin, MARKED) else { panic!("expected the Brackets retry") };
    assert_eq!(b.protected.text, "wts [Necromancer Leggings] cheap, add [Kiwi48213] Zorbulax");
    assert!(b.system.contains("item names in [brackets]"));
    assert!(b.user.ends_with("Message:\nwts [Necromancer Leggings] cheap, add [Kiwi48213] Zorbulax"));
    // DeepL keeps bracket segments in ignored tags in that form
    assert_eq!(deepl::to_xml(&b.protected.text, b.protected.variant), "wts <x>[Necromancer Leggings]</x> cheap, add <x>[Kiwi48213]</x> Zorbulax");
}

// ------------------------------------------------------------------ errors

#[test]
fn http_error_classes_per_provider() {
    for prov in Provider::ALL {
        let p = prep(MARKED, &[]);
        let mut statuses = vec![400, 401, 403, 404, 413];
        if prov == Provider::DeepL {
            statuses.push(456);
        }
        for status in statuses {
            let t = FakeTransport::new(vec![FakeReply::status(status, error_body(prov, "Something is wrong."))]);
            let e = run(&cfg(prov), &p, &t).expect_err("error");
            assert_eq!((e.fallback.kind, e.fallback.status, e.attempts), (FallbackKind::Http, Some(status), 1), "{prov:?} {status}: not retried");
            assert!(e.fallback.message.starts_with("Something is wrong."), "{prov:?}: {}", e.fallback.message);
            assert_eq!(e.fallback.provider, prov);
            no_secret(&e.fallback);
        }
        // retryable: one quick retry, then the answer
        for status in [429, 500, 503, 529] {
            let t = FakeTransport::new(vec![FakeReply::Http(status, error_body(prov, "busy").to_string(), Some(Duration::ZERO)), answer(prov, "Vends {1}")]);
            let a = run(&cfg(prov), &p, &t).unwrap();
            assert_eq!(a.attempts, 2, "{prov:?} {status}");
            // twice: the fallback carries the second status
            let t = FakeTransport::new(vec![FakeReply::status(status, error_body(prov, "busy")), FakeReply::status(status, error_body(prov, "still busy"))]);
            let e = run(&cfg(prov), &p, &t).err().unwrap();
            assert_eq!((e.fallback.status, e.attempts), (Some(status), 2));
            assert!(e.fallback.message.starts_with("still busy"));
            assert_eq!(t.count(), 2, "never a third request");
        }
        // an HTML error page is not shown
        let t = FakeTransport::new(vec![FakeReply::Http(502, "<html><body>Bad gateway</body></html>".into(), None), FakeReply::Http(502, "<html></html>".into(), None)]);
        let e = run(&cfg(prov), &p, &t).err().unwrap();
        assert_eq!((e.fallback.status, e.fallback.message.as_str()), (Some(502), ""));
    }
}

#[test]
fn retries_respect_the_budget_and_quota_errors() {
    let p = prep(MARKED, &[]);
    // retry-after beyond the budget: no retry
    let t = FakeTransport::new(vec![FakeReply::Http(429, error_body(Provider::Claude, "rate").to_string(), Some(Duration::from_secs(30)))]);
    let start = Instant::now();
    let e = run(&cfg(Provider::Claude), &p, &t).err().unwrap();
    assert_eq!((e.attempts, t.count()), (1, 1));
    assert!(start.elapsed() < Duration::from_millis(500));
    // OpenAI "insufficient_quota" is a 429 that no retry fixes
    let body = json!({"error": {"message": "You exceeded your current quota.", "type": "insufficient_quota", "code": "insufficient_quota"}});
    let t = FakeTransport::new(vec![FakeReply::status(429, body)]);
    let e = run(&cfg(Provider::OpenAi), &p, &t).err().unwrap();
    assert_eq!((e.fallback.status, e.attempts), (Some(429), 1));
    assert_eq!(e.fallback.message, "You exceeded your current quota.");
    // DeepL 456 (quota) is not retried
    let t = FakeTransport::new(vec![FakeReply::status(456, json!({"message": "Quota exceeded"}))]);
    let e = run(&cfg(Provider::DeepL), &p, &t).err().unwrap();
    assert_eq!((e.fallback.status, e.attempts, e.fallback.message.as_str()), (Some(456), 1, "Quota exceeded"));
}

#[test]
fn timeouts_and_network_errors() {
    let p = prep(MARKED, &[]);
    let short = Policy { total: Duration::from_millis(300), ..fast() };
    for prov in Provider::ALL {
        let t = FakeTransport::new(vec![FakeReply::Delay(Duration::from_secs(30), Box::new(answer(prov, "x")))]);
        let start = Instant::now();
        let e = call(&cfg(prov), &Message::of(&p), &t, short).err().unwrap();
        assert_eq!((e.fallback.kind, e.fallback.status), (FallbackKind::Timeout, None));
        assert!(start.elapsed() < short.total + Duration::from_millis(400), "{:?}", start.elapsed());
        assert_eq!(t.count(), 1, "a timeout is not retried");
    }
    let t = FakeTransport::new(vec![FakeReply::Fail(TransportError::Connect)]);
    let e = run(&cfg(Provider::Claude), &p, &t).err().unwrap();
    assert_eq!((e.fallback.kind, e.fallback.message.as_str()), (FallbackKind::Network, "could not connect"));
    let t = FakeTransport::new(vec![FakeReply::Fail(TransportError::Tls)]);
    let e = run(&cfg(Provider::Gemini), &p, &t).err().unwrap();
    assert_eq!((e.fallback.kind, e.fallback.message.as_str()), (FallbackKind::Network, "secure connection failed"));
    // the connect timeout never exceeds what is left of the total
    t.with_seen(|s| assert!(s[0].timeouts.connect <= s[0].timeouts.total));
}

#[test]
fn refusals_and_truncations() {
    let p = prep(MARKED, &[]);
    let cases: Vec<(Provider, Value, FallbackKind, &str)> = vec![
        (Provider::Claude, json!({"content": [], "stop_reason": "refusal", "stop_details": {"type": "refusal", "category": "cyber"}}), FallbackKind::Refused, "cyber"),
        (Provider::Claude, json!({"content": [{"type": "text", "text": "Vends"}], "stop_reason": "max_tokens"}), FallbackKind::Truncated, ""),
        (Provider::Claude, json!({"content": [{"type": "thinking", "thinking": ""}], "stop_reason": "end_turn"}), FallbackKind::BadAnswer, "empty answer"),
        (Provider::Gemini, json!({"candidates": [{"content": {"parts": [{"text": "x"}]}, "finishReason": "SAFETY"}]}), FallbackKind::Refused, "SAFETY"),
        (Provider::Gemini, json!({"promptFeedback": {"blockReason": "PROHIBITED_CONTENT"}}), FallbackKind::Refused, "PROHIBITED_CONTENT"),
        (Provider::Gemini, json!({"candidates": [{"content": {"parts": [{"text": "Ve"}]}, "finishReason": "MAX_TOKENS"}]}), FallbackKind::Truncated, ""),
        (Provider::Gemini, json!({"candidates": [{"content": {"parts": [{"text": "only thoughts", "thought": true}]}, "finishReason": "STOP"}]}), FallbackKind::BadAnswer, "empty answer"),
        // not refusals: an unusable answer, with the reason's name (an enum name is not a token)
        (Provider::Gemini, json!({"candidates": [{"content": {"parts": [{"text": "x"}]}, "finishReason": "MALFORMED_RESPONSE"}]}), FallbackKind::BadAnswer, "MALFORMED_RESPONSE"),
        (Provider::Gemini, json!({"candidates": [{"content": {"parts": [{"text": "x"}]}, "finishReason": "MISSING_THOUGHT_SIGNATURE"}]}), FallbackKind::BadAnswer, "MISSING_THOUGHT_SIGNATURE"),
        (Provider::Gemini, json!({"candidates": [{"content": {"parts": [{"text": "x"}]}, "finishReason": "UNEXPECTED_TOOL_CALL"}]}), FallbackKind::BadAnswer, "UNEXPECTED_TOOL_CALL"),
        (Provider::Gemini, json!({"candidates": [{"content": {"parts": [{"text": "x"}]}, "finishReason": "OTHER"}]}), FallbackKind::BadAnswer, "OTHER"),
        (Provider::Gemini, json!({"candidates": [{"content": {"parts": [{"text": "x"}]}, "finishReason": "CONTINUATION"}]}), FallbackKind::Truncated, ""),
        (Provider::Gemini, json!({"candidates": [{"content": {"parts": [{"text": "x"}]}, "finishReason": "IMAGE_SAFETY"}]}), FallbackKind::Refused, "IMAGE_SAFETY"),
        (Provider::OpenAi, json!({"choices": [{"message": {"content": null, "refusal": "I can't help with that."}, "finish_reason": "stop"}]}), FallbackKind::Refused, "I can't help with that."),
        (Provider::OpenAi, json!({"choices": [{"message": {"content": ""}, "finish_reason": "content_filter"}]}), FallbackKind::Refused, "content filter"),
        (Provider::OpenAi, json!({"choices": [{"message": {"content": "Ven"}, "finish_reason": "length"}]}), FallbackKind::Truncated, ""),
        (Provider::DeepL, json!({"translations": []}), FallbackKind::BadAnswer, "no translation in the answer"),
    ];
    for (prov, body, kind, msg) in cases {
        let t = FakeTransport::new(vec![FakeReply::ok(body)]);
        let e = run(&cfg(prov), &p, &t).err().unwrap_or_else(|| panic!("{prov:?} {kind:?}"));
        assert_eq!((e.fallback.kind, e.fallback.message.as_str(), e.fallback.status, e.attempts), (kind, msg, None, 1), "{prov:?}");
    }
    let t = FakeTransport::new(vec![FakeReply::Http(200, "not json".into(), None)]);
    assert_eq!(run(&cfg(Provider::Claude), &p, &t).err().unwrap().fallback.kind, FallbackKind::BadAnswer);
}

// ------------------------------------------------------------------ privacy

#[test]
fn sanitized_messages_hold_no_key() {
    assert_eq!(sanitize_message("Incorrect API key provided: sk-proj-abc*****wxyz. See docs.", "sk-proj-abcdefghwxyz"), "Incorrect API key provided: [redacted] See docs.");
    assert_eq!(sanitize_message(&format!("bad key {KEY}!"), KEY), "bad key [redacted]");
    assert_eq!(sanitize_message("token eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9 refused", ""), "token [redacted] refused");
    assert_eq!(sanitize_message("key ...SENTIN-9f8e7d6 wrong", KEY), "key [redacted] wrong", "a piece of the key");
    assert_eq!(sanitize_message("line\none\ttab\u{7}bell", ""), "line one tab bell");
    let long = "word ".repeat(100);
    let s = sanitize_message(&long, "");
    assert_eq!(s.chars().count(), MESSAGE_MAX);
    assert!(s.ends_with("...") && !s.contains('\u{2026}'));
    assert_eq!(sanitize_message("", KEY), "");
    // a normal service message survives
    assert_eq!(sanitize_message("model: claude-unknown-9", KEY), "model: claude-unknown-9");
    // the configured model name and an all-caps enum name are not tokens; a key piece still is
    let long_model = "gemini-3.5-flash-lite-preview-09-2026";
    assert_eq!(sanitize_message_keeping(&format!("models/{long_model} is not found."), KEY, long_model), format!("models/{long_model} is not found."));
    assert_eq!(sanitize_message(&format!("model {long_model}"), KEY), "model [redacted]", "without the model name, a long run is redacted");
    assert_eq!(sanitize_message("MISSING_THOUGHT_SIGNATURE", ""), "MISSING_THOUGHT_SIGNATURE");
    assert_eq!(sanitize_message_keeping("TPKEYSENTINEL_9F8E7D6C5B4A", KEY, ""), "[redacted]", "a piece of the key always goes");
}

#[test]
fn secrets_never_print() {
    let s = Secret::new(KEY);
    assert_eq!(format!("{s:?}"), "Secret(..)");
    let mut k = keystore::Keys::default();
    k.set("claude", s.clone());
    assert!(!format!("{k:?}").contains(KEY));
    let f = Fallback { provider: Provider::Claude, kind: FallbackKind::Http, status: Some(401), message: format!("x {NAME}") };
    assert_eq!(format!("{f:?}"), "Fallback { provider: Claude, kind: Http, status: Some(401) }");
}

// ------------------------------------------------------------------ settings and config

#[test]
fn settings_json_and_consent_per_service() {
    let d = RemoteSettings::default();
    assert_eq!(serde_json::to_value(&d).unwrap(), json!({"provider": "local", "model": "", "baseUrl": "", "consent": "", "dailyCharCap": 0}));
    let mut s: RemoteSettings = serde_json::from_value(json!({"provider": "openai", "baseUrl": "https://openrouter.ai/api/v1", "model": "m"})).unwrap();
    assert_eq!(s.daily_char_cap, 0, "missing fields take their defaults");
    assert_eq!(s.service_id(), "openai:openrouter.ai/api/v1");
    assert!(!s.has_consent());
    s.give_consent();
    assert!(s.has_consent());
    // another host or provider needs a new consent
    s.base_url = "http://localhost:11434/v1".into();
    assert!(!s.has_consent());
    assert_eq!(s.service_id(), "openai:localhost:11434/v1");
    s.provider = "claude".into();
    s.give_consent();
    assert_eq!(s.consent, "claude");
    s.provider = "gemini".into();
    assert!(!s.has_consent());
    let mut bad = RemoteSettings { provider: "bogus".into(), model: "a\u{1}".into(), base_url: "nope".into(), ..Default::default() };
    assert_eq!(bad.validate(), vec!["provider", "model", "baseUrl"]);
    assert_eq!(bad, RemoteSettings::default());
}

#[test]
fn config_from_settings() {
    let mut keys = keystore::Keys::default();
    let local = RemoteSettings::default();
    assert!(RemoteConfig::from_settings(&local, &keys).unwrap().is_none());
    let mut s = RemoteSettings { provider: "claude".into(), daily_char_cap: 5000, ..Default::default() };
    assert_eq!(RemoteConfig::from_settings(&s, &keys).err(), Some(ConfigError::NoConsent));
    s.give_consent();
    assert_eq!(RemoteConfig::from_settings(&s, &keys).err(), Some(ConfigError::NoKey));
    keys.set("claude", Secret::new(KEY));
    let c = RemoteConfig::from_settings(&s, &keys).unwrap().unwrap();
    assert_eq!((c.provider, c.model.as_str(), c.daily_char_cap, c.key.expose()), (Provider::Claude, "claude-opus-5-5", 5000, KEY));
    assert_eq!(c.identity(), "remote:claude:claude-opus-5-5");
    assert_eq!(c.model_label(), "claude-opus-5-5");
    assert_eq!(cfg_model(Provider::Claude, "claude-x-private").model_label(), "custom");
    let o = RemoteConfig::new(Provider::OpenAi, "m", "https://api.mistral.ai/v1", Secret::new(KEY), 0).unwrap();
    assert_eq!(o.identity(), "remote:openai:api.mistral.ai/v1:m");
    for code in [ConfigError::NoConsent, ConfigError::NoKey, ConfigError::BadUrl, ConfigError::InsecureUrl, ConfigError::ModelRequired, ConfigError::BadModel] {
        assert!(!code.code().is_empty());
    }
    for p in Provider::ALL {
        assert_eq!(Provider::from_id(p.id()), Some(p));
        assert!(p.model_suggestions().iter().any(|m| *m == p.default_model()) || p == Provider::DeepL);
    }
}

#[test]
fn test_button_translates_the_synthetic_sentence() {
    let t = FakeTransport::new(vec![answer(Provider::Claude, "Salut ! Quelqu'un pour un donjon ce soir ? On cherche un soigneur.")]);
    let got = test_translation(&cfg(Provider::Claude), "fr", &t).unwrap();
    assert_eq!(got, "Salut ! Quelqu'un pour un donjon ce soir ? On cherche un soigneur.");
    t.with_seen(|s| assert!(s[0].body["messages"][0]["content"].as_str().unwrap().ends_with(TEST_SENTENCE)));
    let t = FakeTransport::new(vec![FakeReply::status(401, error_body(Provider::Claude, "invalid x-api-key"))]);
    let e = test_translation(&cfg(Provider::Claude), "fr", &t).unwrap_err();
    assert_eq!((e.kind, e.status, e.message.as_str()), (FallbackKind::Http, Some(401), "invalid x-api-key"));
}

#[test]
fn counted_chars_are_text_and_glossary() {
    let p = prep("wts \u{2}[Linen Cloth]\u{3} pst", &[("wts", "vend"), ("pst", "chuchotez")]);
    let m = Message::of(&p);
    // "wts {1} pst" = 11, pairs 3+4+3+9 = 19
    assert_eq!(m.counted_chars(), 30);
}

// ------------------------------------------------------------------ review findings (2026-10-21)

#[test]
fn a_saved_key_goes_only_to_the_server_it_was_saved_for() {
    // keys.bin holds a key for OpenAI's own server
    let mut keys = keystore::Keys::default();
    keys.set(&service_id(Provider::OpenAi, ""), Secret::new(KEY));
    assert_eq!(service_id(Provider::OpenAi, ""), "openai:api.openai.com/v1");
    let mut s = RemoteSettings { provider: "openai".into(), ..Default::default() };
    s.give_consent();
    let c = RemoteConfig::from_settings(&s, &keys).unwrap().unwrap();
    assert_eq!((c.key.expose(), c.service_id().as_str()), (KEY, "openai:api.openai.com/v1"));
    // the player types another server: the saved key is not sent there
    s.base_url = "https://free-llm.example/v1".into();
    s.model = "m".into();
    s.give_consent();
    assert_eq!(RemoteConfig::from_settings(&s, &keys).err(), Some(ConfigError::NoKey));
    // nor to a keyless server on this PC (no Authorization header at all)
    s.base_url = "http://localhost:1234/v1".into();
    s.give_consent();
    let local = RemoteConfig::from_settings(&s, &keys).unwrap().unwrap();
    assert!(local.key.is_empty());
    let t = FakeTransport::new(vec![answer(Provider::OpenAi, "x")]);
    run(&local, &prep(MARKED, &[]), &t).unwrap();
    t.with_seen(|seen| assert!(seen[0].header("Authorization").is_none()));
    // the same server written another way is the same service
    assert_eq!(service_id(Provider::OpenAi, "https://api.openai.com/v1/"), service_id(Provider::OpenAi, ""));
    assert_eq!(service_id(Provider::OpenAi, "https://API.openai.com/v1/chat/completions"), service_id(Provider::OpenAi, ""));
    assert_eq!(service_id(Provider::OpenAi, "https://api.deepseek.com"), service_id(Provider::OpenAi, "https://api.deepseek.com/chat/completions"));
    assert_eq!(service_id(Provider::OpenAi, "nope"), "");
    assert_eq!(service_id(Provider::Claude, "https://ignored.example"), "claude");
}

#[test]
fn routes_behind_one_host_are_separate_services() {
    let groq = "https://gateway.ai.cloudflare.com/v1/acct/gw/groq";
    let workers = "https://gateway.ai.cloudflare.com/v1/acct/gw/workers-ai";
    let a = RemoteConfig::new(Provider::OpenAi, "llama-3.3-70b", groq, Secret::new(KEY), 0).unwrap();
    let b = RemoteConfig::new(Provider::OpenAi, "llama-3.3-70b", workers, Secret::new(KEY), 0).unwrap();
    assert_ne!(a.identity(), b.identity(), "no shared cache entries");
    assert_eq!(a.identity(), "remote:openai:gateway.ai.cloudflare.com/v1/acct/gw/groq:llama-3.3-70b");
    assert_ne!(service_id(Provider::OpenAi, groq), service_id(Provider::OpenAi, workers), "no shared consent or key");
    let mut s = RemoteSettings { provider: "openai".into(), base_url: groq.into(), model: "m".into(), ..Default::default() };
    s.give_consent();
    s.base_url = workers.into();
    assert!(!s.has_consent());
}

#[test]
fn deepl_answers_with_entities_before_cjk_cyrillic_or_hangul_do_not_crash() {
    // review finding: the release build aborts on a panic; these answers panicked in from_xml
    let p = prep("hi <Jerry come", &[]);
    for (xml, want) in [("汤姆&amp;杰瑞很棒", "汤姆&杰瑞很棒"), ("你好&lt;我爱你们", "你好<我爱你们"), ("x&amp;Джерри", "x&Джерри"), ("안녕&gt;하세요반갑습니다", "안녕>하세요반갑습니다")] {
        let t = FakeTransport::new(vec![FakeReply::ok(json!({"translations": [{"text": xml}]}))]);
        let a = run(&cfg(Provider::DeepL), &p, &t).unwrap();
        assert_eq!(a.text, want);
    }
}

#[test]
fn openai_compatible_output_is_bounded_everywhere() {
    let p = prep(MARKED, &[]);
    for base in ["", "https://openrouter.ai/api/v1", "http://localhost:11434/v1"] {
        let c = RemoteConfig::new(Provider::OpenAi, "m", base, Secret::new(if base.starts_with("http:") { "" } else { KEY }), 0).unwrap();
        let t = FakeTransport::new(vec![answer(Provider::OpenAi, "x")]);
        run(&c, &p, &t).unwrap();
        t.with_seen(|s| {
            let b = &s[0].body;
            let limit = b.get("max_completion_tokens").or_else(|| b.get("max_tokens")).and_then(Value::as_u64);
            assert_eq!(limit, Some(u64::from(openai::OUTPUT_LIMIT)), "{base}");
        });
    }
}
