//! DeepL `POST /v2/translate` (developers.deepl.com, read 2026-10-20).
//!
//! - Host: `api-free.deepl.com` for a free key (ending in `:fx`), else `api.deepl.com`.
//! - `Authorization: DeepL-Auth-Key <key>`, JSON body `{"text": [...], "target_lang": ...}`.
//! - Placeholders stay intact through XML tag handling: the text is XML-escaped and every `{N}`
//!   (and, in the Brackets form, every `[...]` segment) is wrapped in `<x>...</x>` with
//!   `ignore_tags: ["x"]`, whose content DeepL never translates.
//! - `context` ("World of Warcraft chat" and the glossary pairs) influences the translation, is
//!   not translated and not billed. `formality: prefer_less` (casual chat; the `prefer_` form
//!   never fails for a language without formality), `preserve_formatting: true`.
//! - The model field is DeepL's `model_type` (sent only when set).
//! - Errors: `{"message": ...}`; 403 invalid key, 456 quota exceeded, 429 too many requests,
//!   413 too large, 5xx and 529 retryable.

use serde_json::{json, Value};

use super::{Failure, FallbackKind, HttpRequest, Message, Parsed, RemoteConfig};
use crate::prompt::Variant;

/// Our target codes to DeepL's (`EN`, `PT` and `ZH` alone are the deprecated unspecified
/// variants; the `/v3/languages` sample lists `en` as not usable as a target).
pub fn target_code(target: &str) -> &'static str {
    match target {
        "en" => "EN-US",
        "fr" => "FR",
        "de" => "DE",
        "es" => "ES",
        "it" => "IT",
        "pt" => "PT-BR",
        "ru" => "RU",
        "ko" => "KO",
        "zh" => "ZH-HANS",
        "zh-TW" => "ZH-HANT",
        _ => "EN-US",
    }
}

fn escape(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
}

/// The protected text as XML: escaped, placeholders (and bracket segments in the Brackets form)
/// inside ignored `<x>` tags.
pub fn to_xml(text: &str, variant: Variant) -> String {
    let mut out = String::with_capacity(text.len() + 32);
    let mut rest = text;
    loop {
        let next = match variant {
            Variant::Placeholders => find_placeholder(rest),
            Variant::Brackets => rest.find('[').and_then(|a| rest[a..].find(']').map(|b| (a, a + b + 1))),
        };
        let Some((a, b)) = next else { break };
        escape(&rest[..a], &mut out);
        out.push_str("<x>");
        escape(&rest[a..b], &mut out);
        out.push_str("</x>");
        rest = &rest[b..];
    }
    escape(rest, &mut out);
    out
}

/// The first `{N}` (one digit 1-9, the engine's placeholder form) as a byte range.
fn find_placeholder(s: &str) -> Option<(usize, usize)> {
    let b = s.as_bytes();
    (0..b.len().saturating_sub(2)).find(|&i| b[i] == b'{' && (b'1'..=b'9').contains(&b[i + 1]) && b[i + 2] == b'}').map(|i| (i, i + 3))
}

