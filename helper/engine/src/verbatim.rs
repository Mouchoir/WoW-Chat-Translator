//! Tokens the model must copy verbatim that no link mark covers (user request L4, 2026-10-19):
//! URLs, e-mail addresses and IDs. In game, the French translation of a long Chinese yell ending
//! with two IDs ("加V <ID>或<ID>") stopped at "Ajoutez V" (the cause was the output length cap,
//! `post::length_cap`, docs/ENGINE.md E1c); such tokens now become placeholders like item names
//! (`protect`), so the model copies them exactly. The IDs in the tests are synthetic.
//!
//! - **URL**: `http://` or `https://` and what follows; `www.` and a domain; a domain followed by
//!   a path (`discord.gg/abc`, `twitch.tv/name`); a bare domain that is a known host
//!   (`KNOWN_HOSTS`: `discord.gg`, `t.me`, `bit.ly`...) or whose top-level domain is a common one
//!   (`KNOWN_TLDS`) with a first label of 4 characters or more (`example.com`); a top-level domain
//!   that is also a chat word (`WORDLIKE_TLDS`: `me`, `to`, `it`, `de`...) needs 3 labels, so
//!   `add.me`, `go.to`, `merci.de` and `lol.gg` stay words. Leading `([{<"'` and trailing
//!   `.,;:!?)]}>"'` stay outside.
//! - **E-mail**: `local@domain.tld`.
//! - **ID**: a run of letters, digits and `_` (scripts written without spaces, such as Chinese,
//!   Japanese and Korean, end it) that has at least one letter and at least 2 digits
//!   (`Kiwi48213`, `Moko77310`, `QQ123456`), or holds a `_` (`dark_knight`), or ends with a
//!   `#` and 3 to 6 digits (a BattleTag, `Name#1234`).
//! - **Full-width forms** (U+FF01-FF5E, typed by Chinese input methods): their letters and digits
//!   are ID characters and their ASCII-range punctuation counts in URLs and addresses, read as the
//!   ASCII character (`Ｋｉｗｉ４８２１３`, `ｗｗｗ．ｅｘａｍｐｌｅ．ｃｎ`); the token is protected as
//!   written (the Latin grammar blocks full-width forms, so the model could not copy it).
//!
//! Not protected (the model needs them to translate, and keeps numbers anyway): plain numbers;
//! times (`20h30`, `1h30m`, `8pm`, `20Uhr`, `20hs`, `20heures`, `20ч30`); levels and item levels
//! (`lvl60`, `L60`, `ilvl200`, `60lvl`, `niv60`, `nivel60`, `stufe60`, `лвл60`, `60лвл`, `ур60`);
//! amounts and units (`x20`, `20x`, `100g`, `50po`, `10k`, `10kkk`, `50кк`, `5g50`, `5g50s`,
//! `5po50pa`, `30min`, `2nd`, `3eme`, `25man`, `20шт`); raid sizes and brackets (`10m`, `lf10m`,
//! `25hc`, `10hm`, `icc25`, `icc10hm`, `3v3`); keys (`m15`, `key20`); ratings (`pvp1800`,
//! `rating2000`, `rio2500`); the glossary's keep-allowed abbreviations (`aq40`). See
//! `game_number` for the exact shapes (English, French, German, Spanish, Portuguese, Italian and
//! Russian shorthand).

use crate::prompt;

/// Top-level domains accepted for a bare domain (without `www.` or a path).
const KNOWN_TLDS: &[&str] = &[
    "com", "net", "org", "gg", "tv", "io", "me", "co", "info", "app", "dev", "xyz", "eu", "fr", "de", "es", "it", "ru", "cn", "kr",
    "tw", "jp", "uk", "be", "ch", "nl", "pl", "br", "ca", "us", "ly", "to",
];

