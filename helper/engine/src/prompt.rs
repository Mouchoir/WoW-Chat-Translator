//! Prompt text (raw ChatML for `/completion`) and the glossary policy. See docs/ENGINE.md
//! (design section D.5, E1 and E1b evaluations).
//!
//! The output script is enforced by the per-target grammar. The E1b evaluation measured the
//! prompt styles below; the default is the style of the tuning that passed the E1b rule.

/// Part of the cache key; bump when the prompt text changes. 2: E1b tuning (casual style,
/// insist retry, expanded must-use terms). 3 (2026-10-19, L4): IDs, URLs and e-mail addresses
/// become placeholders, and the cache key holds the span contents and colours.
pub const PROMPT_VERSION: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    /// Item links and `{...}` groups replaced by `{1}..{9}`.
    Placeholders,
    /// The measured prompt: original names kept inline in `[brackets]`.
    Brackets,
}

/// The variant used first. E1 (CPU, 40 synthetic lines): Placeholders restored 19/19 item names
/// against 10/19 for Brackets. Brackets stays the retry when placeholders come back broken.
pub const DEFAULT: Variant = Variant::Placeholders;

/// System prompt wording.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// E1 prompt (V0): "You translate World of Warcraft chat messages into {L}. Keep player
    /// names, numbers, and every token like {1} exactly as written. Use the glossary for game
    /// abbreviations. Reply with the translation only."
    Plain,
    /// V1: the system prompt the lead measured on GPU before the design (judged the best of
    /// four models), generalized to any target and to placeholders.
    Casual,
}

/// The style used by the engine (E1b decision, docs/ENGINE.md section 10): the casual style
/// keeps game abbreviations and makes fewer echoes (wrong Same 6 to 1 alone).
pub const DEFAULT_STYLE: Style = Style::Casual;

/// What the prompt asks for, beyond the variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Opts {
    pub style: Style,
    /// The retry after an unchanged output (V2): the user turn says the message is not in the
    /// target language yet and names the must-use meanings.
    pub insist: bool,
}

impl Opts {
    pub const DEFAULT: Opts = Opts { style: DEFAULT_STYLE, insist: false };
}

const SYSTEM_PLACEHOLDERS: &str = "Keep player names, numbers, and every token like {1} exactly as written. Use the glossary for game abbreviations. Reply with the translation only.";
/// The measured system text, verbatim apart from the language name.
const SYSTEM_BRACKETS: &str = "Keep player names, numbers, and anything inside [brackets] exactly as written. Use the glossary for game abbreviations. Reply with the translation only.";

/// Game abbreviations of the lead's measured prompt, kept in the casual style.
const CASUAL_ABBREVIATIONS: &str = "(LFM, LFG, WTS, WTB, pst, DPS, BG, DM...)";

/// English name of a target language as written in the prompt.
pub fn language_name(target: &str) -> Option<&'static str> {
    Some(match target {
        "en" => "English",
        "fr" => "French",
        "de" => "German",
        "es" => "Spanish",
        "it" => "Italian",
        "pt" => "Brazilian Portuguese",
        "ru" => "Russian",
        "ko" => "Korean",
        "zh" => "Simplified Chinese",
        "zh-TW" => "Traditional Chinese",
        _ => return None,
    })
}

/// "a French player" in the casual style ("the way a French player would write in chat").
fn player(target: &str) -> &'static str {
    match target {
        "fr" => "a French player",
        "de" => "a German player",
        "es" => "a Spanish player",
        "it" => "an Italian player",
        "pt" => "a Brazilian player",
        "ru" => "a Russian player",
        "ko" => "a Korean player",
        "zh" => "a Chinese player",
        "zh-TW" => "a Taiwanese player",
        _ => "an English-speaking player",
    }
}

/// The system block and the start of the user turn, shared by every message to one target. The
/// warm-up request sends exactly this (default variant and style), so the server's prefix cache
/// already holds it.
pub fn system_prefix(target: &str, v: Variant) -> String {
    system_prefix_with(target, v, DEFAULT_STYLE)
}

pub fn system_prefix_with(target: &str, v: Variant, style: Style) -> String {
    // Unknown targets cannot come from the addon (Lang.TARGETS); English is the safe fallback.
    let lang = language_name(target).unwrap_or("English");
    let system = match style {
        Style::Plain => {
            let rules = match v {
                Variant::Placeholders => SYSTEM_PLACEHOLDERS,
                Variant::Brackets => SYSTEM_BRACKETS,
            };
            format!("You translate World of Warcraft chat messages into {lang}. {rules}")
        }
        Style::Casual => {
            let items = match v {
                Variant::Placeholders => "every token like {1} (item names)",
                Variant::Brackets => "item names in [brackets]",
            };
            let who = player(target);
            format!(
                "You translate chat messages from the online game World of Warcraft into {lang}. Write natural, casual {lang}, the way {who} would write in chat. Keep {items}, player names and game abbreviations {CASUAL_ABBREVIATIONS} unchanged. Use the official {lang} World of Warcraft names when you know them. Reply with the {lang} translation only."
            )
        }
    };
    format!("<|im_start|>system\n{system}<|im_end|>\n<|im_start|>user\n")
}

