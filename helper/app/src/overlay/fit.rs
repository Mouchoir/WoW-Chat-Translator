//! Fitting a translation into the K reserved rows: wrap into at most K lines at the card's text
//! width at the full font size; if that fails, use the largest size down to 80% of it at which the
//! text fits; if it does not fit even at 80%, cut the last line at 80% and end it with "..."
//! (three dots).
//!
//! Sizes are in supersampled pixels (`fonts::SS` per pixel), so the shrink moves in steps of a third
//! of a pixel instead of whole pixels (80% of a 14 px chat font is 11.2 px, not 11 or 12).
//!
//! Link colours (2026-10-19, user request L3): the text may hold the engine's coloured marks
//! (`tp_bridge::marks`, `STX COLOR RRGGBB name ETX`). They are removed before anything is
//! measured, and each fitted line reports which of its bytes keep a link colour
//! (`Fitted::runs`), so wrapping, the shrink and the cut work on the visible text only.

use tp_bridge::marks::{self, Piece};

pub const MIN_SCALE: f32 = 0.8;
pub const ELLIPSIS: &str = "...";

/// Bytes `start..end` of a fitted line are drawn in `rgb` (a link name's colour).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub start: usize,
    pub end: usize,
    pub rgb: [u8; 3],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Fitted {
    pub lines: Vec<String>,
    /// Font size the lines were wrapped at (same unit as the `size` given to [`fit`]).
    pub size: i32,
    pub cut: bool,
    /// Per line, its coloured runs (empty for a line without any; empty overall for a text
    /// without link colours).
    pub runs: Vec<Vec<Run>>,
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

/// The visible text, one colour per character (None: the card's text colour).
struct Styled {
    chars: Vec<char>,
    colors: Vec<Option<[u8; 3]>>,
}

impl Styled {
    fn parse(text: &str) -> Styled {
        let mut st = Styled { chars: Vec::new(), colors: Vec::new() };
        for p in marks::split(text) {
            let (t, rgb) = match p {
                Piece::Plain(t) => (t, None),
                Piece::Span { text, rgb } => (text, rgb),
            };
            for c in t.chars() {
                st.chars.push(c);
                st.colors.push(rgb);
            }
        }
        st
    }

    /// The character range left after trimming white space at both ends.
    fn trimmed(&self) -> (usize, usize) {
        let a = self.chars.iter().position(|c| !c.is_whitespace()).unwrap_or(self.chars.len());
        let e = self.chars.iter().rposition(|c| !c.is_whitespace()).map_or(a, |i| i + 1);
        (a, e.max(a))
    }
}

/// A line being built: its text and one colour per character.
#[derive(Clone, Default)]
struct Line {
    text: String,
    colors: Vec<Option<[u8; 3]>>,
}

impl Line {
    fn push(&mut self, c: char, rgb: Option<[u8; 3]>) {
        self.text.push(c);
        self.colors.push(rgb);
    }

    fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    fn trim_end(mut self) -> Line {
        while self.text.ends_with(char::is_whitespace) {
            self.pop();
        }
        self
    }

    fn pop(&mut self) {
        if self.text.pop().is_some() {
            self.colors.pop();
        }
    }

    /// The coloured byte runs of the line.
    fn runs(&self) -> Vec<Run> {
        let mut v: Vec<Run> = Vec::new();
        for ((i, c), rgb) in self.text.char_indices().zip(&self.colors) {
            let Some(rgb) = *rgb else { continue };
            let end = i + c.len_utf8();
            match v.last_mut() {
                Some(r) if r.end == i && r.rgb == rgb => r.end = end,
                _ => v.push(Run { start: i, end, rgb }),
            }
        }
        v
    }
}

/// Splits characters `a..e` into tokens: runs of spaces, single CJK characters, and words (as
/// character ranges).
fn tokens(chars: &[char], a: usize, e: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = a;
    let mut kind = 0u8; // 0 none, 1 space, 2 word
    for (i, &c) in chars.iter().enumerate().take(e).skip(a) {
        let k = if c.is_whitespace() {
            1
        } else if breaks_anywhere(c) {
            3
        } else {
            2
        };
        if k == 3 {
            if kind != 0 && start < i {
                out.push((start, i));
            }
            out.push((i, i + 1));
            start = i + 1;
            kind = 0;
            continue;
        }
        if k != kind {
            if kind != 0 && start < i {
                out.push((start, i));
            }
            start = i;
            kind = k;
        }
    }
    if kind != 0 && start < e {
        out.push((start, e));
    }
    out
}

/// Greedy word wrap of styled text at `width` px. Words longer than a line are broken between
/// characters.
fn wrap_styled(st: &Styled, width: i32, measure: &mut dyn FnMut(&str) -> i32) -> Vec<Line> {
    let (a, e) = st.trimmed();
    let mut lines: Vec<Line> = Vec::new();
    let mut cur = Line::default();
    for (ta, te) in tokens(&st.chars, a, e) {
        let tok: String = st.chars[ta..te].iter().collect();
        if tok.chars().all(char::is_whitespace) {
            if !cur.is_empty() {
                cur.push(' ', st.colors[ta]);
            }
            continue;
        }
        let candidate = format!("{}{tok}", cur.text);
        if measure(&candidate) <= width {
            for i in ta..te {
                cur.push(st.chars[i], st.colors[i]);
            }
            continue;
        }
        let trimmed = std::mem::take(&mut cur).trim_end();
        if !trimmed.is_empty() {
            lines.push(trimmed);
        }
        if measure(&tok) <= width {
            for i in ta..te {
                cur.push(st.chars[i], st.colors[i]);
            }
            continue;
        }
        // A word wider than the line: break it between characters.
        for i in ta..te {
            let candidate = format!("{}{}", cur.text, st.chars[i]);
            if measure(&candidate) > width && !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
            }
            cur.push(st.chars[i], st.colors[i]);
        }
    }
    let last = cur.trim_end();
    if !last.is_empty() {
        lines.push(last);
    }
    lines
}