/// Top-level domains that are also chat words or word endings (`add.me`, `go.to`, `so.it`,
/// `merci.de`): a bare domain with one of them needs 3 labels (`example.co.uk`) or to be a known
/// host.
const WORDLIKE_TLDS: &[&str] = &["me", "to", "it", "de", "be", "us", "co", "es", "fr", "eu", "ch", "nl", "pl", "ca", "uk", "jp", "kr", "tw", "ly", "br"];

/// Bare hosts protected whatever their length (links players paste in chat).
const KNOWN_HOSTS: &[&str] = &[
    "discord.gg", "discord.com", "t.me", "wa.me", "x.com", "qq.com", "vk.com", "ok.ru", "bit.ly", "goo.gl", "youtu.be", "twitch.tv",
    "wago.io", "raider.io",
];

/// Letters before the number in a game number (`lvl60`, `x20`, `icc25`, `key15`). A single letter
/// not listed counts too when the number has at most 3 digits (`v12`, `a10`).
const PREFIXES: &[&str] = &[
    "x", "l", "lv", "lvl", "lvls", "lev", "level", "ilvl", "ilv", "il", "gs", "ip", "lf", "lfm", "lfg", "t", "tier", "s", "season", "r",
    "rank", "p", "ph", "phase", "h", "n", "hc", "nm", "m", "mm", "k", "key", "keys", "d", "w", "wk", "week", "day", "wave", "act",
    "patch", "aq", "icc", "toc", "togc", "totc", "naxx", "uld", "ulduar", "voa", "os", "rs", "ony", "mc", "bwl", "zg", "bt", "ssc",
    "tk", "kara", "za", "zul", "swp", "ds", "bwd", "bot", "tot", "hof", "soo", "msv", "bg", "rbg", "rbgs", "arena", "cr", "mmr", "rio",
    "top", "cap", "ny",
    // other languages' level shorthand (fr niv, nv, niveau; es pt nivel, nível; it liv, livello; de
    // stufe; ru лвл, лв, ур, уровень, илвл, гс) and ratings (pvp1800, rating2000, elo, io, score)
    "niv", "nv", "niveau", "nivel", "nível", "liv", "livello", "stufe", "лвл", "лв", "ур", "уровень", "илвл", "гс", "pvp",
    "pve", "rating", "rat", "elo", "io", "score", "рейтинг",
];

/// Letters after the number (`100g`, `30min`, `2nd`, `3eme`, `25man`, `60lvl`). A single letter
/// always counts (`5k`, `10m`, `100w`, `100р`).
const SUFFIXES: &[&str] = &[
    "kk", "mil", "po", "pa", "pc", "gp", "gold", "silver", "copper", "hr", "hrs", "min", "mins", "mn", "sec", "secs", "st", "nd",
    "rd", "th", "er", "re", "eme", "ème", "ieme", "ième", "man", "men", "ppl", "pm", "am", "hc", "nm", "lvl", "lv", "lvls", "vs",
    "ilvl", "gs", "dps", "hps", "ms", "fps", "hz", "eur", "usd", "euro", "euros", "pts", "pt", "cr", "kills", "hp", "sp", "ap",
    "days", "ans", "yo", "mo", "mb", "gb",
    // other languages: levels (niv, nv, лвл, лв, ур), amounts (kkk, кк, ккк, шт pieces, stk,
    // pcs), raid modes (hm), times (uhr, hs, heure, heures, мин, сек, час, часа, часов) and ratings
    "niv", "nv", "лвл", "лв", "ур", "kkk", "кк", "ккк", "шт", "stk", "pcs", "hm", "uhr", "hs", "heure", "heures", "мин", "сек",
    "час", "часа", "часов", "io", "rio", "elo", "mmr", "rating",
];

/// Letters between two numbers (`20h30`, `3v3`, `2x2`, `1m30`, `5g50`, `5po50`, `20ч30`).
const MIDDLES: &[&str] = &["h", "v", "vs", "x", "m", "k", "min", "mn", "g", "s", "po", "pa", "ч", "uhr"];

