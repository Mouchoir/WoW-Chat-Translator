//! One translation, model call excluded: pre-processing (protect, shortcuts, glossary expansion,
//! prompt, grammar) and post-processing (clean, placeholder check, same-language decision,
//! restore), plus the choice of the one follow-up request (Brackets or insist retry). The worker
//! and the E1 evaluation (`evalprompt`) share these functions, so E1 measures exactly what ships.

use crate::grammar;
use crate::post;
use crate::prompt::{self, Opts, Style, Variant};
use crate::protect::{self, Protected};

/// Translation-quality switches measured by E1b (docs/ENGINE.md section 10). `DEFAULT` is what
/// the engine ships; `evalprompt --tuning` tries the others.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tuning {
    /// V0 (`Plain`) or V1 (`Casual`) system prompt.
    pub style: Style,
    /// V2: when the output is the input again and the input has must-use glossary pairs or is
    /// not shown to be in the target language, ask once more with a stronger instruction.
    pub insist: bool,
    /// V3: `Same` only with positive evidence that the source is already in the target language
    /// (`post::looks_like_target`); otherwise the model output is shown as the translation.
    pub same_needs_evidence: bool,
    /// V4: must-use glossary terms with one meaning are replaced by that meaning in the message
    /// before the call (and left out of the prompt glossary).
    pub expand: bool,
}

impl Tuning {
    /// The E1 behaviour.
    pub const V0: Tuning = Tuning { style: Style::Plain, insist: false, same_needs_evidence: false, expand: false };
    /// What the engine uses: V1+V3+V4, the cheapest tuning that passed the E1b rule (V2 gave
    /// the same results with one more model call; docs/ENGINE.md section 10).
    pub const DEFAULT: Tuning = Tuning { style: prompt::DEFAULT_STYLE, insist: false, same_needs_evidence: true, expand: true };

    /// Parses "v0", "v1", "v0+v2", "v1+v2+v3+v4"... ("v1" selects the casual style; without it
    /// the style is plain). "default" gives `DEFAULT`.
    pub fn parse(s: &str) -> Option<Tuning> {
        if s == "default" {
            return Some(Tuning::DEFAULT);
        }
        let mut t = Tuning::V0;
        for part in s.split('+') {
            match part.trim() {
                "v0" => {}
                "v1" => t.style = Style::Casual,
                "v2" => t.insist = true,
                "v3" => t.same_needs_evidence = true,
                "v4" => t.expand = true,
                _ => return None,
            }
        }
        Some(t)
    }

    /// "v0", "v1+v2+v3"...
    pub fn label(&self) -> String {
        let mut parts = vec![if self.style == Style::Casual { "v1" } else { "v0" }];
        if self.insist {
            parts.push("v2");
        }
        if self.same_needs_evidence {
            parts.push("v3");
        }
        if self.expand {
            parts.push("v4");
        }
        parts.join("+")
    }
}

/// Which request of a job this is: at most one follow-up after the first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attempt {
    First,
    /// Placeholders came back broken: names inline in brackets.
    Brackets,
    /// The output was the input again (V2).
    Insist,
}

/// A message ready for the model.
pub struct Prepared {
    pub target: String,
    /// The model input (after the V4 expansion).
    pub protected: Protected,
    /// The protected text before any expansion, placeholders in place (what the player wrote).
    pub source: String,
    /// Neutralized single-line pairs (all of them).
    pub pairs: Vec<(String, String)>,
    /// The pairs the prompt lists (all of them minus the ones V4 expanded into the message); the
    /// remote providers send these.
    pub listed: Vec<(String, String)>,
    /// The local model's full ChatML prompt.
    pub prompt: String,
    /// The same request as a system message and a user message (no ChatML), for the remote LLM
    /// providers (docs/ENGINE.md section 13).
    pub system: String,
    pub user: String,
    pub grammar: &'static str,
    /// min(256, 32 + input bytes).
    pub n_predict: u32,
    pub tuning: Tuning,
    pub attempt: Attempt,
    /// Insist attempt: the first attempt's answer, kept when this one fails.
    pub fallback: Option<Finish>,
}

