//! Fitting a translation into the K reserved rows (design D.3 "Fit"): wrap into at most K lines
//! at the card's text width; if that fails, retry at 85% of the font size; then cut the last line
//! and end it with "..." (three dots).

pub const SHRINK: f32 = 0.85;
pub const ELLIPSIS: &str = "...";

#[derive(Clone, Debug, PartialEq)]
pub struct Fitted {
    pub lines: Vec<String>,
    /// 1.0 or `SHRINK`.
    pub scale: f32,
    pub cut: bool,
}

/// Characters after which a line may break without a space (CJK ideographs, kana, fullwidth).
fn breaks_anywhere(c: char) -> bool {
    matches!(c as u32,
        0x2E80..=0x9FFF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF | 0x20000..=0x2FFFF)
}

/// Splits into tokens: runs of spaces, single CJK characters, and words.
fn tokens(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut kind = 0u8; // 0 none, 1 space, 2 word
    for (i, c) in text.char_indices() {
        let k = if c.is_whitespace() {
            1
        } else if breaks_anywhere(c) {
            3
        } else {
            2
        };
        if k == 3 {
            if kind != 0 && start < i {
                out.push(&text[start..i]);
            }
            out.push(&text[i..i + c.len_utf8()]);
            start = i + c.len_utf8();
            kind = 0;
            continue;
        }
        if k != kind {
            if kind != 0 && start < i {
                out.push(&text[start..i]);
            }
            start = i;
            kind = k;
        }
    }
    if kind != 0 && start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// Greedy word wrap at `width` px. Words longer than a line are broken between characters.
pub fn wrap(text: &str, width: i32, measure: &mut dyn FnMut(&str) -> i32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for tok in tokens(text.trim()) {
        if tok.chars().all(char::is_whitespace) {
            if !cur.is_empty() {
                cur.push(' ');
            }
            continue;
        }
        let candidate = format!("{cur}{tok}");
        if measure(&candidate) <= width {
            cur = candidate;
            continue;
        }
        let trimmed = cur.trim_end().to_string();
        if !trimmed.is_empty() {
            lines.push(trimmed);
            cur = String::new();
        }
        if measure(tok) <= width {
            cur = tok.to_string();
            continue;
        }
        // A word wider than the line: break it between characters.
        for c in tok.chars() {
            let candidate = format!("{cur}{c}");
            if measure(&candidate) > width && !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
                cur.push(c);
            } else {
                cur = candidate;
            }
        }
    }
    let last = cur.trim_end();
    if !last.is_empty() {
        lines.push(last.to_string());
    }
    lines
}

/// Cuts `line` so that `line + "..."` fits in `width`.
fn ellipsize(line: &str, width: i32, measure: &mut dyn FnMut(&str) -> i32) -> String {
    let mut s: Vec<char> = line.trim_end().chars().collect();
    loop {
        let candidate: String = s.iter().collect::<String>().trim_end().to_string() + ELLIPSIS;
        if s.is_empty() || measure(&candidate) <= width {
            return candidate;
        }
        s.pop();
    }
}

/// Fits `text` into at most `max_lines` lines of `width` px. `measure(s, scale)` is the width in
/// px of `s` at `scale` x the base font size.
pub fn fit(text: &str, max_lines: usize, width: i32, measure: &mut dyn FnMut(&str, f32) -> i32) -> Fitted {
    let max_lines = max_lines.max(1);
    let width = width.max(1);
    let full = wrap(text, width, &mut |s| measure(s, 1.0));
    if full.len() <= max_lines {
        return Fitted { lines: full, scale: 1.0, cut: false };
    }
    let small = wrap(text, width, &mut |s| measure(s, SHRINK));
    if small.len() <= max_lines {
        return Fitted { lines: small, scale: SHRINK, cut: false };
    }
    let mut lines: Vec<String> = small.into_iter().take(max_lines).collect();
    let last = lines.pop().unwrap_or_default();
    lines.push(ellipsize(&last, width, &mut |s| measure(s, SHRINK)));
    Fitted { lines, scale: SHRINK, cut: true }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 10 px per character at scale 1 (8.5 px at 85%).
    fn mono(s: &str, scale: f32) -> i32 {
        (s.chars().count() as f32 * 10.0 * scale).round() as i32
    }

    #[test]
    fn short_text_one_line() {
        let f = fit("hello world", 2, 200, &mut mono);
        assert_eq!(f, Fitted { lines: vec!["hello world".into()], scale: 1.0, cut: false });
    }

    #[test]
    fn wraps_on_words() {
        let f = fit("the quick brown fox jumps", 3, 100, &mut mono);
        assert_eq!(f.lines, vec!["the quick", "brown fox", "jumps"]);
        assert_eq!(f.scale, 1.0);
    }

    #[test]
    fn shrinks_to_85_percent_when_needed() {
        // 22 chars: 220 px at 100%, 187 px at 85%: two lines of 110 px only fit at 85%.
        let text = "aaaa bbbb cccc dddd ee";
        let f = fit(text, 2, 110, &mut mono);
        assert_eq!(f.scale, SHRINK);
        assert!(!f.cut);
        assert_eq!(f.lines.len(), 2);
        assert!(f.lines.iter().all(|l| mono(l, SHRINK) <= 110), "{:?}", f.lines);
    }

    #[test]
    fn cuts_with_three_dots() {
        let f = fit("one two three four five six seven eight nine ten", 2, 100, &mut mono);
        assert!(f.cut);
        assert_eq!(f.lines.len(), 2);
        let last = f.lines.last().unwrap();
        assert!(last.ends_with("..."), "{last}");
        assert!(!last.contains('\u{2026}'));
        assert!(mono(last, SHRINK) <= 100);
    }

    #[test]
    fn long_words_and_cjk_break_between_characters() {
        let f = wrap("abcdefghijklmnop", 50, &mut |s| mono(s, 1.0));
        assert_eq!(f, vec!["abcde", "fghij", "klmno", "p"]);
        let cjk = wrap("你好世界你好世界", 40, &mut |s| mono(s, 1.0));
        assert_eq!(cjk, vec!["你好世界", "你好世界"]);
        let mixed = wrap("ok 你好世界", 40, &mut |s| mono(s, 1.0));
        assert_eq!(mixed, vec!["ok 你", "好世界"]);
    }

    #[test]
    fn whitespace_is_normalized() {
        assert_eq!(wrap("  a   b  ", 100, &mut |s| mono(s, 1.0)), vec!["a b"]);
        assert!(wrap("   ", 100, &mut |s| mono(s, 1.0)).is_empty());
        assert_eq!(fit("", 3, 100, &mut mono).lines, Vec::<String>::new());
    }

    #[test]
    fn tiny_width_still_terminates() {
        let f = fit("abc def", 1, 1, &mut mono);
        assert_eq!(f.lines.len(), 1);
        assert!(f.lines[0].ends_with("..."));
    }
}