/// Scripts written without spaces between words (Han, kana, Hangul, full-width forms): their
/// characters end a token, so an ID glued to Chinese text is still found (full-width letters and
/// digits excepted: they are an ID's own characters, `is_id_char`). `post::length_cap` counts
/// them 3.
pub fn is_wide(c: char) -> bool {
    matches!(c as u32, 0x1100..=0x11FF | 0x2E80..=0x9FFF | 0xA960..=0xA97F | 0xAC00..=0xD7FF | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFFEF | 0x20000..=0x3FFFF)
}

/// Full-width digits and Latin letters (U+FF10-FF19, FF21-FF3A, FF41-FF5A).
fn is_full_width_alnum(c: char) -> bool {
    matches!(c as u32, 0xFF10..=0xFF19 | 0xFF21..=0xFF3A | 0xFF41..=0xFF5A)
}

/// A full-width form of an ASCII character (U+FF01-FF5E) read as that character.
fn narrow(c: char) -> char {
    match c as u32 {
        u @ 0xFF01..=0xFF5E => char::from_u32(u - 0xFEE0).unwrap_or(c),
        _ => c,
    }
}

fn narrowed(s: &str) -> String {
    s.chars().map(narrow).collect()
}

fn is_id_char(c: char) -> bool {
    c == '_' || (c.is_alphanumeric() && (!is_wide(c) || is_full_width_alnum(c)))
}

/// An ASCII printable character, or its full-width form: the characters of URLs and addresses.
fn is_url_char(c: char) -> bool {
    c.is_ascii_graphic() || matches!(c as u32, 0xFF01..=0xFF5E)
}

/// The top-level domain of `host` (`a.b.tld`: at least two labels of ASCII letters, digits and
/// `-`; the last one letters only, at least 2).
fn domain_tld(host: &str) -> Option<&str> {
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() < 2 || labels.iter().any(|l| l.is_empty() || !l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')) {
        return None;
    }
    let tld = labels[labels.len() - 1];
    (tld.len() >= 2 && tld.bytes().all(|b| b.is_ascii_alphabetic())).then_some(tld)
}

/// `t` (already trimmed of surrounding punctuation) is a URL.
pub fn is_url(t: &str) -> bool {
    let l = t.to_ascii_lowercase();
    if let Some(rest) = l.strip_prefix("https://").or_else(|| l.strip_prefix("http://")) {
        return rest.bytes().any(|b| b.is_ascii_alphanumeric());
    }
    let (rest, www) = match l.strip_prefix("www.") {
        Some(r) => (r, true),
        None => (l.as_str(), false),
    };
    let (host, path) = match rest.find('/') {
        Some(k) => (&rest[..k], true),
        None => (rest, false),
    };
    let Some(tld) = domain_tld(host) else { return false };
    if www || path || KNOWN_HOSTS.contains(&host) {
        return true;
    }
    // a bare domain: a common top-level domain, and not a chat word joined by a dot
    let labels = host.split('.').count();
    let first = host.split('.').next().unwrap_or("");
    KNOWN_TLDS.contains(&tld) && (labels >= 3 || (!WORDLIKE_TLDS.contains(&tld) && first.chars().count() >= 4))
}

/// `t` is an e-mail address.
pub fn is_email(t: &str) -> bool {
    let Some((local, domain)) = t.split_once('@') else { return false };
    !local.is_empty()
        && local.bytes().all(|b| b.is_ascii_alphanumeric() || b"._%+-".contains(&b))
        && domain_tld(&domain.to_ascii_lowercase()).is_some()
}

/// The runs of a lowercase word: (is digits, text).
fn runs(w: &str) -> Vec<(bool, String)> {
    let mut v: Vec<(bool, String)> = Vec::new();
    for c in w.chars() {
        let d = c.is_ascii_digit();
        match v.last_mut() {
            Some((k, s)) if *k == d => s.push(c),
            _ => v.push((d, c.to_string())),
        }
    }
    v
}