/// DeepL's XML answer back to plain text: the `<x>` tags are removed first (DeepL may move or
/// duplicate them; their content stays), then the entities are decoded, so text the player
/// typed as `<x>` (sent as `&lt;x&gt;`) survives.
pub fn from_xml(xml: &str) -> String {
    let stripped = xml.replace("<x>", "").replace("</x>", "").replace("<x/>", "");
    unescape(&stripped)
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        // ';' is ASCII: its byte index is a char boundary (never slice the text at a fixed byte:
        // CJK or Cyrillic text right after '&' would cut a character).
        let end = tail.as_bytes().iter().take(12).position(|&b| b == b';');
        let decoded = end.and_then(|e| {
            let ent = &tail[1..e];
            let c = match ent {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ if ent.starts_with("#x") || ent.starts_with("#X") => u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32),
                _ if ent.starts_with('#') => ent[1..].parse::<u32>().ok().and_then(char::from_u32),
                _ => None,
            };
            c.map(|c| (c, e + 1))
        });
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &tail[len..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The `context` parameter: the setting and the glossary pairs the prompt would list.
pub fn context(pairs: &[(String, String)]) -> String {
    let mut s = String::from("World of Warcraft chat.");
    if !pairs.is_empty() {
        s.push_str(" Glossary:");
        for (i, (t, m)) in pairs.iter().enumerate() {
            s.push_str(if i == 0 { " " } else { "; " });
            s.push_str(&format!("\"{t}\" = \"{m}\""));
        }
        s.push('.');
    }
    s
}

pub fn request(cfg: &RemoteConfig, msg: &Message) -> HttpRequest {
    let mut body = json!({
        "text": [to_xml(msg.text, msg.variant)],
        "target_lang": target_code(msg.target),
        "context": context(msg.pairs),
        "tag_handling": "xml",
        "ignore_tags": ["x"],
        "preserve_formatting": true,
        "formality": "prefer_less",
    });
    if !cfg.model.is_empty() {
        body["model_type"] = json!(cfg.model);
    }
    HttpRequest {
        url: cfg.endpoint.clone(),
        headers: vec![("Authorization", format!("DeepL-Auth-Key {}", cfg.key.expose()))],
        body: serde_json::to_vec(&body).unwrap_or_default(),
    }
}

pub fn parse(body: &[u8]) -> Parsed {
    let Ok(v) = serde_json::from_slice::<Value>(body) else { return Parsed::Fail(Failure::new(FallbackKind::BadAnswer, "not JSON")) };
    match v.pointer("/translations/0/text").and_then(Value::as_str) {
        Some(t) if !t.trim().is_empty() => Parsed::Text(from_xml(t)),
        _ => Parsed::Fail(Failure::new(FallbackKind::BadAnswer, "no translation in the answer")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_round_trip_keeps_placeholders_and_typed_markup() {
        let x = to_xml("wts {1} & {2} <cheap> {0} {12}", Variant::Placeholders);
        // only the engine's placeholder form {1}..{9} is wrapped
        assert_eq!(x, "wts <x>{1}</x> &amp; <x>{2}</x> &lt;cheap&gt; {0} {12}");
        assert_eq!(from_xml(&x), "wts {1} & {2} <cheap> {0} {12}");
        assert_eq!(from_xml("Vends <x>{1}</x> &amp; <x>{2}</x> &lt;x&gt; pas cher"), "Vends {1} & {2} <x> pas cher");
        assert_eq!(from_xml("a &#233;t&#xE9; &bogus; & b"), "a été &bogus; & b");
        let b = to_xml("wts [Linen Cloth] cheap", Variant::Brackets);
        assert_eq!(b, "wts <x>[Linen Cloth]</x> cheap");
    }

    #[test]
    fn entities_followed_by_multibyte_text_decode_without_panic() {
        // review finding: a fixed 12-byte cut after '&' fell inside a character
        assert_eq!(from_xml("汤姆&amp;杰瑞很棒"), "汤姆&杰瑞很棒");
        assert_eq!(from_xml("你好&lt;我爱你们"), "你好<我爱你们");
        assert_eq!(from_xml("x&amp;Джерри"), "x&Джерри");
        assert_eq!(from_xml("안녕&gt;하세요반갑습니다"), "안녕>하세요반갑습니다");
        assert_eq!(from_xml("&我爱你们你们"), "&我爱你们你们", "a bare '&' before CJK stays");
        assert_eq!(from_xml("&#x4F60;&#22909;"), "你好");
        let round = to_xml("hi <我爱你们 come & Джерри {1}", Variant::Placeholders);
        assert_eq!(from_xml(&round), "hi <我爱你们 come & Джерри {1}");
    }

    #[test]
    fn target_codes() {
        let all = ["en", "fr", "de", "es", "it", "pt", "ru", "ko", "zh", "zh-TW"];
        let codes: Vec<&str> = all.iter().map(|t| target_code(t)).collect();
        assert_eq!(codes, ["EN-US", "FR", "DE", "ES", "IT", "PT-BR", "RU", "KO", "ZH-HANS", "ZH-HANT"]);
    }

    #[test]
    fn context_lists_the_pairs() {
        assert_eq!(context(&[]), "World of Warcraft chat.");
        let p = vec![("wts".to_string(), "vend".to_string()), ("pst".to_string(), "chuchotez-moi".to_string())];
        assert_eq!(context(&p), "World of Warcraft chat. Glossary: \"wts\" = \"vend\"; \"pst\" = \"chuchotez-moi\".");
    }
}