// One value per request, moved once: boxing the large variant would buy nothing.
#[allow(clippy::large_enum_variant)]
pub enum Prep {
    /// No letters outside placeholders: no model call.
    Nothing,
    /// Hangul text with target `ko`: no model call.
    Same,
    Ready(Prepared),
}

#[derive(Clone)]
pub enum Finish {
    /// Restored translation.
    Done(String),
    Same,
    /// Placeholders broken: ask again with `Variant::Brackets`.
    Retry,
    /// Output unchanged and not proven to be in the target language (V2): ask again with a
    /// stronger instruction. Carries the answer to give when that is not possible or fails.
    Insist(Box<Finish>),
    /// ChatML remnant or empty output.
    Bad,
}

/// What to do after a model answer.
#[allow(clippy::large_enum_variant)]
pub enum Step {
    Final(Finish),
    Again(Prepared),
}

/// Neutralized pairs; pairs that end up empty are dropped.
pub fn clean_pairs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(t, m)| (protect::clean_pair_text(t), protect::clean_pair_text(m)))
        .filter(|(t, m)| !t.is_empty() && !m.is_empty())
        .collect()
}

/// Pre-processing shortcuts that need no model (design D.5 steps 4 and 5).
pub fn shortcut(target: &str, p: &Protected) -> Option<Prep> {
    if p.plain_letters == 0 {
        return Some(Prep::Nothing);
    }
    if target == "ko" && p.plain_hangul * 2 > p.plain_letters {
        return Some(Prep::Same);
    }
    None
}

pub fn n_predict(protected: &str) -> u32 {
    (32 + protected.len()).min(256) as u32
}

/// True when `text[i..i + len]` stands alone (no letter or digit right before or after).
fn whole_word(text: &str, i: usize, len: usize) -> bool {
    let before = text[..i].chars().next_back();
    let after = text[i + len..].chars().next();
    !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
}

/// V4: replaces each must-use term that has one meaning by that meaning (whole words, ASCII
/// case ignored, like the addon's matching). Returns the new text and the expanded terms.
/// Meanings with braces are skipped so no placeholder can be forged.
pub fn expand(text: &str, pairs: &[(String, String)]) -> (String, Vec<String>) {
    let mut s = text.to_string();
    let mut done = Vec::new();
    for (term, meaning) in pairs {
        if term.is_empty() || !prompt::must_use(term) || !prompt::single_meaning(meaning) || meaning.contains(['{', '}']) {
            continue;
        }
        let t = term.as_bytes();
        let mut out = String::with_capacity(s.len() + meaning.len());
        let mut last = 0;
        let mut i = 0;
        let b = s.as_bytes();
        let mut hit = false;
        while i + t.len() <= b.len() {
            if s.is_char_boundary(i) && s.is_char_boundary(i + t.len()) && b[i..i + t.len()].eq_ignore_ascii_case(t) && whole_word(&s, i, t.len()) {
                out.push_str(&s[last..i]);
                out.push_str(meaning);
                i += t.len();
                last = i;
                hit = true;
            } else {
                i += 1;
            }
        }
        if hit {
            out.push_str(&s[last..]);
            s = out;
            done.push(term.clone());
        }
    }
    (s, done)
}

/// V4 for the Brackets form: the text between `[...]` segments is expanded, the segments (item
/// names, typed brackets) are left alone.
pub fn expand_outside_brackets(text: &str, pairs: &[(String, String)]) -> (String, Vec<String>) {
    let mut s = String::with_capacity(text.len() + 32);
    let mut done: Vec<String> = Vec::new();
    let mut last = 0;
    let tail = (text.len(), text.len());
    for (a, b) in bracket_segments(text).into_iter().chain(std::iter::once(tail)) {
        let (piece, terms) = expand(&text[last..a], pairs);
        s.push_str(&piece);
        s.push_str(&text[a..b]);
        for t in terms {
            if !done.contains(&t) {
                done.push(t);
            }
        }
        last = b;
    }
    (s, done)
}

