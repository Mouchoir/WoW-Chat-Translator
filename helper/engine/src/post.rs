//! Output post-processing: think remnants, labels, quotes, length cap, placeholder check and restore, same-language detection.

/// Steps 1 to 4 of design D.5 post-processing. `input` is the protected input text (what the model
/// was asked to translate). Returns None for a bad output (ChatML remnant, or empty).
pub fn clean(raw: &str, input: &str) -> Option<String> {
    // 1. Think remnants; a ChatML token means the model broke out of its turn.
    if raw.contains("<|im_") {
        return None;
    }
    let mut s = strip_think(raw).replace("<|endoftext|>", "");
    // Mark characters (0x02-0x04) are ours: the model can never forge a coloured span.
    s.retain(|c| !tp_bridge::marks::is_mark(c));
    // 2. Join lines, trim.
    s = s.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ");
    // 3. Labels and wrapping quotes the input did not have (twice: `Translation: "..."`).
    for _ in 0..2 {
        s = strip_label(&s, input);
        s = strip_quotes(&s, input);
    }
    // 4. Length cap, then empty check.
    let cap = length_cap(input);
    if s.chars().count() > cap {
        s = s.chars().take(cap).collect::<String>().trim_end().to_string();
    }
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// What a wide character (Han, kana, Hangul, full-width) counts for in the length cap.
pub const WIDE_WEIGHT: usize = 3;

/// The longest output kept, in characters: 3 x the input's characters + 40, a wide character
/// counting `WIDE_WEIGHT` (2026-10-19, L4). A runaway answer is cut there; n_predict bounds it
/// too. Fact behind the weight: with every character counting 1, the French translations of two
/// synthetic Chinese yells (71 and 44 characters) were cut at exactly 253 and 172 characters,
/// before the IDs that ended them, as the in-game yell lost its two IDs; a Han character comes
/// out as up to about 6 Latin characters.
pub fn length_cap(input: &str) -> usize {
    let weighted: usize = input.chars().map(|c| if crate::verbatim::is_wide(c) { WIDE_WEIGHT } else { 1 }).sum();
    3 * weighted + 40
}

/// Removes `<think>...</think>` blocks and stray think tags.
fn strip_think(raw: &str) -> String {
    let mut s = raw.to_string();
    while let Some(a) = s.find("<think>") {
        match s[a..].find("</think>") {
            Some(rel) => s.replace_range(a..a + rel + "</think>".len(), ""),
            // Unclosed: everything after it is reasoning, not a translation.
            None => s.truncate(a),
        }
    }
    s.replace("</think>", "")
}

/// Labels a model may put before its answer (compared case-insensitively, colon variants included).
const LABELS: &[&str] = &[
    "translation:", "translation :", "translated:", "traduction:", "traduction :", "übersetzung:", "traducción:",
    "tradução:", "traduzione:", "перевод:", "번역:", "翻译:", "翻译：", "翻譯:", "翻譯：",
];

fn strip_label(s: &str, input: &str) -> String {
    let lower = s.to_lowercase();
    let input_lower = input.trim_start().to_lowercase();
    for label in LABELS {
        if lower.starts_with(label) && !input_lower.starts_with(label) {
            // Lowercasing can change byte lengths; cut by characters.
            let n = label.chars().count();
            return s.chars().skip(n).collect::<String>().trim().to_string();
        }
    }
    s.to_string()
}

const QUOTES: &[(char, char)] = &[('"', '"'), ('\u{201C}', '\u{201D}'), ('\u{AB}', '\u{BB}'), ('\'', '\''), ('\u{300C}', '\u{300D}'), ('\u{201E}', '\u{201C}')];

fn strip_quotes(s: &str, input: &str) -> String {
    let input = input.trim();
    for &(open, close) in QUOTES {
        if s.chars().count() >= 2 && s.starts_with(open) && s.ends_with(close) && !(input.starts_with(open) && input.ends_with(close)) {
            let inner: String = {
                let mut it = s.chars();
                it.next();
                it.next_back();
                it.collect()
            };
            // A quote inside means the outer pair is not a wrapper ("a" and "b").
            if open == close && inner.contains(open) {
                continue;
            }
            return inner.trim().trim_matches('\u{A0}').trim().to_string();
        }
    }
    s.to_string()
}

/// Casefolded letters and digits only.
fn fold(s: &str) -> Vec<char> {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Levenshtein distance over chars.
pub fn levenshtein(a: &[char], b: &[char]) -> usize {
    if a.is_empty() {
        return b.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(ca != cb);
            cur[j + 1] = sub.min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Step 6: the output is the input again (casefolded, punctuation and spaces removed, equal or a
/// Levenshtein ratio of at least 0.9).
pub fn is_same(input: &str, output: &str) -> bool {
    let a = fold(input);
    let b = fold(output);
    if a == b {
        return true;
    }
    let longest = a.len().max(b.len());
    if longest == 0 {
        return true;
    }
    // A length gap above 10% already rules out a ratio of 0.9.
    if a.len().abs_diff(b.len()) * 10 > longest {
        return false;
    }
    let ratio = 1.0 - levenshtein(&a, &b) as f64 / longest as f64;
    ratio >= 0.9
}

/// Writing systems the targets use (Kana only to tell Japanese apart).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Script {
    Latin,
    Cyrillic,
    Han,
    Kana,
    Hangul,
}

fn script_of(c: char) -> Option<Script> {
    if !c.is_alphabetic() {
        return None;
    }
    let u = c as u32;
    Some(if crate::protect::is_hangul(c) {
        Script::Hangul
    } else if (0x3040..=0x30FF).contains(&u) || (0x31F0..=0x31FF).contains(&u) {
        Script::Kana
    } else if (0x3400..=0x4DBF).contains(&u) || (0x4E00..=0x9FFF).contains(&u) || (0xF900..=0xFAFF).contains(&u) {
        Script::Han
    } else if (0x0400..=0x052F).contains(&u) {
        Script::Cyrillic
    } else if c.is_ascii_alphabetic() || (0x00C0..=0x024F).contains(&u) {
        Script::Latin
    } else {
        return None;
    })
}

/// The script of a target language.
pub fn target_script(target: &str) -> Script {
    match target {
        "zh" | "zh-TW" => Script::Han,
        "ko" => Script::Hangul,
        "ru" => Script::Cyrillic,
        _ => Script::Latin,
    }
}

/// True when most letters of `text` are in the target's script (and, for Chinese, no kana:
/// Japanese mixes kana with Han). Text without letters is false.
pub fn script_matches(text: &str, target: &str) -> bool {
    let mut n = [0usize; 5];
    for c in text.chars() {
        if let Some(s) = script_of(c) {
            n[s as usize] += 1;
        }
    }
    let total: usize = n.iter().sum();
    let want = target_script(target);
    if total == 0 || (want == Script::Han && n[Script::Kana as usize] > 0) {
        return false;
    }
    n[want as usize] * 2 > total
}

/// Frequent short words per Latin-script target, accents removed (see `fold_word`). Elided
/// French forms (l', qu', j') appear as their letters because words split at apostrophes.
const WORDS_EN: &[&str] = &[
    "the", "a", "an", "and", "or", "but", "if", "so", "to", "of", "in", "on", "at", "for", "with", "from", "by", "about",
    "is", "are", "was", "were", "be", "been", "am", "i", "im", "you", "u", "he", "she", "it", "we", "they", "me", "my",
    "your", "our", "their", "this", "that", "these", "those", "there", "here", "what", "where", "when", "who", "why",
    "how", "which", "can", "cant", "could", "will", "wont", "would", "should", "do", "does", "did", "dont", "doesnt",
    "not", "no", "yes", "yeah", "anyone", "anybody", "someone", "somebody", "any", "some", "all", "just", "need",
    "needs", "want", "wants", "have", "has", "had", "get", "got", "go", "going", "come", "know", "think", "please",
    "thanks", "thank", "sorry", "guys", "more", "one", "first", "then", "now", "still", "also", "only",
    "very", "really", "good", "well", "selling", "buying", "paying", "looking", "t", "s", "m", "ll", "re", "ve",
];
const WORDS_FR: &[&str] = &[
    "le", "la", "les", "l", "un", "une", "des", "du", "de", "d", "et", "ou", "mais", "donc", "car", "est", "sont", "es",
    "suis", "ai", "as", "avons", "avez", "ont", "a", "je", "j", "tu", "il", "elle", "on", "nous", "vous", "ils",
    "elles", "me", "m", "moi", "te", "t", "toi", "se", "s", "mon", "ma", "mes", "ton", "ta", "tes", "son", "sa",
    "ses", "notre", "votre", "leur", "ce", "c", "cet", "cette", "ces", "ca", "qui", "que", "qu", "quoi", "quand",
    "comment", "pourquoi", "avec", "pour", "par", "dans", "sur", "sous", "chez", "sans", "pas", "ne", "n", "plus",
    "tres", "bien", "merci", "salut", "bonjour", "bonsoir", "oui", "non", "si", "y", "en", "au", "aux", "tout",
    "tous", "toute", "toutes", "quelqu", "quelque", "faire", "fait", "fais", "peut", "veux", "veut", "besoin",
    "cherche", "groupe", "soir", "demain", "stp", "svp", "vends", "achete", "donjon", "premier",
];
const WORDS_DE: &[&str] = &[
    "der", "die", "das", "den", "dem", "des", "ein", "eine", "einen", "einem", "einer", "und", "oder", "aber", "ist",
    "sind", "bin", "bist", "war", "ich", "du", "er", "sie", "es", "wir", "ihr", "mich", "mir", "dich", "dir", "uns",
    "euch", "mein", "meine", "dein", "deine", "sein", "seine", "fur", "mit", "im", "in", "am", "an", "auf", "aus",
    "bei", "zu", "zum", "zur", "von", "vom", "nach", "nicht", "kein", "keine", "wer", "was", "wo", "wie", "warum",
    "jemand", "hat", "habe", "hab", "haben", "noch", "auch", "schon", "mal", "ja", "nein", "danke", "bitte", "gruppe",
    "suche", "suchen", "lust", "runde", "heute", "morgen", "gleich", "kann", "kannst", "will", "brauche", "brauchen",
    "gut", "zuerst", "dann",
];
const WORDS_ES: &[&str] = &[
    "el", "la", "los", "las", "lo", "un", "una", "unos", "unas", "y", "o", "pero", "es", "son", "soy", "eres", "esta",
    "estoy", "yo", "tu", "ella", "nosotros", "ellos", "me", "mi", "te", "ti", "se", "su", "sus", "nos", "para", "por",
    "con", "en", "de", "del", "al", "a", "que", "quien", "como", "donde", "cuando", "porque", "alguien", "algo",
    "nadie", "no", "si", "hola", "gracias", "todos", "todo", "quiere", "quiero", "puede", "hacer", "busco",
    "buscamos", "falta", "grupo", "conmigo", "contigo", "mazmorra", "hay", "vamos", "ya", "muy", "bien",
];
const WORDS_IT: &[&str] = &[
    "il", "lo", "la", "i", "gli", "le", "un", "una", "uno", "e", "o", "ma", "sono", "sei", "io", "tu", "lui", "lei",
    "noi", "voi", "loro", "mi", "ti", "si", "ci", "mio", "mia", "tuo", "tua", "suo", "sua", "per", "con", "in", "di",
    "da", "del", "della", "che", "chi", "come", "dove", "quando", "perche", "qualcuno", "qualcosa", "nessuno", "non",
    "ciao", "grazie", "tutti", "tutto", "vuole", "voglio", "puo", "fare", "cerco", "cerchiamo", "manca", "gruppo", "a",
];
const WORDS_PT: &[&str] = &[
    "o", "a", "os", "as", "um", "uma", "uns", "umas", "e", "ou", "mas", "sao", "sou", "eu", "voce", "vc", "ele", "ela",
    "nos", "voces", "eles", "me", "te", "se", "meu", "minha", "seu", "sua", "para", "por", "pra", "com", "em", "de",
    "do", "da", "dos", "das", "no", "na", "nas", "que", "quem", "como", "onde", "quando", "porque", "alguem", "algo",
    "ninguem", "nao", "sim", "ola", "oi", "obrigado", "obrigada", "valeu", "todos", "tudo", "quer", "quero", "pode",
    "fazer", "procuro", "procurando", "falta", "grupo", "vamos", "ja", "muito", "bem",
];

const LATIN_LANGS: &[(&str, &[&str])] = &[("en", WORDS_EN), ("fr", WORDS_FR), ("de", WORDS_DE), ("es", WORDS_ES), ("it", WORDS_IT), ("pt", WORDS_PT)];

/// Lowercase, Latin accents removed.
fn fold_word(w: &str) -> String {
    w.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'à' | 'â' | 'ä' | 'á' | 'ã' | 'å' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' | 'í' | 'ì' => 'i',
            'ô' | 'ö' | 'ó' | 'õ' | 'ò' => 'o',
            'ù' | 'û' | 'ü' | 'ú' => 'u',
            'ç' => 'c',
            'ñ' => 'n',
            'ß' => 's',
            c => c,
        })
        .collect()
}

/// Positive evidence that `text` (the protected message: placeholders are skipped as digits) is
/// already written in the target language. Words in `ignore` (glossary terms, lowercase) and
/// keep-allowed game abbreviations do not count. Non-Latin targets: the target script is the
/// evidence. Latin targets: the target's frequent words must outnumber those of every other
/// Latin-script language and make up at least a quarter of the counted words.
pub fn looks_like_target(text: &str, target: &str, ignore: &[String]) -> bool {
    if !script_matches(text, target) {
        return false;
    }
    if target_script(target) != Script::Latin {
        return true;
    }
    let want = match target {
        "en" | "fr" | "de" | "es" | "it" | "pt" => target,
        // Unknown Latin targets cannot come from the addon; no evidence.
        _ => return false,
    };
    let ignore: Vec<String> = ignore.iter().map(|t| fold_word(t.trim())).collect();
    let mut counted = 0usize;
    let mut hits = [0usize; 6];
    for w in text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()) {
        if w.chars().any(|c| c.is_ascii_digit()) {
            continue;
        }
        let f = fold_word(w);
        if ignore.contains(&f) || crate::prompt::keep_allowed(&f) {
            continue;
        }
        counted += 1;
        for (i, (_, words)) in LATIN_LANGS.iter().enumerate() {
            if words.contains(&f.as_str()) {
                hits[i] += 1;
            }
        }
    }
    let wi = LATIN_LANGS.iter().position(|(code, _)| *code == want).unwrap_or(0);
    let mine = hits[wi];
    let best_other = hits.iter().enumerate().filter(|(i, _)| *i != wi).map(|(_, &h)| h).max().unwrap_or(0);
    mine >= 1 && mine > best_other && mine * 4 >= counted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_table() {
        let cases: &[(&str, &str, Option<&str>)] = &[
            ("Bonjour à tous", "hello all", Some("Bonjour à tous")),
            ("  Bonjour\n\nà tous  ", "hello all", Some("Bonjour à tous")),
            ("<think>\nhmm\n</think>\n\nBonjour", "hello", Some("Bonjour")),
            ("</think>Bonjour", "hello", Some("Bonjour")),
            ("Bonjour <think>never closed", "hello", Some("Bonjour")),
            ("Bonjour<|im_end|>", "hello", None),
            ("<|im_start|>assistant", "hello", None),
            ("Bonjour<|endoftext|>", "hello", Some("Bonjour")),
            ("\"Bonjour\"", "hello", Some("Bonjour")),
            ("\"Bonjour\"", "\"hello\"", Some("\"Bonjour\"")),
            ("« Bonjour »", "hello", Some("Bonjour")),
            ("\u{201C}Hallo\u{201D}", "hello", Some("Hallo")),
            ("\"a\" et \"b\"", "a and b", Some("\"a\" et \"b\"")),
            ("Translation: Bonjour", "hello", Some("Bonjour")),
            ("Traduction : « Bonjour »", "hello", Some("Bonjour")),
            ("TRANSLATION: \"Bonjour\"", "hello", Some("Bonjour")),
            ("translation: is hard", "translation: is hard", Some("translation: is hard")),
            ("Перевод: Привет", "hello", Some("Привет")),
            ("翻译：你好", "hello", Some("你好")),
            ("", "hello", None),
            ("<think></think>", "hello", None),
            ("\"\"", "hello", None),
            ("Bon\u{2}\u{4}FF0000jour\u{3}", "hello", Some("BonFF0000jour")),
        ];
        for (raw, input, want) in cases {
            assert_eq!(clean(raw, input).as_deref(), *want, "raw={raw:?}");
        }
    }

    #[test]
    fn length_cap_rules() {
        let input = "hi"; // cap = 3*2 + 40 = 46
        let raw = "x".repeat(100);
        assert_eq!(clean(&raw, input).unwrap().chars().count(), 46);
        let raw = "é".repeat(100);
        assert_eq!(clean(&raw, input).unwrap().chars().count(), 46);
        // a wide character counts 3: a Chinese source of 71 characters (57 of them Han) keeps a
        // French answer of 400 characters (the old cap, 253, cut the IDs at its end)
        let zh = format!("{}加V {{1}}或{{2}}", "金".repeat(55));
        assert_eq!(zh.chars().count(), 65);
        assert_eq!(length_cap(&zh), 3 * (57 * 3 + 8) + 40);
        let fr = format!("{} ajoutez V {{1}} ou {{2}}", "x".repeat(380));
        assert_eq!(clean(&fr, &zh).as_deref(), Some(fr.as_str()));
        assert_eq!(length_cap("안녕"), 3 * 6 + 40);
        assert_eq!(length_cap("hello"), 55);
    }

    #[test]
    fn same_language() {
        assert!(is_same("salut tout le monde !", "Salut tout le monde."));
        assert!(is_same("anyone selling {1}? paying 5g a stack", "Anyone selling {1}? Paying 5g a stack."));
        // One letter changed in a long line: ratio above 0.9.
        assert!(is_same("quelqu'un pour faire le donjon ce soir", "quelqu'un pour faire le donjon ce soir?!"));
        assert!(is_same("quelqu un pour faire le donjon ce soir", "quelqu'un pour faire la donjon ce soir"));
        assert!(!is_same("anyone know where the portal is", "quelqu'un sait où est le portail"));
        assert!(!is_same("gg", "bien joué"));
        assert!(is_same("hello there", "hello where"), "1 edit over 10 letters is exactly 0.9");
        assert!(!is_same("hello there", "hello whale"));
    }

    #[test]
    fn scripts() {
        assert!(script_matches("wts {1} cheap", "fr"));
        assert!(!script_matches("wts {1} cheap, pst", "zh"));
        assert!(!script_matches("lfm {1} need tank", "ko"));
        assert!(script_matches("有人一起去刷{1}吗？缺坦克", "zh"));
        assert!(!script_matches("有人一起去刷{1}吗？缺坦克", "fr"));
        assert!(!script_matches("ダンジョン行きます", "zh"), "kana is Japanese");
        assert!(!script_matches("東京に行く", "zh"), "Han with kana is Japanese");
        assert!(script_matches("ищу группу в {1}, нужен танк", "ru"));
        assert!(script_matches("파티 구해요 {1} 탱커 있어요", "ko"));
        assert!(!script_matches("{1} 123 !!", "fr"), "no letters");
        assert_eq!(target_script("zh-TW"), Script::Han);
        assert_eq!(target_script("pt"), Script::Latin);
    }

    fn terms(t: &[&str]) -> Vec<String> {
        t.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn target_language_evidence() {
        // Lines already in the target language (E1 corpus "same" lines and the worker test).
        assert!(looks_like_target("salut tout le monde, quelqu'un pour faire le donjon ce soir ?", "fr", &[]));
        assert!(looks_like_target("salut tout le monde", "fr", &[]));
        assert!(looks_like_target("anyone selling {1}? paying 5g a stack", "en", &[]));
        assert!(looks_like_target("hat jemand Lust auf eine Runde im {1}?", "de", &[]));
        assert!(looks_like_target("hola a todos, ¿alguien quiere hacer la mazmorra conmigo?", "es", &[]));
        assert!(looks_like_target("ищу группу в {1}, нужен танк", "ru", &[]));
        assert!(looks_like_target("ok merci", "fr", &[]));
        // Lines the model echoed in E1 although they needed a translation.
        assert!(!looks_like_target("brb 5 min, need to feed the cat", "fr", &terms(&["brb"])));
        assert!(!looks_like_target("afk 10 min, dinner is ready", "fr", &terms(&["afk"])));
        assert!(!looks_like_target("vds {1} pas cher, mp moi", "en", &terms(&["mp"])));
        assert!(!looks_like_target("lf heal pour hc, pst", "en", &terms(&["lf", "pst"])));
        assert!(!looks_like_target("lfm {1} need healer, pst, thx", "de", &terms(&["lfm", "pst", "thx"])));
        assert!(!looks_like_target("wts {1} cheap, pst", "zh", &terms(&["wts", "pst"])));
        assert!(!looks_like_target("lfm {1} need tank, thx", "ko", &terms(&["lfm", "thx"])));
        assert!(!looks_like_target("thx for the group guys, gg", "fr", &terms(&["thx", "gg"])));
        assert!(!looks_like_target("merci pour le groupe, à demain", "de", &[]));
        assert!(!looks_like_target("suche Gruppe für die Todesminen, bin Heiler", "fr", &[]));
        // No words left to judge: no evidence.
        assert!(!looks_like_target("gg wp", "en", &[]));
        assert!(!looks_like_target("{1}", "en", &[]));
        assert!(!looks_like_target("hello there", "xx", &[]));
        // A tie is no evidence ("a" is French, Spanish, Italian, Portuguese and English).
        assert!(!looks_like_target("a", "fr", &[]));
        assert_eq!(fold_word("Für"), "fur");
        assert_eq!(fold_word("ÉTÉ"), "ete");
    }

    #[test]
    fn levenshtein_basics() {
        let c = |s: &str| s.chars().collect::<Vec<_>>();
        assert_eq!(levenshtein(&c("kitten"), &c("sitting")), 3);
        assert_eq!(levenshtein(&c(""), &c("abc")), 3);
        assert_eq!(levenshtein(&c("abc"), &c("")), 3);
        assert_eq!(levenshtein(&c("été"), &c("ete")), 2);
    }
}