/// Builds the full prompt with the default style. `protected` is the neutralized text
/// (placeholders in place for `Variant::Placeholders`); `pairs` are (term, meaning), already
/// neutralized.
pub fn build(target: &str, protected: &str, pairs: &[(&str, &str)], v: Variant) -> String {
    build_with(target, protected, pairs, v, Opts::DEFAULT)
}

pub fn build_with(target: &str, protected: &str, pairs: &[(&str, &str)], v: Variant, o: Opts) -> String {
    let mut s = system_prefix_with(target, v, o.style);
    if !pairs.is_empty() {
        s.push_str("Glossary:\n");
        for (term, meaning) in pairs {
            s.push_str("- \"");
            s.push_str(term);
            s.push_str("\" = \"");
            s.push_str(meaning);
            s.push_str("\"\n");
        }
        s.push('\n');
    }
    if o.insist {
        s.push_str(&insist_text(target, pairs, v));
        s.push_str("\n\n");
    }
    s.push_str("Message:\n");
    s.push_str(protected);
    s.push_str("<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n");
    s
}

/// The stronger instruction of the V2 retry, placed before "Message:".
fn insist_text(target: &str, pairs: &[(&str, &str)], v: Variant) -> String {
    let lang = language_name(target).unwrap_or("English");
    let items = match v {
        Variant::Placeholders => "tokens like {1}",
        Variant::Brackets => "item names in [brackets]",
    };
    let mut s = format!("This message is not in {lang} yet. Translate all of it into {lang}; only {items}, player names and game abbreviations may stay as written.");
    let uses: Vec<String> = pairs
        .iter()
        .filter(|(t, m)| must_use(t) && single_meaning(m))
        .map(|(t, m)| format!("\"{m}\" for \"{t}\""))
        .collect();
    if !uses.is_empty() {
        s.push_str(" Write ");
        s.push_str(&uses.join(", "));
        s.push('.');
    }
    s
}

/// Game abbreviations that players type as-is in every language: grouping and trade, roles and
/// fights, chat signals, and the short names of battlegrounds, dungeons, raids and cities. A
/// glossary pair whose term is one of these (or its plural, or `lf<N>`/`lf<N>m`) is
/// keep-allowed: the translation may keep the term as typed, because the addon's glossary row
/// already shows its meaning. Every other pair is must-use: the translation has to carry its
/// meaning (chat words such as thx, brb, gz, pls, false friends such as cheap, full names such
/// as stockade, and user entries for other terms).
const KEEP_ALLOWED: &[&str] = &[
    // Grouping and trade.
    "lf", "lfg", "lfm", "pug", "wts", "wtb", "wtt", "pst", "inv", "ah", "boe", "bop", "ml", "summ", "lvl",
    // Roles and fights.
    "tank", "heal", "heals", "healer", "dps", "dd", "mt", "ot", "hc", "oom", "cc", "aoe", "rez", "rdy", "dot", "hot",
    // Chat signals.
    "afk", "gg", "wp", "lol",
    // Battlegrounds, dungeons, raids and cities (short names only; full names must be localized).
    "bg", "wsg", "ab", "av", "rfc", "wc", "dm", "sfk", "bfd", "gnomer", "rfk", "sm", "gy", "rfd", "ulda", "zf", "mara",
    "st", "brd", "lbrs", "ubrs", "dme", "dmw", "dmn", "strat", "scholo", "mc", "ony", "bwl", "zg", "aq20", "aq40",
    "naxx", "sw", "org", "ogri", "tb", "uc", "xr",
];

/// True when a glossary term may stay as typed in the translation (see `KEEP_ALLOWED`).
pub fn keep_allowed(term: &str) -> bool {
    let t = term.trim().to_ascii_lowercase();
    if KEEP_ALLOWED.contains(&t.as_str()) {
        return true;
    }
    if let Some(base) = t.strip_suffix('s') {
        if KEEP_ALLOWED.contains(&base) {
            return true;
        }
    }
    // lf2, lf2m, lf10m.
    if let Some(rest) = t.strip_prefix("lf") {
        let digits = rest.strip_suffix('m').unwrap_or(rest);
        return !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit());
    }
    false
}

/// True when the translation has to carry the term's meaning.
pub fn must_use(term: &str) -> bool {
    !keep_allowed(term)
}