fn make(target: &str, mut protected: Protected, source: String, pairs: Vec<(String, String)>, tuning: Tuning, attempt: Attempt, fallback: Option<Finish>) -> Prepared {
    let mut listed: Vec<&(String, String)> = pairs.iter().collect();
    if tuning.expand {
        let (text, expanded) = match protected.variant {
            Variant::Placeholders => expand(&protected.text, &pairs),
            Variant::Brackets => expand_outside_brackets(&protected.text, &pairs),
        };
        protected.text = text;
        listed.retain(|(t, _)| !expanded.contains(t));
    }
    let refs: Vec<(&str, &str)> = listed.iter().map(|(t, m)| (t.as_str(), m.as_str())).collect();
    let opts = Opts { style: tuning.style, insist: attempt == Attempt::Insist };
    let prompt = prompt::build_with(target, &protected.text, &refs, protected.variant, opts);
    let system = prompt::system_text(target, protected.variant, tuning.style);
    let user = prompt::user_text(target, &protected.text, &refs, protected.variant, opts);
    let listed: Vec<(String, String)> = listed.into_iter().cloned().collect();
    Prepared {
        target: target.to_string(),
        n_predict: n_predict(&protected.text),
        grammar: grammar::for_target(target),
        protected,
        source,
        listed,
        pairs,
        prompt,
        system,
        user,
        tuning,
        attempt,
        fallback,
    }
}

/// Prepares a message with the default tuning.
pub fn prepare(target: &str, marked: &str, pairs: &[(&str, &str)], v: Variant) -> Prep {
    prepare_with(target, marked, pairs, v, Tuning::DEFAULT)
}

pub fn prepare_with(target: &str, marked: &str, pairs: &[(&str, &str)], v: Variant, tuning: Tuning) -> Prep {
    let protected = protect::protect(marked, v);
    if let Some(s) = shortcut(target, &protected) {
        return s;
    }
    let source = protected.text.clone();
    Prep::Ready(make(target, protected, source, clean_pairs(pairs), tuning, Attempt::First, None))
}

/// The same message rebuilt for the Brackets retry (no expansion: names stay inline).
pub fn rebuild(p: &Prepared, marked: &str, v: Variant) -> Prepared {
    let protected = protect::protect(marked, v);
    let attempt = if v == Variant::Brackets { Attempt::Brackets } else { p.attempt };
    make(&p.target, protected, p.source.clone(), p.pairs.clone(), p.tuning, attempt, None)
}

/// The V2 retry: same variant, the insist instruction, `fallback` kept for a failure.
pub fn insist(p: &Prepared, marked: &str, fallback: Finish) -> Prepared {
    let protected = protect::protect(marked, p.protected.variant);
    make(&p.target, protected, p.source.clone(), p.pairs.clone(), p.tuning, Attempt::Insist, Some(fallback))
}

/// The `[...]` segments of `s`, as byte ranges (no nesting: each `[` pairs with the next `]`).
fn bracket_segments(s: &str) -> Vec<(usize, usize)> {
    let mut v = Vec::new();
    let mut from = 0;
    while let Some(a) = s[from..].find('[').map(|i| from + i) {
        let Some(b) = s[a + 1..].find(']').map(|i| a + 1 + i) else { break };
        v.push((a, b + 1));
        from = b + 1;
    }
    v
}

