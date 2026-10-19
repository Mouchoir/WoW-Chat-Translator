//! The marks inside a TRANSLATE text (docs/BRIDGE.md section 3, "Marks"), shared by the engine
//! (placeholders) and the app (coloured link names, clipboard text).
//!
//! The addon wraps each real link name and each `{...}` group in STX (0x02) ... ETX (0x03). Since
//! addon build 5 a link span may carry the link's colour: COLOR (0x04) and six hex digits RRGGBB
//! right after the STX.
//!
//! ```text
//! span = STX [COLOR hex6] name ETX
//! ```
//!
//! Reading rules (one grammar for every reader; `docs/BRIDGE.md` section 3a, reference
//! `tools/bridge/proto.py` `parse_marks`, vectors `tests/vectors/marks.json`):
//! - a STX inside an open span restarts the span (the text before it becomes plain);
//! - a stray ETX is dropped; an unterminated span is plain text; an empty span is dropped;
//! - COLOR followed by six hex digits (either case) right after a STX is the span's colour (the
//!   seven characters are not text); a COLOR anywhere else, or not followed by six hex digits, is
//!   dropped alone (the characters after it stay text). The addon writes only well-formed spans.
//!
//! The engine writes a translated span back as `STX COLOR RRGGBB name ETX` only when it had a
//! colour (`span`); spans without a colour come back as plain text.
//!
//! On the wire the addon sends no COLOR (an older Translate Companion would paint the digits as
//! text): the colours come in a COLOURS message (span index, RGB) right before the TRANSLATE,
//! and the scene writes them into the text with [`with_colours`] (docs/BRIDGE.md section 3b).

/// Start of a span.
pub const STX: char = '\u{2}';
/// End of a span.
pub const ETX: char = '\u{3}';
/// Start of a span's colour (six hex digits follow).
pub const COLOR: char = '\u{4}';

/// One piece of a marked text. Deliberately no Debug (chat text).
#[derive(Clone, PartialEq, Eq)]
pub enum Piece {
    Plain(String),
    /// A complete span: its text and the link colour, when it had one.
    Span { text: String, rgb: Option<[u8; 3]> },
}

/// True for the three mark characters.
pub fn is_mark(c: char) -> bool {
    matches!(c, STX | ETX | COLOR)
}

fn hex_val(c: char) -> Option<u8> {
    c.to_digit(16).map(|v| v as u8)
}

/// The colour after a COLOR at `chars[i]` (the six characters after it), if they are hex digits.
fn color_at(chars: &[char], i: usize) -> Option<[u8; 3]> {
    let h = chars.get(i + 1..i + 7)?;
    let mut v = [0u8; 6];
    for (k, c) in h.iter().enumerate() {
        v[k] = hex_val(*c)?;
    }
    Some([v[0] << 4 | v[1], v[2] << 4 | v[3], v[4] << 4 | v[5]])
}

/// Splits a marked text into plain pieces and complete spans (rules in the module comment).
/// Consecutive plain text is merged into one piece.
pub fn split(marked: &str) -> Vec<Piece> {
    let chars: Vec<char> = marked.chars().collect();
    let mut out = Vec::new();
    let mut plain = String::new();
    // an open span: its text and colour
    let mut span: Option<(String, Option<[u8; 3]>)> = None;
    // the previous character was a STX (the only place a colour may start)
    let mut after_stx = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let was_after_stx = std::mem::replace(&mut after_stx, false);
        match c {
            STX => {
                if let Some((open, _)) = span.take() {
                    plain.push_str(&open);
                }
                span = Some((String::new(), None));
                after_stx = true;
            }
            ETX => {
                if let Some((text, rgb)) = span.take() {
                    if !text.is_empty() {
                        if !plain.is_empty() {
                            out.push(Piece::Plain(std::mem::take(&mut plain)));
                        }
                        out.push(Piece::Span { text, rgb });
                    }
                }
            }
            COLOR => {
                // a colour only right after the STX; any other COLOR is dropped alone
                if was_after_stx {
                    if let (Some(rgb), Some((_, color))) = (color_at(&chars, i), span.as_mut()) {
                        *color = Some(rgb);
                        i += 6;
                    }
                }
            }
            _ => match span.as_mut() {
                Some((s, _)) => s.push(c),
                None => plain.push(c),
            },
        }
        i += 1;
    }
    if let Some((open, _)) = span {
        plain.push_str(&open);
    }
    if !plain.is_empty() {
        out.push(Piece::Plain(plain));
    }
    out
}

