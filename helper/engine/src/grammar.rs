//! Per-target GBNF grammars that block scripts the target language cannot use.
//!
//! Measured: without a grammar Qwen3-4B sometimes answered a French request in Chinese; the Latin
//! grammar fixed it at no measurable cost. The `\uXXXX` escapes are parsed by llama.cpp.

/// en, fr, de, es, it, pt, ru: no CJK ideographs, kana, Hangul syllables or full-width forms.
pub const LATIN: &str = r"root ::= [^\u3000-\u9FFF\uAC00-\uD7AF\uF900-\uFAFF\uFF00-\uFFEF]*";
/// zh, zh-TW: no kana, Hangul or Cyrillic.
pub const CHINESE: &str = r"root ::= [^\u3040-\u30FF\uAC00-\uD7AF\u1100-\u11FF\u3130-\u318F\u0400-\u04FF]*";
/// ko: no kana, Cyrillic or CJK ideographs.
pub const KOREAN: &str = r"root ::= [^\u3040-\u30FF\u0400-\u04FF\u4E00-\u9FFF\u3400-\u4DBF]*";

pub fn for_target(target: &str) -> &'static str {
    match target {
        "zh" | "zh-TW" => CHINESE,
        "ko" => KOREAN,
        _ => LATIN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grammar_strings() {
        // The measured string, byte for byte (exe-tech llama.rs PROBE_GRAMMAR).
        assert_eq!(LATIN, "root ::= [^\\u3000-\\u9FFF\\uAC00-\\uD7AF\\uF900-\\uFAFF\\uFF00-\\uFFEF]*");
        for t in ["en", "fr", "de", "es", "it", "pt", "ru"] {
            assert_eq!(for_target(t), LATIN, "{t}");
        }
        assert_eq!(for_target("zh"), CHINESE);
        assert_eq!(for_target("zh-TW"), CHINESE);
        assert_eq!(for_target("ko"), KOREAN);
        assert_eq!(CHINESE, "root ::= [^\\u3040-\\u30FF\\uAC00-\\uD7AF\\u1100-\\u11FF\\u3130-\\u318F\\u0400-\\u04FF]*");
        assert_eq!(KOREAN, "root ::= [^\\u3040-\\u30FF\\u0400-\\u04FF\\u4E00-\\u9FFF\\u3400-\\u4DBF]*");
        // Single rule, a negated class, ASCII only (JSON-safe).
        for g in [LATIN, CHINESE, KOREAN] {
            assert!(g.starts_with("root ::= [^") && g.ends_with("]*") && g.is_ascii());
        }
    }
}