/// Brackets answers: a model sometimes translates an item name inside its brackets (E1b, en07:
/// a Korean link name came back in English). When the output has as many `[...]` segments as
/// the input, each segment that holds one of our spans gets the original text back.
pub fn repair_brackets(output: &str, p: &Protected) -> String {
    let input = bracket_segments(&p.text);
    let out = bracket_segments(output);
    if input.is_empty() || input.len() != out.len() {
        return output.to_string();
    }
    let originals: Vec<String> = p.spans.iter().zip(&p.wrapped).map(|(s, &w)| if w { format!("[{s}]") } else { s.clone() }).collect();
    let mut s = String::with_capacity(output.len() + 32);
    let mut last = 0;
    for (&(ia, ib), &(oa, ob)) in input.iter().zip(&out) {
        let want = &p.text[ia..ib];
        s.push_str(&output[last..oa]);
        s.push_str(if originals.iter().any(|o| o == want) { want } else { &output[oa..ob] });
        last = ob;
    }
    s.push_str(&output[last..]);
    s
}

/// Post-processing of one complete model output (design D.5 steps 1 to 6, with the E1b
/// same-language decision).
pub fn finish(p: &Prepared, raw: &str) -> Finish {
    let Some(mut s) = post::clean(raw, &p.protected.text) else {
        return p.fallback.clone().unwrap_or(Finish::Bad);
    };
    if p.protected.variant == Variant::Brackets {
        s = repair_brackets(&s, &p.protected);
    }
    if p.protected.variant == Variant::Placeholders && !protect::placeholders_valid(&s, p.protected.spans.len()) {
        // First attempt: Brackets retry. Insist attempt: keep the first answer.
        return p.fallback.clone().unwrap_or(Finish::Retry);
    }
    let restored = protect::restore(&s, &p.protected);
    let unchanged = post::is_same(&p.source, &s) || post::is_same(&p.protected.text, &s);
    if !unchanged {
        return Finish::Done(restored);
    }
    let t = p.tuning;
    let terms: Vec<String> = p.pairs.iter().map(|(t, _)| t.to_lowercase()).collect();
    let evidence = post::looks_like_target(&p.source, &p.target, &terms);
    let can_insist = t.insist && p.attempt == Attempt::First;
    if t.same_needs_evidence {
        if evidence {
            return Finish::Same;
        }
        let shown = Finish::Done(restored);
        return if can_insist { Finish::Insist(Box::new(shown)) } else { shown };
    }
    let has_must_use = p.pairs.iter().any(|(t, _)| prompt::must_use(t));
    if can_insist && (has_must_use || !evidence) {
        return Finish::Insist(Box::new(Finish::Same));
    }
    Finish::Same
}

/// The follow-up request after a first attempt (Brackets or insist retry), else the final
/// answer. A job never makes more than two model calls.
pub fn next(p: &Prepared, fin: Finish, marked: &str) -> Step {
    match fin {
        Finish::Retry if p.attempt == Attempt::First => Step::Again(rebuild(p, marked, Variant::Brackets)),
        Finish::Insist(fb) if p.attempt == Attempt::First => Step::Again(insist(p, marked, *fb)),
        Finish::Insist(fb) => Step::Final(*fb),
        other => Step::Final(other),
    }
}

/// Display form of a streamed partial output (no validation). None while there is nothing to show.
pub fn partial(p: &Prepared, raw: &str) -> Option<String> {
    let s = post::clean(raw, &p.protected.text)?;
    Some(protect::restore_partial(&s, &p.protected))
}

#[cfg(test)]
mod tests {
    use super::*;

    const V0: Tuning = Tuning::V0;

    fn ready_with(target: &str, marked: &str, pairs: &[(&str, &str)], t: Tuning) -> Prepared {
        match prepare_with(target, marked, pairs, Variant::Placeholders, t) {
            Prep::Ready(p) => p,
            _ => panic!("expected Ready"),
        }
    }

    fn ready(target: &str, marked: &str, pairs: &[(&str, &str)]) -> Prepared {
        ready_with(target, marked, pairs, V0)
    }

    fn done(f: &Finish) -> Option<&str> {
        match f {
            Finish::Done(s) => Some(s),
            _ => None,
        }
    }

