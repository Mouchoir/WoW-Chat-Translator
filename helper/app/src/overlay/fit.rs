//! Fitting a translation into the K reserved rows: wrap into at most K lines at the card's text
//! width at the full font size; if that fails, use the largest size down to 80% of it at which the
//! text fits; if it does not fit even at 80%, cut the last line at 80% and end it with "..."
//! (three dots).
//!
//! Sizes are in supersampled pixels (`fonts::SS` per pixel), so the shrink moves in steps of a third
//! of a pixel instead of whole pixels (80% of a 14 px chat font is 11.2 px, not 11 or 12).

pub const MIN_SCALE: f32 = 0.8;
pub const ELLIPSIS: &str = "...";

#[derive(Clone, Debug, PartialEq)]
pub struct Fitted {
    pub lines: Vec<String>,
    /// Font size the lines were wrapped at (same unit as the `size` given to [`fit`]).
    pub size: i32,
    pub cut: bool,
}

/// Smallest size the fit may use for a base size: 80% of it, rounded up so that it is never
/// below 80%.
pub fn min_size(size: i32) -> i32 {
    let size = size.max(1);
    // (size * 4 + 4) / 5 is ceil(0.8 * size) in integers
    ((size * 4 + 4) / 5).clamp(1, size)
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

/// Fits `text` into at most `max_lines` lines of `width` px. `measure(s, size)` is the width in
/// px of `s` at font size `size`; `size` is the base size (full size).
pub fn fit(text: &str, max_lines: usize, width: i32, size: i32, measure: &mut dyn FnMut(&str, i32) -> i32) -> Fitted {
    let max_lines = max_lines.max(1);
    let width = width.max(1);
    let size = size.max(1);
    let full = wrap(text, width, &mut |s| measure(s, size));
    if full.len() <= max_lines {
        return Fitted { lines: full, size, cut: false };
    }
    let min = min_size(size);
    let small = wrap(text, width, &mut |s| measure(s, min));
    if small.len() > max_lines {
        let mut lines: Vec<String> = small.into_iter().take(max_lines).collect();
        let last = lines.pop().unwrap_or_default();
        lines.push(ellipsize(&last, width, &mut |s| measure(s, min)));
        return Fitted { lines, size: min, cut: true };
    }
    // The largest size in [min, size) that fits: the line count only grows with the size, so a
    // binary search finds it with a few wraps (each wrap measures every candidate line). `best`
    // always holds the lines of `lo`, which is known to fit.
    let (mut lo, mut hi, mut best) = (min, size - 1, small);
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        let l = wrap(text, width, &mut |s| measure(s, mid));
        if l.len() <= max_lines {
            best = l;
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    Fitted { lines: best, size: lo, cut: false }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One character is `size` px wide (a monospace font whose width is its size).
    fn mono(s: &str, size: i32) -> i32 {
        s.chars().count() as i32 * size
    }

    #[test]
    fn short_text_one_line() {
        let f = fit("hello world", 2, 200, 10, &mut mono);
        assert_eq!(f, Fitted { lines: vec!["hello world".into()], size: 10, cut: false });
    }

    #[test]
    fn wraps_on_words() {
        let f = fit("the quick brown fox jumps", 3, 100, 10, &mut mono);
        assert_eq!(f.lines, vec!["the quick", "brown fox", "jumps"]);
        assert_eq!(f.size, 10);
    }

    #[test]
    fn min_size_is_never_below_80_percent() {
        assert_eq!(min_size(42), 34); // 33.6 rounded up
        assert_eq!(min_size(30), 24);
        assert_eq!(min_size(10), 8);
        assert_eq!(min_size(1), 1);
        for s in 1..200 {
            let m = min_size(s);
            assert!(m as f32 >= 0.8 * s as f32 - 1e-4 && m <= s, "{s} -> {m}");
            assert!(m == 1 || ((m - 1) as f32) < 0.8 * s as f32, "{s} -> {m} is not the smallest");
        }
    }

    #[test]
    fn shrinks_to_the_largest_size_that_fits() {
        // 22 characters, two lines of 110 px: 3 lines at 10 px; at 9 px "cccc dddd ee" (12
        // characters, 108 px) fits on the second line, so 9 is chosen (8 would fit too).
        let text = "aaaa bbbb cccc dddd ee";
        let f = fit(text, 2, 110, 10, &mut mono);
        assert_eq!((f.size, f.cut), (9, false), "{f:?}");
        assert_eq!(f.lines, vec!["aaaa bbbb", "cccc dddd ee"]);
        assert!(f.lines.iter().all(|l| mono(l, f.size) <= 110), "{f:?}");
    }

    #[test]
    fn shrink_stops_at_80_percent_then_cuts_with_three_dots() {
        // In supersampled units (30 = 10 px): fits only below 80%, so it is cut at 24.
        let f = fit("one two three four five six seven eight nine ten", 2, 300, 30, &mut mono);
        assert!(f.cut);
        assert_eq!(f.size, 24, "80% of 30");
        assert_eq!(f.lines.len(), 2);
        let last = f.lines.last().unwrap();
        assert!(last.ends_with("..."), "{last}");
        assert!(!last.contains('\u{2026}'));
        assert!(f.lines.iter().all(|l| mono(l, 24) <= 300), "{f:?}");
    }

    #[test]
    fn shrink_uses_fractional_steps() {
        // 30 = 10 px at 3x: a text that needs 27 (9 px) is fitted at 27, not cut and not 24.
        let text = "abcdefghij klmnopqrst"; // two words of 10 characters
        // two lines of 270 px: fits from size 27 down (10 * 27 = 270)
        let f = fit(text, 2, 270, 30, &mut mono);
        assert_eq!((f.size, f.cut), (27, false), "{f:?}");
        assert_eq!(f.lines, vec!["abcdefghij", "klmnopqrst"]);
    }

    #[test]
    fn long_words_and_cjk_break_between_characters() {
        let f = wrap("abcdefghijklmnop", 50, &mut |s| mono(s, 10));
        assert_eq!(f, vec!["abcde", "fghij", "klmno", "p"]);
        let cjk = wrap("你好世界你好世界", 40, &mut |s| mono(s, 10));
        assert_eq!(cjk, vec!["你好世界", "你好世界"]);
        let mixed = wrap("ok 你好世界", 40, &mut |s| mono(s, 10));
        assert_eq!(mixed, vec!["ok 你", "好世界"]);
    }

    #[test]
    fn whitespace_is_normalized() {
        assert_eq!(wrap("  a   b  ", 100, &mut |s| mono(s, 10)), vec!["a b"]);
        assert!(wrap("   ", 100, &mut |s| mono(s, 10)).is_empty());
        assert_eq!(fit("", 3, 100, 10, &mut mono).lines, Vec::<String>::new());
    }

    #[test]
    fn tiny_width_still_terminates() {
        let f = fit("abc def", 1, 1, 10, &mut mono);
        assert_eq!(f.lines.len(), 1);
        assert!(f.lines[0].ends_with("..."));
    }

    #[test]
    fn six_rows_are_used_before_shrinking() {
        let text = "aa bb cc dd ee ff";
        let f = fit(text, 6, 20, 10, &mut mono);
        assert_eq!((f.lines.len(), f.size, f.cut), (6, 10, false), "{f:?}");
    }
}