fn unit(s: &str) -> bool {
    s.chars().count() == 1 || SUFFIXES.contains(&s)
}

/// A number the model needs to read (time, level, amount, size...), not an ID. `w` is lowercase.
pub fn game_number(w: &str) -> bool {
    let r = runs(w);
    let text = |i: usize| r[i].1.as_str();
    match r.iter().map(|(d, _)| *d).collect::<Vec<_>>().as_slice() {
        // lvl60, x20, icc25, v12 (a single letter with up to 3 digits)
        [false, true] => PREFIXES.contains(&text(0)) || (text(0).chars().count() == 1 && text(1).len() <= 3),
        // 100g, 30min, 2nd, 60lvl
        [true, false] => unit(text(1)),
        // lf10m, lvl60s
        [false, true, false] => PREFIXES.contains(&text(0)) && unit(text(2)),
        // 20h30, 3v3, 2x2
        [true, false, true] => MIDDLES.contains(&text(1)),
        // 1h30m, 2h30min
        [true, false, true, false] => MIDDLES.contains(&text(1)) && unit(text(3)),
        _ => false,
    }
}

/// `w` (a run of letters, digits and `_`, maybe with a `#digits` tail when `tag`) is an ID.
/// Full-width letters and digits count as their ASCII forms.
pub fn is_id(w: &str, tag: bool) -> bool {
    let w = &narrowed(w);
    let letters = w.chars().filter(|c| c.is_alphabetic()).count();
    if tag {
        return letters > 0;
    }
    if w.contains('_') {
        return w.chars().filter(|c| c.is_alphanumeric()).count() >= 2;
    }
    let digits = w.chars().filter(char::is_ascii_digit).count();
    if letters == 0 || digits < 2 || prompt::keep_allowed(w) {
        return false;
    }
    !game_number(&w.to_lowercase())
}