/// The text with every mark and colour code removed, span texts kept.
pub fn plain(marked: &str) -> String {
    if !marked.contains(is_mark) {
        return marked.to_string();
    }
    let mut s = String::with_capacity(marked.len());
    for p in split(marked) {
        match p {
            Piece::Plain(t) | Piece::Span { text: t, .. } => s.push_str(&t),
        }
    }
    s
}

/// `RRGGBB`, upper case.
pub fn hex(rgb: [u8; 3]) -> String {
    format!("{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])
}

/// A span as the engine writes it back: `STX COLOR RRGGBB text ETX` with a colour, else the text
/// itself (an uncoloured span needs no mark in a translation).
pub fn span(text: &str, rgb: Option<[u8; 3]>) -> String {
    match rgb {
        Some(c) => format!("{STX}{COLOR}{}{text}{ETX}", hex(c)),
        None => text.to_string(),
    }
}

/// The TRANSLATE text with the COLOURS of its spans written in: span k (from 0, in reading
/// order, as [`split`] finds them) gets `COLOR RRGGBB` after its STX when `colours` lists k (a
/// colour already in the text stays when it does not). Every span keeps its marks, so the engine
/// still protects the uncoloured ones (`{...}` groups); indexes past the last span are ignored.
/// The text is rebuilt from [`split`], so marks that are not spans are dropped, as every reader
/// drops them.
pub fn with_colours(marked: &str, colours: &[(u8, [u8; 3])]) -> String {
    if colours.is_empty() {
        return marked.to_string();
    }
    let mut out = String::with_capacity(marked.len() + 7 * colours.len());
    let mut k = 0usize;
    for p in split(marked) {
        match p {
            Piece::Plain(t) => out.push_str(&t),
            Piece::Span { text, rgb } => {
                let rgb = colours.iter().find(|(i, _)| usize::from(*i) == k).map(|(_, c)| *c).or(rgb);
                out.push(STX);
                if let Some(c) = rgb {
                    out.push(COLOR);
                    out.push_str(&hex(c));
                }
                out.push_str(&text);
                out.push(ETX);
                k += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(s: &str) -> Vec<(String, Option<[u8; 3]>)> {
        split(s)
            .into_iter()
            .filter_map(|p| match p {
                Piece::Span { text, rgb } => Some((text, rgb)),
                Piece::Plain(_) => None,
            })
            .collect()
    }

    #[test]
    fn spans_with_and_without_colour() {
        let s = "wts \u{2}\u{4}0070dd[Necromancer Leggings]\u{3} and \u{2}{skull}\u{3}";
        assert_eq!(spans(s), vec![("[Necromancer Leggings]".to_string(), Some([0x00, 0x70, 0xDD])), ("{skull}".to_string(), None)]);
        assert!(split(s) == vec![
            Piece::Plain("wts ".into()),
            Piece::Span { text: "[Necromancer Leggings]".into(), rgb: Some([0, 0x70, 0xDD]) },
            Piece::Plain(" and ".into()),
            Piece::Span { text: "{skull}".into(), rgb: None },
        ]);
        assert_eq!(plain(s), "wts [Necromancer Leggings] and {skull}");
        // upper and lower case hex
        assert_eq!(spans("\u{2}\u{4}FFd100x\u{3}"), vec![("x".to_string(), Some([255, 0xD1, 0]))]);
    }

    #[test]
    fn legacy_grammar_is_unchanged() {
        // a STX inside an open span restarts it, a stray ETX is dropped, unterminated is plain
        assert_eq!(spans("\u{2}a\u{2}b\u{3}c\u{3}"), vec![("b".to_string(), None)]);
        assert_eq!(plain("\u{2}a\u{2}b\u{3}c\u{3}"), "abc");
        assert_eq!(plain("buy \u{2}[Copper Ore] now"), "buy [Copper Ore] now");
        assert!(spans("buy \u{2}[Copper Ore] now").is_empty());
        assert_eq!(plain("a \u{3} b"), "a  b");
        assert!(spans("x\u{2}\u{3}y").is_empty());
        assert_eq!(plain("x\u{2}\u{3}y"), "xy");
        assert_eq!(plain("no marks at all"), "no marks at all");
    }

    #[test]
    fn a_colour_only_right_after_the_stx() {
        // not right after the STX: the COLOR is dropped alone, the digits stay text (3a)
        assert_eq!(spans("\u{2}ab\u{4}FF0000c\u{3}"), vec![("abFF0000c".to_string(), None)]);
        // outside a span
        assert_eq!(plain("a\u{4}FF0000b"), "aFF0000b");
        // a bad COLOR right after the STX does not let a later one become the colour
        assert_eq!(spans("\u{2}\u{4}ZZ\u{4}FF0000x\u{3}"), vec![("ZZFF0000x".to_string(), None)]);
        // fewer than six hex digits: only the COLOR goes
        assert_eq!(spans("\u{2}\u{4}FF00[x]\u{3}"), vec![("FF00[x]".to_string(), None)]);
        assert_eq!(plain("a\u{4}zz"), "azz");
        // a colour with nothing after it: empty span, dropped
        assert!(spans("\u{2}\u{4}FF0000\u{3}").is_empty());
        assert_eq!(plain("\u{2}\u{4}FF0000\u{3}"), "");
        // an unterminated coloured span is plain text without its colour
        assert_eq!(plain("x \u{2}\u{4}a335ee[Big Sword]"), "x [Big Sword]");
        // a second COLOR in the same span is dropped alone
        assert_eq!(spans("\u{2}\u{4}FF0000\u{4}00FF00y\u{3}"), vec![("00FF00y".to_string(), Some([255, 0, 0]))]);
        // at the very end
        assert_eq!(plain("abc\u{4}"), "abc");
        assert_eq!(plain("abc\u{4}12345"), "abc12345");
    }

    #[test]
    fn utf8_and_restart_with_colour() {
        let s = "求组\u{2}\u{4}FF8000[熔火之心]\u{3}，\u{2}\u{4}71D5FF[Étoffe de lin]\u{3}";
        assert_eq!(spans(s), vec![("[熔火之心]".to_string(), Some([255, 128, 0])), ("[Étoffe de lin]".to_string(), Some([0x71, 0xD5, 0xFF]))]);
        // a restart drops the colour of the abandoned span
        assert_eq!(spans("\u{2}\u{4}FF0000a\u{2}b\u{3}"), vec![("b".to_string(), None)]);
        assert_eq!(plain("\u{2}\u{4}FF0000a\u{2}b\u{3}"), "ab");
    }

    #[test]
    fn span_writer_round_trips() {
        assert_eq!(span("[Linen Cloth]", None), "[Linen Cloth]");
        let w = span("[Linen Cloth]", Some([0x1E, 0xFF, 0x00]));
        assert_eq!(w, "\u{2}\u{4}1EFF00[Linen Cloth]\u{3}");
        assert_eq!(spans(&w), vec![("[Linen Cloth]".to_string(), Some([0x1E, 0xFF, 0]))]);
        assert_eq!(hex([0, 0x70, 0xDD]), "0070DD");
        assert!(is_mark('\u{4}') && !is_mark('\u{5}'));
    }

    #[test]
    fn colours_are_written_into_their_spans() {
        let wire = "wts \u{2}[Lattice Leggings]\u{3} and \u{2}[Fireball]\u{3} \u{2}{skull}\u{3}";
        let merged = with_colours(wire, &[(0, [0, 0x70, 0xDD]), (1, [0x71, 0xD5, 0xFF])]);
        assert_eq!(merged, "wts \u{2}\u{4}0070DD[Lattice Leggings]\u{3} and \u{2}\u{4}71D5FF[Fireball]\u{3} \u{2}{skull}\u{3}");
        assert_eq!(spans(&merged), vec![
            ("[Lattice Leggings]".to_string(), Some([0, 0x70, 0xDD])),
            ("[Fireball]".to_string(), Some([0x71, 0xD5, 0xFF])),
            ("{skull}".to_string(), None),
        ]);
        // no colours: the text as it came; an index past the last span: ignored
        assert_eq!(with_colours(wire, &[]), wire);
        assert_eq!(with_colours("a \u{2}[x]\u{3}", &[(5, [1, 2, 3])]), "a \u{2}[x]\u{3}");
        // the index counts spans as the reader finds them (a stray ETX, an empty span and an
        // unterminated span are not spans) and the rebuilt text drops those marks
        assert_eq!(with_colours("\u{3}\u{2}\u{3}\u{2}[a]\u{3} \u{2}[b", &[(0, [255, 0, 0])]), "\u{2}\u{4}FF0000[a]\u{3} [b");
        // a colour already in the text stays unless COLOURS has one for that span
        assert_eq!(with_colours("\u{2}\u{4}00FF00[a]\u{3}\u{2}\u{4}00FF00[b]\u{3}", &[(1, [255, 0, 0])]), "\u{2}\u{4}00FF00[a]\u{3}\u{2}\u{4}FF0000[b]\u{3}");
        assert_eq!(plain(&merged), plain(wire));
    }
}