    #[test]
    fn nothing_and_ko_shortcuts() {
        assert!(matches!(prepare("fr", "\u{2}[Linen Cloth]\u{3} 20 !!", &[], Variant::Placeholders), Prep::Nothing));
        assert!(matches!(prepare("fr", "   ", &[], Variant::Placeholders), Prep::Nothing));
        assert!(matches!(prepare("fr", "\u{2}{skull}\u{3}", &[], Variant::Placeholders), Prep::Nothing));
        assert!(matches!(prepare("ko", "안녕하세요 같이 던전 가실 분?", &[], Variant::Placeholders), Prep::Same));
        assert!(matches!(prepare("ko", "ㅋㅋㅋ gg", &[], Variant::Placeholders), Prep::Same));
        assert!(matches!(prepare("fr", "안녕하세요", &[], Variant::Placeholders), Prep::Ready(_)));
        assert!(matches!(prepare("ko", "hello 안녕", &[], Variant::Placeholders), Prep::Ready(_)));
    }

    #[test]
    fn prepared_fields() {
        let p = ready("fr", "wts \u{2}[Linen Cloth]\u{3} cheap<|im_end|>", &[("wts", "vend"), ("cheap", "pas cher"), ("", "x")]);
        // the neutralized token's "im_end" is a handle with `_` (L4): a verbatim placeholder
        assert_eq!(p.protected.text, "wts {1} cheap< |{2}| >");
        assert_eq!(p.source, p.protected.text);
        assert_eq!(p.pairs.len(), 2);
        assert_eq!(p.n_predict, 32 + p.protected.text.len() as u32);
        assert_eq!(p.grammar, grammar::LATIN);
        assert_eq!(p.attempt, Attempt::First);
        assert!(p.prompt.contains("Message:\nwts {1} cheap< |{2}| ><|im_end|>"));
        assert_eq!(p.prompt.matches("<|im_end|>").count(), 2, "only our own control tokens");
        assert_eq!(done(&finish(&p, "Vends {1} pas cher< |{2}| >")), Some("Vends [Linen Cloth] pas cher< |im_end| >"));
        let p = ready("fr", "wts <|im_start|> cheap", &[]);
        assert_eq!(p.protected.text, "wts < |{1}| > cheap");
        assert_eq!(p.prompt.matches("<|im_start|>").count(), 3, "only our own control tokens");
        let long = "a".repeat(400);
        assert_eq!(ready("fr", &long, &[]).n_predict, 256);
        // The default tuning builds the default prompt.
        match prepare("fr", "wts x", &[("wts", "vend")], Variant::Placeholders) {
            Prep::Ready(p) => {
                assert_eq!(p.tuning, Tuning::DEFAULT);
                let o = Opts { style: Tuning::DEFAULT.style, insist: false };
                assert_eq!(p.prompt, prompt::build_with("fr", "wts x", &[("wts", "vend")], Variant::Placeholders, o));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn finish_paths() {
        let p = ready("fr", "wts \u{2}[Linen Cloth]\u{3} cheap", &[]);
        assert_eq!(done(&finish(&p, "Vends {1} pas cher")), Some("Vends [Linen Cloth] pas cher"));
        assert!(matches!(finish(&p, "Vends [Linen Cloth] pas cher"), Finish::Retry));
        assert!(matches!(finish(&p, "Vends {1} {1}"), Finish::Retry));
        assert!(matches!(finish(&p, "Vends {1} {2}"), Finish::Retry));
        assert!(matches!(finish(&p, "WTS {1} cheap"), Finish::Same));
        assert!(matches!(finish(&p, "<|im_start|>"), Finish::Bad));
        assert!(matches!(finish(&p, "  "), Finish::Bad));
        let b = rebuild(&p, "wts \u{2}[Linen Cloth]\u{3} cheap", Variant::Brackets);
        assert_eq!(b.protected.text, "wts [Linen Cloth] cheap");
        assert_eq!(b.attempt, Attempt::Brackets);
        assert_eq!(b.source, "wts {1} cheap");
        assert_eq!(done(&finish(&b, "Vends [Linen Cloth] pas cher")), Some("Vends [Linen Cloth] pas cher"));
        // Brackets never asks for another retry.
        assert!(matches!(finish(&b, "Vends {1}"), Finish::Done(_)));
    }

    #[test]
    fn next_allows_one_follow_up() {
        let marked = "wts \u{2}[Linen Cloth]\u{3} cheap";
        let p = ready("fr", marked, &[]);
        let Step::Again(b) = next(&p, finish(&p, "Vends [Linen Cloth]"), marked) else { panic!() };
        assert_eq!(b.attempt, Attempt::Brackets);
        assert!(matches!(next(&b, Finish::Retry, marked), Step::Final(Finish::Retry)));
        assert!(matches!(next(&p, Finish::Same, marked), Step::Final(Finish::Same)));
        let Step::Again(i) = next(&p, Finish::Insist(Box::new(Finish::Same)), marked) else { panic!() };
        assert_eq!(i.attempt, Attempt::Insist);
        assert!(matches!(next(&i, Finish::Insist(Box::new(Finish::Same)), marked), Step::Final(Finish::Same)));
    }

    #[test]
    fn v2_insists_once_on_an_echo() {
        let t = Tuning { insist: true, ..V0 };
        let marked = "brb 5 min, need to feed the cat";
        let p = ready_with("fr", marked, &[("brb", "je reviens")], t);
        let fin = finish(&p, "brb 5 min, need to feed the cat.");
        assert!(matches!(&fin, Finish::Insist(fb) if matches!(**fb, Finish::Same)));
        let Step::Again(i) = next(&p, fin, marked) else { panic!() };
        assert!(i.prompt.contains("This message is not in French yet."));
        assert!(i.prompt.contains("Write \"je reviens\" for \"brb\"."));
        assert_eq!(done(&finish(&i, "Je reviens dans 5 min, je dois nourrir le chat.")), Some("Je reviens dans 5 min, je dois nourrir le chat."));
        // Still an echo after insisting: the legacy answer (Same) without V3.
        assert!(matches!(finish(&i, "brb 5 min, need to feed the cat"), Finish::Same));
        // A broken or empty retry keeps the first answer.
        assert!(matches!(finish(&i, ""), Finish::Same));
        // A line in the target language without must-use pairs: Same at once.
        let q = ready_with("fr", "salut tout le monde", &[], t);
        assert!(matches!(finish(&q, "Salut tout le monde !"), Finish::Same));
        // In the target language but a must-use pair was not applied: insist.
        let q = ready_with("fr", "brb dans 5 min, je dois partir", &[("brb", "je reviens")], t);
        assert!(matches!(finish(&q, "brb dans 5 min, je dois partir"), Finish::Insist(_)));
        // Keep-allowed pairs only, in the target language: Same.
        let q = ready_with("fr", "lfm pour le donjon, on cherche un tank", &[("lfm", "cherche des membres")], t);
        assert!(matches!(finish(&q, "lfm pour le donjon, on cherche un tank"), Finish::Same));
        // Wrong script: insist.
        let q = ready_with("zh", "wts \u{2}[Linen Cloth]\u{3} cheap, pst", &[("wts", "want to sell")], t);
        assert!(matches!(finish(&q, "wts {1} cheap, pst"), Finish::Insist(_)));
    }

    #[test]
    fn v3_same_needs_evidence() {
        let t = Tuning { same_needs_evidence: true, ..V0 };
        let p = ready_with("fr", "brb 5 min, need to feed the cat", &[("brb", "je reviens")], t);
        assert_eq!(done(&finish(&p, "brb 5 min, need to feed the cat")), Some("brb 5 min, need to feed the cat"));
        let p = ready_with("fr", "salut tout le monde, quelqu'un pour faire le donjon ce soir ?", &[], t);
        assert!(matches!(finish(&p, "Salut tout le monde, quelqu'un pour faire le donjon ce soir ?"), Finish::Same));
        let p = ready_with("en", "anyone selling \u{2}[Silk Cloth]\u{3}? paying 5g a stack", &[], t);
        assert!(matches!(finish(&p, "Anyone selling {1}? Paying 5g a stack."), Finish::Same));
        // V2 + V3: insist first, and the shown fallback is the echo itself.
        let t = Tuning { insist: true, same_needs_evidence: true, ..V0 };
        let marked = "afk 10 min, dinner is ready";
        let p = ready_with("fr", marked, &[("afk", "absent")], t);
        let fin = finish(&p, "afk 10 min, dinner is ready");
        assert!(matches!(&fin, Finish::Insist(fb) if done(fb) == Some("afk 10 min, dinner is ready")));
        let Step::Again(i) = next(&p, fin, marked) else { panic!() };
        assert_eq!(done(&finish(&i, "afk 10 min, dinner is ready")), Some("afk 10 min, dinner is ready"));
        assert_eq!(done(&finish(&i, "afk 10 min, le dîner est prêt")), Some("afk 10 min, le dîner est prêt"));
    }

    #[test]
    fn v4_expands_must_use_terms() {
        let pairs: Vec<(String, String)> = [("wts", "vend"), ("cheap", "pas cher"), ("inv", "invite / inventaire"), ("pls", "s'il te plaît"), ("x", "{1}")]
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect();
        let (s, e) = expand("wts {1} Cheap, cheapest cheap. inv me PLS x", &pairs);
        assert_eq!(s, "wts {1} pas cher, cheapest pas cher. inv me s'il te plaît x");
        assert_eq!(e, vec!["cheap".to_string(), "pls".to_string()]);
        let (s, e) = expand("été cheap", &[("été".into(), "summer".into())]);
        assert_eq!((s.as_str(), e.len()), ("summer cheap", 1));
        // Prompt: the message carries the meaning, the glossary lists only what was not expanded.
        let t = Tuning { expand: true, ..V0 };
        let p = ready_with("fr", "wts \u{2}[Linen Cloth]\u{3} cheap, pst", &[("wts", "vend"), ("cheap", "pas cher"), ("pst", "chuchotez-moi")], t);
        assert_eq!(p.protected.text, "wts {1} pas cher, pst");
        assert_eq!(p.source, "wts {1} cheap, pst");
        assert!(p.prompt.contains("Glossary:\n- \"wts\" = \"vend\"\n- \"pst\" = \"chuchotez-moi\"\n\nMessage:\nwts {1} pas cher, pst<|im_end|>"));
        // An output equal to the expanded input is an echo too.
        assert!(matches!(finish(&p, "wts {1} pas cher, pst"), Finish::Same));
        assert_eq!(done(&finish(&p, "Vends {1} pas cher, mp")), Some("Vends [Linen Cloth] pas cher, mp"));
        // Brackets retry: expanded outside the brackets only.
        let b = rebuild(&p, "wts \u{2}[Cheap Linen]\u{3} cheap, pst", Variant::Brackets);
        assert_eq!(b.protected.text, "wts [Cheap Linen] pas cher, pst");
        assert!(!b.prompt.contains("- \"cheap\""));
        let (s, e) = expand_outside_brackets("[cheap] cheap [x] cheap", &[("cheap".into(), "pas cher".into())]);
        assert_eq!((s.as_str(), e), ("[cheap] pas cher [x] pas cher", vec!["cheap".to_string()]));
    }

    #[test]
    fn brackets_repair_puts_item_names_back() {
        let marked = "파티 구해요 \u{2}[검은바위 나락]\u{3} 탱커 있어요";
        let p = ready("en", marked, &[]);
        let b = rebuild(&p, marked, Variant::Brackets);
        assert_eq!(done(&finish(&b, "Looking for a party [Blackrock Spire] - Tank here.")), Some("Looking for a party [검은바위 나락] - Tank here."));
        // {skull} was wrapped by us: repaired, then unwrapped.
        let marked = "mark \u{2}{skull}\u{3} then [x] \u{2}[Linen Cloth]\u{3}";
        let b = rebuild(&ready("fr", marked, &[]), marked, Variant::Brackets);
        assert_eq!(b.protected.text, "mark [{skull}] then [x] [Linen Cloth]");
        assert_eq!(done(&finish(&b, "marque [{crâne}] puis [y] [Étoffe de lin]")), Some("marque {skull} puis [y] [Linen Cloth]"));
        // Another number of segments: left as the model wrote it.
        assert_eq!(done(&finish(&b, "marque [{crâne}] puis [Étoffe de lin]")), Some("marque [{crâne}] puis [Étoffe de lin]"));
        assert_eq!(bracket_segments("a [b] c [d"), vec![(2, 5)]);
    }

    #[test]
    fn tuning_labels() {
        for s in ["v0", "v1", "v0+v2", "v1+v2+v3", "v1+v2+v3+v4", "v0+v4"] {
            assert_eq!(Tuning::parse(s).unwrap().label(), s);
        }
        assert_eq!(Tuning::parse("default"), Some(Tuning::DEFAULT));
        assert_eq!(Tuning::parse("v9"), None);
        assert_eq!(Tuning::DEFAULT.style, prompt::DEFAULT_STYLE, "warm-up prefix and requests agree");
        assert_eq!(Tuning::DEFAULT.label(), "v1+v3+v4", "E1b decision");
    }

    #[test]
    fn coloured_links_and_verbatim_tokens_through_the_pipeline() {
        // L3: the colour comes back as a coloured mark in Done, Partial and the Brackets retry
        let marked = "wts \u{2}\u{4}0070DD[Necromancer Leggings]\u{3}, add Kiwi48213";
        let p = ready_with("fr", marked, &[], Tuning::DEFAULT);
        assert_eq!(p.protected.text, "wts {1}, add {2}");
        assert_eq!(done(&finish(&p, "Vends {1}, ajoute {2}")), Some("Vends \u{2}\u{4}0070DD[Necromancer Leggings]\u{3}, ajoute Kiwi48213"));
        assert_eq!(partial(&p, "Vends {1}, aj").as_deref(), Some("Vends \u{2}\u{4}0070DD[Necromancer Leggings]\u{3}, aj"));
        // the model cannot forge a mark
        assert_eq!(done(&finish(&p, "Vends {1}\u{2}\u{4}FF0000x\u{3}, ajoute {2}")), Some("Vends \u{2}\u{4}0070DD[Necromancer Leggings]\u{3}FF0000x, ajoute Kiwi48213"));
        // L4: a dropped ID breaks the placeholders, the Brackets retry keeps it inline
        let Step::Again(b) = next(&p, finish(&p, "Vends {1}, ajoute"), marked) else { panic!() };
        assert_eq!(b.protected.text, "wts [Necromancer Leggings], add [Kiwi48213]");
        assert_eq!(
            done(&finish(&b, "Vends [Necromancer Leggings], ajoute [Kiwi48213]")),
            Some("Vends \u{2}\u{4}0070DD[Necromancer Leggings]\u{3}, ajoute Kiwi48213")
        );
        // the source with every span protected decides "unchanged" (an ID-only echo is Same
        // only with evidence)
        let q = ready_with("en", "anyone selling \u{2}[Silk Cloth]\u{3}? add Kiwi48213", &[], Tuning::DEFAULT);
        assert!(matches!(finish(&q, "Anyone selling {1}? Add {2}."), Finish::Same));
        // a line made only of IDs and URLs has nothing to translate
        assert!(matches!(prepare("fr", "Kiwi48213 discord.gg/AbC12", &[], Variant::Placeholders), Prep::Nothing));
    }

    #[test]
    fn partial_restores_known_placeholders() {
        let p = ready("fr", "wts \u{2}[Linen Cloth]\u{3}", &[]);
        assert_eq!(partial(&p, "Vends {1").as_deref(), Some("Vends {1"));
        assert_eq!(partial(&p, "Vends {1} x").as_deref(), Some("Vends [Linen Cloth] x"));
        assert_eq!(partial(&p, ""), None);
    }
}