/// Byte ranges of the verbatim tokens of `s`, in order and never overlapping.
pub fn find(s: &str) -> Vec<(usize, usize)> {
    let b = s.as_bytes();
    let mut found: Vec<(usize, usize)> = Vec::new();
    // URLs and e-mail addresses: runs of printable ASCII and their full-width forms, read as
    // ASCII (any other character ends a run); the ranges are byte ranges of `s`.
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        if !is_url_char(chars[i].1) {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && is_url_char(chars[i].1) {
            i += 1;
        }
        let (mut a, mut e) = (start, i);
        while a < e && "([{<\"'".contains(narrow(chars[a].1)) {
            a += 1;
        }
        while e > a && ".,;:!?)]}>\"'".contains(narrow(chars[e - 1].1)) {
            e -= 1;
        }
        if a < e {
            let text: String = chars[a..e].iter().map(|&(_, c)| narrow(c)).collect();
            if is_url(&text) || is_email(&text) {
                let end = chars.get(e).map_or(s.len(), |&(k, _)| k);
                found.push((chars[a].0, end));
            }
        }
    }
    // IDs: runs of letters, digits and `_`, outside the URLs and addresses.
    let mut ids = Vec::new();
    let mut it = s.char_indices().peekable();
    while let Some((a, c)) = it.next() {
        if !is_id_char(c) {
            continue;
        }
        let mut e = a + c.len_utf8();
        while let Some(&(k, d)) = it.peek() {
            if !is_id_char(d) {
                break;
            }
            e = k + d.len_utf8();
            it.next();
        }
        // a BattleTag: `#` and 3 to 6 digits, then no letter or digit
        let mut tag = false;
        if b.get(e) == Some(&b'#') {
            let n = b[e + 1..].iter().take_while(|x| x.is_ascii_digit()).count();
            let after = s[e + 1 + n..].chars().next();
            if (3..=6).contains(&n) && !after.is_some_and(is_id_char) {
                tag = true;
                // skip the tail in the outer loop
                while let Some(&(k, _)) = it.peek() {
                    if k >= e + 1 + n {
                        break;
                    }
                    it.next();
                }
                e += 1 + n;
            }
        }
        if found.iter().any(|&(x, y)| a < y && x < e) {
            continue;
        }
        if is_id(&s[a..e], tag) {
            ids.push((a, e));
        }
    }
    found.extend(ids);
    found.sort_unstable();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(s: &str) -> Vec<&str> {
        find(s).into_iter().map(|(a, e)| &s[a..e]).collect()
    }

    #[test]
    fn the_in_game_yell() {
        // L4: the IDs at the end of a Chinese line, glued to the Han characters
        assert_eq!(tokens("加V Kiwi48213或Moko77310"), vec!["Kiwi48213", "Moko77310"]);
    }

    #[test]
    fn urls_and_addresses() {
        let s = "join discord.gg/AbC12 or https://example.com/a?b=1&c=2, see www.example.org. mail me at some.one+wow@example.co.uk!";
        assert_eq!(tokens(s), vec!["discord.gg/AbC12", "https://example.com/a?b=1&c=2", "www.example.org", "some.one+wow@example.co.uk"]);
        assert_eq!(tokens("(twitch.tv/somebody)"), vec!["twitch.tv/somebody"]);
        assert_eq!(tokens("HTTP://EXAMPLE.COM"), vec!["HTTP://EXAMPLE.COM"]);
        assert_eq!(tokens("discord.gg and example.com"), vec!["discord.gg", "example.com"]);
        assert_eq!(tokens("访问www.example.com领取"), vec!["www.example.com"]);
        // not URLs: abbreviations with dots, slashes between words, numbers
        for s in ["e.g. this", "i.e. that", "a.m. ok", "and/or", "w/e", "5g/stack", "v1.2.3", "1.5k/2k", "lol.ok", "etc.", "http://", "x@y", "@name"] {
            assert!(tokens(s).is_empty(), "{s}: {:?}", tokens(s));
        }
    }

    #[test]
    fn ids_and_handles() {
        for (s, want) in [
            ("add Moko77310 on bnet", "Moko77310"),
            ("QQ123456789", "QQ123456789"),
            ("my tag is Name#1234.", "Name#1234"),
            ("Jäy#12345", "Jäy#12345"),
            ("dark_knight", "dark_knight"),
            ("xX_Sn1per_Xx!", "xX_Sn1per_Xx"),
            ("abc12def", "abc12def"),
            ("V123456", "V123456"),
            ("wx: abc_123", "abc_123"),
        ] {
            assert_eq!(tokens(s), vec![want], "{s}");
        }
        assert_eq!(tokens("Bob22, Ann33 and Jo44"), vec!["Bob22", "Ann33", "Jo44"]);
    }

    #[test]
    fn numbers_times_levels_and_units_stay_plain() {
        for s in [
            "48213", "20h30", "20H30", "1h30m", "2h30min", "8pm", "20:30", "lvl60", "LVL60", "L60", "lv70", "level60", "ilvl200",
            "60lvl", "gs4500", "x20", "20x", "2x2", "100g", "250po", "50pa", "10k", "100kk", "30min", "10mins", "2nd", "21st",
            "100th", "3eme", "2ème", "25man", "10m", "lf10m", "lf2m", "25hc", "10nm", "icc25", "toc10", "3v3", "10v10", "m15",
            "M15", "key20", "aq40", "aq20", "100w", "100р", "v12", "a10", "#1234", "wts x20 5g a stack", "lf1m dps 60+",
            "on part à 20h30, lvl 60 min", "1000金", "x", "_", "5k/10k",
        ] {
            assert!(tokens(s).is_empty(), "{s}: {:?}", tokens(s));
        }
        assert!(game_number("lf10m") && game_number("20h30") && !game_number("kiwi48213"));
        assert!(!is_id("Jay5", false), "one digit");
        assert!(!is_id("12345", false), "no letter");
    }

    #[test]
    fn other_languages_shorthand_stays_plain() {
        // integration review: levels, prices, raid sizes, times and ratings typed in other
        // languages' shorthand are numbers the model translates, not IDs
        for s in [
            "niv60", "Niv60", "nv60", "nivel60", "nível60", "stufe60", "liv60", "лвл60", "60лвл", "ур60", "60ур", "уровень60",
            "5g50", "5g50s", "10g50", "1g20s", "2g50", "5po50pa", "5po50", "50кк", "10kkk", "100ккк", "20шт", "10hm", "25hm",
            "icc10hm", "20Uhr", "20uhr30", "20hs", "20heures", "20ч30", "pvp1800", "rating2000", "rio2500", "io2500", "elo1500",
            "нужен хил в рейд, только 60лвл и выше", "wts stack of cloth 2g50 each", "raid ce soir à 20heures, icc10hm",
        ] {
            assert!(tokens(s).is_empty(), "{s}: {:?}", tokens(s));
        }
        // IDs keep their shape
        assert_eq!(tokens("add Niv60 or Kiwi48213"), vec!["Kiwi48213"]);
        assert_eq!(tokens("JayJay58026 ou Jay55732"), vec!["JayJay58026", "Jay55732"]);
    }

    #[test]
    fn chat_words_joined_by_a_dot_are_not_domains() {
        for s in ["add.me", "pm.me", "inv.me", "go.to", "so.it", "merci.de", "lol.gg", "ty.gg", "wts 2g50 each, add.me", "ok.com", "lol.tv"] {
            assert!(tokens(s).is_empty(), "{s}: {:?}", tokens(s));
        }
        // known hosts, long enough first labels, three labels, www., a scheme or a path still are
        for (s, want) in [
            ("t.me", "t.me"),
            ("bit.ly", "bit.ly"),
            ("raider.io", "raider.io"),
            ("goldshop.com", "goldshop.com"),
            ("example.co.uk", "example.co.uk"),
            ("www.ab.de", "www.ab.de"),
            ("http://go.to", "http://go.to"),
            ("add.me/x", "add.me/x"),
            ("t.me/somechan", "t.me/somechan"),
        ] {
            assert_eq!(tokens(s), vec![want], "{s}");
        }
    }

    #[test]
    fn full_width_ids_and_urls() {
        // typed with a Chinese input method: protected as written
        assert_eq!(tokens("金币出售，诚信经营，加V：Ｋｉｗｉ４８２１３"), vec!["Ｋｉｗｉ４８２１３"]);
        assert_eq!(tokens("加Ｖ　ＱＱ１２３４５６７"), vec!["ＱＱ１２３４５６７"]);
        assert_eq!(tokens("网址ｗｗｗ．ｅｘａｍｐｌｅ．ｃｎ，欢迎"), vec!["ｗｗｗ．ｅｘａｍｐｌｅ．ｃｎ"]);
        assert_eq!(tokens("邮箱ｓｏｍｅ＠ｅｘａｍｐｌｅ．ｃｏｍ"), vec!["ｓｏｍｅ＠ｅｘａｍｐｌｅ．ｃｏｍ"]);
        // full-width game numbers and plain numbers stay plain; Han ends a run
        for s in ["５０级", "ｌｖｌ６０", "２０ｈ３０", "１２３４５", "ｘ２０"] {
            assert!(tokens(s).is_empty(), "{s}: {:?}", tokens(s));
        }
        // the length cap still counts every full-width form as wide
        assert!(is_wide('Ｋ') && is_wide('，') && is_wide('金') && !is_wide('K'));
    }

    #[test]
    fn ranges_are_ordered_and_disjoint() {
        let s = "Moko77310 https://x.example.com/Moko77310 Bob22@mail.example.com Ann33";
        let f = find(s);
        assert_eq!(tokens(s), vec!["Moko77310", "https://x.example.com/Moko77310", "Bob22@mail.example.com", "Ann33"]);
        assert!(f.windows(2).all(|w| w[0].1 <= w[1].0), "{f:?}");
        assert!(find("").is_empty());
    }
}