/// A meaning with one sense only (the addon joins several senses with " / ").
pub fn single_meaning(meaning: &str) -> bool {
    !meaning.contains(" / ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_names() {
        for code in ["en", "fr", "de", "es", "it", "pt", "ru", "ko", "zh", "zh-TW"] {
            assert!(language_name(code).is_some(), "{code}");
        }
        assert_eq!(language_name("pt"), Some("Brazilian Portuguese"));
        assert_eq!(language_name("xx"), None);
        assert_eq!(language_name("zh-tw"), None);
    }

    const PLAIN: Opts = Opts { style: Style::Plain, insist: false };

    const GOLDEN_FR_PLACEHOLDERS_GLOSSARY: &str = "<|im_start|>system\n\
You translate World of Warcraft chat messages into French. Keep player names, numbers, and every token like {1} exactly as written. Use the glossary for game abbreviations. Reply with the translation only.<|im_end|>\n\
<|im_start|>user\n\
Glossary:\n\
- \"wts\" = \"vend\"\n\
- \"cheap\" = \"pas cher\"\n\
\n\
Message:\n\
wts {1} x20 cheap<|im_end|>\n\
<|im_start|>assistant\n\
<think>\n\
\n\
</think>\n\
\n";

    const GOLDEN_EN_BRACKETS_PLAIN: &str = "<|im_start|>system\n\
You translate World of Warcraft chat messages into English. Keep player names, numbers, and anything inside [brackets] exactly as written. Use the glossary for game abbreviations. Reply with the translation only.<|im_end|>\n\
<|im_start|>user\n\
Message:\n\
vds [Étoffe de lin]<|im_end|>\n\
<|im_start|>assistant\n\
<think>\n\
\n\
</think>\n\
\n";

    const GOLDEN_FR_CASUAL_GLOSSARY: &str = "<|im_start|>system\n\
You translate chat messages from the online game World of Warcraft into French. Write natural, casual French, the way a French player would write in chat. Keep every token like {1} (item names), player names and game abbreviations (LFM, LFG, WTS, WTB, pst, DPS, BG, DM...) unchanged. Use the official French World of Warcraft names when you know them. Reply with the French translation only.<|im_end|>\n\
<|im_start|>user\n\
Glossary:\n\
- \"wts\" = \"vend\"\n\
- \"cheap\" = \"pas cher\"\n\
\n\
Message:\n\
wts {1} x20 cheap<|im_end|>\n\
<|im_start|>assistant\n\
<think>\n\
\n\
</think>\n\
\n";

    const GOLDEN_FR_CASUAL_INSIST: &str = "<|im_start|>system\n\
You translate chat messages from the online game World of Warcraft into French. Write natural, casual French, the way a French player would write in chat. Keep every token like {1} (item names), player names and game abbreviations (LFM, LFG, WTS, WTB, pst, DPS, BG, DM...) unchanged. Use the official French World of Warcraft names when you know them. Reply with the French translation only.<|im_end|>\n\
<|im_start|>user\n\
Glossary:\n\
- \"brb\" = \"je reviens\"\n\
- \"lfm\" = \"cherche des membres\"\n\
- \"inv\" = \"invite / inventaire\"\n\
\n\
This message is not in French yet. Translate all of it into French; only tokens like {1}, player names and game abbreviations may stay as written. Write \"je reviens\" for \"brb\".\n\
\n\
Message:\n\
brb, lfm inv {1}<|im_end|>\n\
<|im_start|>assistant\n\
<think>\n\
\n\
</think>\n\
\n";

    #[test]
    fn golden_placeholders_with_glossary() {
        let p = build_with("fr", "wts {1} x20 cheap", &[("wts", "vend"), ("cheap", "pas cher")], Variant::Placeholders, PLAIN);
        assert_eq!(p, GOLDEN_FR_PLACEHOLDERS_GLOSSARY);
    }

    #[test]
    fn golden_brackets_without_glossary() {
        let p = build_with("en", "vds [Étoffe de lin]", &[], Variant::Brackets, PLAIN);
        assert_eq!(p, GOLDEN_EN_BRACKETS_PLAIN);
    }

    #[test]
    fn golden_casual_with_glossary() {
        let p = build_with("fr", "wts {1} x20 cheap", &[("wts", "vend"), ("cheap", "pas cher")], Variant::Placeholders, Opts { style: Style::Casual, insist: false });
        assert_eq!(p, GOLDEN_FR_CASUAL_GLOSSARY);
    }

    #[test]
    fn golden_casual_insist() {
        let pairs = [("brb", "je reviens"), ("lfm", "cherche des membres"), ("inv", "invite / inventaire")];
        let p = build_with("fr", "brb, lfm inv {1}", &pairs, Variant::Placeholders, Opts { style: Style::Casual, insist: true });
        assert_eq!(p, GOLDEN_FR_CASUAL_INSIST);
        // Without must-use pairs the insist line has no "Write ..." part.
        let p = build_with("de", "lfm {1} need tank", &[("lfm", "looking for more")], Variant::Placeholders, Opts { style: Style::Plain, insist: true });
        assert!(p.contains("\n\nThis message is not in German yet. Translate all of it into German; only tokens like {1}, player names and game abbreviations may stay as written.\n\nMessage:\n"));
        let p = build_with("en", "vds [x]", &[], Variant::Brackets, Opts { style: Style::Casual, insist: true });
        assert!(p.contains("only item names in [brackets], player names"));
        assert!(p.contains("<|im_start|>user\nThis message is not in English yet."));
    }

    #[test]
    fn default_build_uses_the_default_style() {
        let p = build("fr", "a", &[], Variant::Placeholders);
        assert_eq!(p, build_with("fr", "a", &[], Variant::Placeholders, Opts::DEFAULT));
        assert!(p.starts_with(&system_prefix("fr", Variant::Placeholders)));
        assert!(!p.contains("not in French yet"));
    }

    #[test]
    fn every_target_variant_and_style() {
        for code in ["en", "fr", "de", "es", "it", "pt", "ru", "ko", "zh", "zh-TW"] {
            let name = language_name(code).unwrap();
            for v in [Variant::Placeholders, Variant::Brackets] {
                for style in [Style::Plain, Style::Casual] {
                    for insist in [false, true] {
                        let o = Opts { style, insist };
                        for pairs in [&[][..], &[("lfm", "looking for more")][..]] {
                            let p = build_with(code, "hello {1}", pairs, v, o);
                            assert!(p.starts_with(&system_prefix_with(code, v, style)));
                            match style {
                                Style::Plain => assert!(p.contains(&format!("chat messages into {name}. Keep")), "{code}"),
                                Style::Casual => {
                                    assert!(p.contains(&format!("World of Warcraft into {name}. Write natural, casual {name}, the way ")), "{code}");
                                    assert!(p.contains(&format!("Reply with the {name} translation only.")));
                                }
                            }
                            assert_eq!(p.contains("Glossary:\n- \"lfm\" = \"looking for more\"\n\n"), !pairs.is_empty());
                            assert_eq!(p.contains("token like {1}"), v == Variant::Placeholders);
                            assert_eq!(p.contains("[brackets]"), v == Variant::Brackets);
                            assert_eq!(p.contains(&format!("This message is not in {name} yet.")), insist);
                            assert!(p.ends_with("Message:\nhello {1}<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"));
                            // Exactly one system, user and assistant turn.
                            assert_eq!(p.matches("<|im_start|>").count(), 3);
                            assert_eq!(p.matches("<|im_end|>").count(), 2);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn measured_system_text_is_kept_for_brackets() {
        // Text of the measured probe prompt (exe-tech llama.rs), French target.
        let measured = "You translate World of Warcraft chat messages into French. Keep player names, numbers, and anything inside [brackets] exactly as written. Use the glossary for game abbreviations. Reply with the translation only.";
        assert!(system_prefix_with("fr", Variant::Brackets, Style::Plain).contains(measured));
    }

    #[test]
    fn lead_reference_prompt_is_the_casual_style() {
        // The lead's GPU reference system prompt (French target), with only the item wording
        // changed for placeholders.
        let lead = "You translate chat messages from the online game World of Warcraft into French. Write natural, casual French, the way a French player would write in chat. Keep item names in [brackets], player names and game abbreviations (LFM, LFG, WTS, WTB, pst, DPS, BG, DM...) unchanged. Use the official French World of Warcraft names when you know them. Reply with the French translation only.";
        assert!(system_prefix_with("fr", Variant::Brackets, Style::Casual).contains(lead));
        assert!(system_prefix_with("fr", Variant::Placeholders, Style::Casual).contains(&lead.replace("item names in [brackets]", "every token like {1} (item names)")));
    }

    #[test]
    fn unknown_target_falls_back_to_english() {
        assert!(build("xx", "a", &[], DEFAULT).contains("into English."));
    }

    #[test]
    fn glossary_policy() {
        for t in ["lfm", "LFM", "lfg", "lf", "wts", "WTB", "pst", "dps", "bg", "BGs", "hc", "afk", "gg", "lol", "inv", "dm", "brd", "lf2m", "LF1M", "lf3", "sw", "aq40", "dots"] {
            assert!(keep_allowed(t), "{t} is keep-allowed");
        }
        for t in ["cheap", "thx", "ty", "brb", "gz", "grats", "pls", "plz", "np", "omw", "idk", "nvm", "mp", "mats", "stockade", "stock", "armory", "lfx", "lfm2", "", "s"] {
            assert!(must_use(t), "{t} is must-use");
        }
        assert!(single_meaning("pas cher"));
        assert!(!single_meaning("invite / inventaire"));
    }
}