/// Greedy word wrap at `width` px (link marks removed). Words longer than a line are broken
/// between characters.
pub fn wrap(text: &str, width: i32, measure: &mut dyn FnMut(&str) -> i32) -> Vec<String> {
    wrap_styled(&Styled::parse(text), width, measure).into_iter().map(|l| l.text).collect()
}

/// Cuts `line` so that `line + "..."` fits in `width` (the dots in the text colour).
fn ellipsize(line: Line, width: i32, measure: &mut dyn FnMut(&str) -> i32) -> Line {
    let mut l = line.trim_end();
    loop {
        let mut candidate = l.clone().trim_end();
        for c in ELLIPSIS.chars() {
            candidate.push(c, None);
        }
        if l.is_empty() || measure(&candidate.text) <= width {
            return candidate;
        }
        l.pop();
    }
}

fn fitted(lines: Vec<Line>, size: i32, cut: bool) -> Fitted {
    let runs: Vec<Vec<Run>> = lines.iter().map(Line::runs).collect();
    let runs = if runs.iter().all(Vec::is_empty) { Vec::new() } else { runs };
    Fitted { lines: lines.into_iter().map(|l| l.text).collect(), size, cut, runs }
}

/// Fits `text` into at most `max_lines` lines of `width` px. `measure(s, size)` is the width in
/// px of `s` at font size `size`; `size` is the base size (full size). The text may hold link
/// marks (removed; their colours are reported in `runs`).
pub fn fit(text: &str, max_lines: usize, width: i32, size: i32, measure: &mut dyn FnMut(&str, i32) -> i32) -> Fitted {
    let max_lines = max_lines.max(1);
    let width = width.max(1);
    let size = size.max(1);
    let st = Styled::parse(text);
    let full = wrap_styled(&st, width, &mut |s| measure(s, size));
    if full.len() <= max_lines {
        return fitted(full, size, false);
    }
    let min = min_size(size);
    let small = wrap_styled(&st, width, &mut |s| measure(s, min));
    if small.len() > max_lines {
        let mut lines: Vec<Line> = small.into_iter().take(max_lines).collect();
        let last = lines.pop().unwrap_or_default();
        lines.push(ellipsize(last, width, &mut |s| measure(s, min)));
        return fitted(lines, min, true);
    }
    // The largest size in [min, size) that fits: the line count only grows with the size, so a
    // binary search finds it with a few wraps (each wrap measures every candidate line). `best`
    // always holds the lines of `lo`, which is known to fit.
    let (mut lo, mut hi, mut best) = (min, size - 1, small);
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        let l = wrap_styled(&st, width, &mut |s| measure(s, mid));
        if l.len() <= max_lines {
            best = l;
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    fitted(best, lo, false)
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
        assert_eq!(f, Fitted { lines: vec!["hello world".into()], size: 10, cut: false, runs: Vec::new() });
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

    const BLUE: [u8; 3] = [0, 0x70, 0xDD];
    const PURPLE: [u8; 3] = [0xA3, 0x35, 0xEE];

    fn run(start: usize, end: usize, rgb: [u8; 3]) -> Run {
        Run { start, end, rgb }
    }

    #[test]
    fn link_colours_become_runs_and_are_never_measured() {
        // L3: the marks and colour codes are not text; the link name keeps its colour
        let text = "Vends \u{2}\u{4}0070DD[Linen Cloth]\u{3} pas cher";
        let f = fit(text, 1, 1000, 10, &mut |s, sz| {
            assert!(!s.contains(marks::is_mark) && !s.contains("0070DD"), "measured {s:?}");
            mono(s, sz)
        });
        assert_eq!(f.lines, vec!["Vends [Linen Cloth] pas cher"]);
        assert_eq!(f.runs, vec![vec![run(6, 19, BLUE)]]);
        assert_eq!(&f.lines[0][6..19], "[Linen Cloth]");
        // the same text without colour fits the same way
        let plain = fit("Vends [Linen Cloth] pas cher", 1, 1000, 10, &mut mono);
        assert_eq!((plain.lines, plain.size), (f.lines.clone(), f.size));
        assert!(plain.runs.is_empty());
    }

    #[test]
    fn coloured_runs_follow_wrapping_and_utf8() {
        // a link name wrapped over two lines keeps its colour on both; two links, two colours
        let text = "a \u{2}\u{4}0070DD[Épée de feu]\u{3} b \u{2}\u{4}A335EE[x]\u{3}";
        let f = fit(text, 3, 90, 10, &mut mono);
        assert_eq!(f.lines, vec!["a [Épée", "de feu] b", "[x]"]);
        assert_eq!(f.runs[0], vec![run(2, 9, BLUE)], "[Épée is 7 bytes from 2");
        assert_eq!(f.runs[1], vec![run(0, 7, BLUE)]);
        assert_eq!(f.runs[2], vec![run(0, 3, PURPLE)]);
        // the space inside a link name keeps the link colour; the one after it does not
        let g = fit("\u{2}\u{4}0070DD[a b]\u{3} c", 1, 1000, 10, &mut mono);
        assert_eq!(g.runs, vec![vec![run(0, 5, BLUE)]]);
    }

    #[test]
    fn the_cut_keeps_colours_and_adds_plain_dots() {
        // fits only below 80%: cut at 24 with "..." in the text colour
        let text = "one \u{2}\u{4}A335EE[two three]\u{3} four five six seven eight nine ten";
        let f = fit(text, 2, 300, 30, &mut mono);
        assert!(f.cut && f.size == 24, "{f:?}");
        assert_eq!(f.lines, vec!["one [two", "three] fo..."]);
        assert_eq!(f.runs, vec![vec![run(4, 8, PURPLE)], vec![run(0, 6, PURPLE)]]);
        // a cut inside a coloured name: the kept part is coloured, the dots are not
        let g = fit("\u{2}\u{4}A335EE[abcdefghijklmnop]\u{3}", 1, 100, 10, &mut mono);
        assert_eq!(g.lines, vec!["[abcdefgh..."]);
        assert_eq!(g.runs, vec![vec![run(0, 9, PURPLE)]]);
    }
}
