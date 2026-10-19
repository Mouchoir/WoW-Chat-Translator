//! Messages carried by the strip (bridge v2), rebuilt by `reasm` and parsed strictly here.
//!
//! All integers are big-endian; `str8` = u8 length + bytes, `str16` = u16 length + bytes.
//! ```text
//! 0x10 TRANSLATE reqId u16 | slot u8 (0..126) | rows u8 (1..6)
//!                | flags u8 (bit0 noCache, bit1 plate, bit2 reply, bit3 copy, bits 4-7 = 0)
//!                | target str8 (ASCII, 1..8) | text str16 (UTF-8, <= 1024 B, may hold 0x02/0x03/0x04 marks: `marks`)
//!                | nPairs u8 (<= 8) | pairs: term str8 (<= 48 B), meaning str8 (<= 120 B);
//!                  sum(2 + #term + #meaning) <= 512
//!                  copy needs reply; reply needs plate, rows 2..6 (row 0 = the Helper's status,
//!                  rows 1.. = the text) and a target that is a language code (letters and '-')
//! 0x11 CANCEL    reqId u16 | slot u8 (0..127)
//! 0x12 CLEAR     (empty)
//! 0x13 TEST      nonce u16
//! 0x14 VIEW      n u8 (0..4) | n x { id u8, x u16, y u16, w u16, h u16, fontPx u8, rowPx u8 }
//! 0x15 COLOURS   reqId u16 | n u8 (1..255) | n x { span u8, r u8, g u8, b u8 }
//!                  span: the index (from 0) of a marked span in the text of the TRANSLATE with
//!                  the same reqId, strictly increasing; sent right before that TRANSLATE
//!                  (addon build 5, docs/BRIDGE.md section 3b)
//! ```
//! A message whose length does not match exactly, or with a field out of range, is refused.

/// Chat text (a message, a translation or a glossary term). Deliberately has no Debug,
/// Display, Clone or Serialize, so it cannot be logged or copied into a log record by accident.
pub struct ChatText(String);

impl ChatText {
    pub fn new(s: String) -> Self {
        ChatText(s)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Length in bytes.
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

/// One chat clip rectangle and its font metrics, in physical px from the client origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub id: u8,
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    pub font_px: u8,
    pub row_px: u8,
}

/// A translation request for the reserved rows tagged with `slot`.
pub struct Translate {
    pub req_id: u16,
    pub slot: u8,
    pub rows: u8,
    pub no_cache: bool,
    pub plate: bool,
    /// The conversation window's reply card: row 0 carries the Helper's status, rows 1..rows-1
    /// the translation (the user's own text, to be pasted into the game).
    pub reply: bool,
    /// The user allowed the clipboard for this reply (only ever set with `reply`).
    pub copy: bool,
    pub target: String,
    /// Marked text: item links and `{...}` groups are wrapped in 0x02 ... 0x03. The addon sends
    /// the link colours apart (COLOURS); the scene writes them into the text as 0x04 RRGGBB after
    /// the 0x02 (`marks::with_colours`), the form the engine and the overlay read.
    pub text: ChatText,
    /// Glossary pairs (term, meaning) matched in this message.
    pub pairs: Vec<(ChatText, ChatText)>,
}

pub enum Message {
    Translate(Translate),
    Cancel { req_id: u16, slot: u8 },
    Clear,
    Test { nonce: u16 },
    View(Vec<View>),
    /// The colours of the link spans of the next TRANSLATE with this reqId: (span index from 0,
    /// RGB), strictly increasing. Numbers only, no text.
    Colours { req_id: u16, spans: Vec<(u8, [u8; 3])> },
}

#[derive(Debug)]
pub enum ParseError {
    Empty,
    UnknownType(u8),
    /// Truncated, or bytes left over after the last field.
    Length,
    Utf8,
    Range,
}

const MSG_TRANSLATE: u8 = 0x10;
const MSG_CANCEL: u8 = 0x11;
const MSG_CLEAR: u8 = 0x12;
const MSG_TEST: u8 = 0x13;
const MSG_VIEW: u8 = 0x14;
const MSG_COLOURS: u8 = 0x15;

const MAX_SLOT: u8 = 126;
/// Reserved rows per request (1..6). A row tag carries the row number modulo 4; `cards::build`
/// numbers consecutive rows on from the first visible one.
const MAX_ROWS: u8 = 6;
/// TRANSLATE flag bits.
pub const FLAG_NO_CACHE: u8 = 1;
pub const FLAG_PLATE: u8 = 2;
pub const FLAG_REPLY: u8 = 4;
pub const FLAG_COPY: u8 = 8;
/// Every defined flag bit; any other bit makes the message invalid.
pub const FLAGS_MASK: u8 = FLAG_NO_CACHE | FLAG_PLATE | FLAG_REPLY | FLAG_COPY;
/// A reply has its status row and at least one text row.
pub const REPLY_MIN_ROWS: u8 = 2;
const MAX_TARGET: usize = 8;
const MAX_TEXT: usize = 1024;
const MAX_PAIRS: u8 = 8;
const MAX_TERM: usize = 48;
const MAX_MEANING: usize = 120;
const MAX_PAIR_BYTES: usize = 512;
const MAX_VIEWS: u8 = 4;

struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], ParseError> {
        let end = self.i.checked_add(n).ok_or(ParseError::Length)?;
        let s = self.b.get(self.i..end).ok_or(ParseError::Length)?;
        self.i = end;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8, ParseError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, ParseError> {
        let s = self.take(2)?;
        Ok(u16::from_be_bytes([s[0], s[1]]))
    }

    fn str8(&mut self) -> Result<&'a [u8], ParseError> {
        let n = self.u8()? as usize;
        self.take(n)
    }

    fn str16(&mut self) -> Result<&'a [u8], ParseError> {
        let n = self.u16()? as usize;
        self.take(n)
    }

    fn end(&self) -> Result<(), ParseError> {
        if self.i == self.b.len() {
            Ok(())
        } else {
            Err(ParseError::Length)
        }
    }
}

fn utf8(b: &[u8]) -> Result<String, ParseError> {
    std::str::from_utf8(b).map(str::to_owned).map_err(|_| ParseError::Utf8)
}

/// An explicit language code ("de", "zh-TW"): ASCII letters and '-', starting with a letter. A
/// reply's target must be one; a placeholder such as "?" for an undecided language is not.
fn is_language_code(b: &[u8]) -> bool {
    b.first().is_some_and(u8::is_ascii_alphabetic) && b.iter().all(|c| c.is_ascii_alphabetic() || *c == b'-')
}

fn parse_translate(r: &mut Reader) -> Result<Translate, ParseError> {
    let req_id = r.u16()?;
    let slot = r.u8()?;
    let rows = r.u8()?;
    let flags = r.u8()?;
    if slot > MAX_SLOT || rows == 0 || rows > MAX_ROWS || flags & !FLAGS_MASK != 0 {
        return Err(ParseError::Range);
    }
    let (plate, reply, copy) = (flags & FLAG_PLATE != 0, flags & FLAG_REPLY != 0, flags & FLAG_COPY != 0);
    // Never copy an incoming message; a reply always has its status row on a plate.
    if (copy && !reply) || (reply && (!plate || rows < REPLY_MIN_ROWS)) {
        return Err(ParseError::Range);
    }
    let target = r.str8()?;
    if target.is_empty() || target.len() > MAX_TARGET || !target.iter().all(u8::is_ascii_graphic) {
        return Err(ParseError::Range);
    }
    if reply && !is_language_code(target) {
        return Err(ParseError::Range);
    }
    let text = r.str16()?;
    if text.len() > MAX_TEXT {
        return Err(ParseError::Range);
    }
    let text = utf8(text)?;
    let n = r.u8()?;
    if n > MAX_PAIRS {
        return Err(ParseError::Range);
    }
    let mut pairs = Vec::with_capacity(n as usize);
    let mut used = 0usize;
    for _ in 0..n {
        let term = r.str8()?;
        let meaning = r.str8()?;
        used += 2 + term.len() + meaning.len();
        if term.len() > MAX_TERM || meaning.len() > MAX_MEANING || used > MAX_PAIR_BYTES {
            return Err(ParseError::Range);
        }
        pairs.push((ChatText(utf8(term)?), ChatText(utf8(meaning)?)));
    }
    Ok(Translate {
        req_id,
        slot,
        rows,
        no_cache: flags & FLAG_NO_CACHE != 0,
        plate,
        reply,
        copy,
        // checked above: ASCII only
        target: String::from_utf8_lossy(target).into_owned(),
        text: ChatText(text),
        pairs,
    })
}

/// Parses one complete message (as rebuilt by the reassembler).
pub fn parse_message(b: &[u8]) -> Result<Message, ParseError> {
    let (&ty, body) = b.split_first().ok_or(ParseError::Empty)?;
    let mut r = Reader { b: body, i: 0 };
    let m = match ty {
        MSG_TRANSLATE => Message::Translate(parse_translate(&mut r)?),
        MSG_CANCEL => {
            let req_id = r.u16()?;
            let slot = r.u8()?;
            if slot > crate::tags::TEST_SLOT {
                return Err(ParseError::Range);
            }
            Message::Cancel { req_id, slot }
        }
        MSG_CLEAR => Message::Clear,
        MSG_TEST => Message::Test { nonce: r.u16()? },
        MSG_VIEW => {
            let n = r.u8()?;
            if n > MAX_VIEWS {
                return Err(ParseError::Range);
            }
            let mut views = Vec::with_capacity(n as usize);
            for _ in 0..n {
                views.push(View {
                    id: r.u8()?,
                    x: r.u16()?,
                    y: r.u16()?,
                    w: r.u16()?,
                    h: r.u16()?,
                    font_px: r.u8()?,
                    row_px: r.u8()?,
                });
            }
            Message::View(views)
        }
        MSG_COLOURS => {
            let req_id = r.u16()?;
            let n = r.u8()?;
            if n == 0 {
                return Err(ParseError::Range);
            }
            let mut spans: Vec<(u8, [u8; 3])> = Vec::with_capacity(n as usize);
            for _ in 0..n {
                let s = r.take(4)?;
                if spans.last().is_some_and(|&(prev, _)| s[0] <= prev) {
                    return Err(ParseError::Range);
                }
                spans.push((s[0], [s[1], s[2], s[3]]));
            }
            Message::Colours { req_id, spans }
        }
        other => return Err(ParseError::UnknownType(other)),
    };
    r.end()?;
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn simple_messages() {
        assert!(matches!(parse_message(&[]), Err(ParseError::Empty)));
        assert!(matches!(parse_message(&[0x12]), Ok(Message::Clear)));
        assert!(matches!(parse_message(&[0x12, 0]), Err(ParseError::Length)));
        assert!(matches!(parse_message(&[0x13, 1, 2]), Ok(Message::Test { nonce: 0x0102 })));
        assert!(matches!(parse_message(&[0x13, 1]), Err(ParseError::Length)));
        assert!(matches!(parse_message(&[0x11, 0, 7, 3]), Ok(Message::Cancel { req_id: 7, slot: 3 })));
        assert!(matches!(parse_message(&[0x11, 0, 7, 200]), Err(ParseError::Range)));
        assert!(matches!(parse_message(&[0x16]), Err(ParseError::UnknownType(0x16))));
        assert!(matches!(parse_message(&[0x02]), Err(ParseError::UnknownType(0x02))));
    }

    #[test]
    fn colours() {
        // reqId 300, spans 0 (0070DD) and 2 (71D5FF)
        let b = hex("15012c02000070dd0271d5ff");
        let Ok(Message::Colours { req_id, spans }) = parse_message(&b) else { panic!("colours") };
        assert_eq!((req_id, spans), (300, vec![(0, [0, 0x70, 0xDD]), (2, [0x71, 0xD5, 0xFF])]));
        // n 0, a span index not increasing, truncated, a trailing byte
        assert!(matches!(parse_message(&hex("15012c00")), Err(ParseError::Range)));
        assert!(matches!(parse_message(&hex("15012c02010070dd0171d5ff")), Err(ParseError::Range)));
        assert!(matches!(parse_message(&hex("15012c02020070dd0171d5ff")), Err(ParseError::Range)));
        assert!(matches!(parse_message(&hex("15012c02000070dd0271d5")), Err(ParseError::Length)));
        assert!(matches!(parse_message(&hex("15012c01000070dd00")), Err(ParseError::Length)));
        assert!(matches!(parse_message(&hex("15012c")), Err(ParseError::Length)));
        // span 255 is the last index
        assert!(matches!(parse_message(&hex("15000101ffffd100")), Ok(Message::Colours { spans, .. }) if spans == vec![(255, [255, 0xD1, 0])]));
    }

    #[test]
    fn view() {
        let b = hex("140101000a01f400c800500d10");
        let Ok(Message::View(v)) = parse_message(&b) else { panic!("view") };
        assert_eq!(v, vec![View { id: 1, x: 10, y: 500, w: 200, h: 80, font_px: 13, row_px: 16 }]);
        assert!(matches!(parse_message(&hex("1405")), Err(ParseError::Range)));
        assert!(matches!(parse_message(&hex("1401")), Err(ParseError::Length)));
        assert!(matches!(parse_message(&hex("1400")), Ok(Message::View(_))));
    }

    #[test]
    fn translate() {
        // reqId 5, slot 3, rows 2, flags 3, "fr", "a\x02[b]\x03", pairs ("lf", "cherche")
        let b = hex("10000503020302667200066102");
        let mut b = b;
        b.extend_from_slice(b"[b]");
        b.push(3);
        b.extend_from_slice(&hex("01026c6607"));
        b.extend_from_slice(b"cherche");
        let Ok(Message::Translate(t)) = parse_message(&b) else { panic!("translate") };
        assert_eq!((t.req_id, t.slot, t.rows, t.no_cache, t.plate, t.reply, t.copy), (5, 3, 2, true, true, false, false));
        assert_eq!(t.target, "fr");
        assert_eq!(t.text.as_str(), "a\u{2}[b]\u{3}");
        assert_eq!(t.text.len(), 6);
        assert_eq!(t.pairs.len(), 1);
        assert_eq!((t.pairs[0].0.as_str(), t.pairs[0].1.as_str()), ("lf", "cherche"));
        // trailing byte, truncation, ranges
        let mut long = b.clone();
        long.push(0);
        assert!(matches!(parse_message(&long), Err(ParseError::Length)));
        assert!(matches!(parse_message(&b[..b.len() - 1]), Err(ParseError::Length)));
        let mut bad = b.clone();
        bad[3] = 127; // slot 127 never carries a TRANSLATE
        assert!(matches!(parse_message(&bad), Err(ParseError::Range)));
        let mut bad = b.clone();
        bad[4] = 0; // rows 0
        assert!(matches!(parse_message(&bad), Err(ParseError::Range)));
        let mut bad = b.clone();
        bad[5] = 4; // reply without the plate
        assert!(matches!(parse_message(&bad), Err(ParseError::Range)));
        let mut bad = b.clone();
        bad[5] = 16; // undefined flag bit
        assert!(matches!(parse_message(&bad), Err(ParseError::Range)));
        let mut bad = b.clone();
        bad[11] = 0xFF; // invalid UTF-8 in the text
        assert!(matches!(parse_message(&bad), Err(ParseError::Utf8)));
    }

    #[test]
    fn translate_rows_range_is_1_to_6() {
        // reqId 1, slot 2, rows r, flags 2 (plate), "en", "x", no pairs
        let msg = |rows: u8| {
            let mut b = vec![0x10, 0, 1, 2, rows, 2, 2, b'e', b'n', 0, 1, b'x', 0];
            b.shrink_to_fit();
            b
        };
        for rows in 1..=6u8 {
            match parse_message(&msg(rows)) {
                Ok(Message::Translate(t)) => assert_eq!(t.rows, rows),
                _ => panic!("rows {rows} refused"),
            }
        }
        for rows in [0u8, 7, 8, 255] {
            assert!(matches!(parse_message(&msg(rows)), Err(ParseError::Range)), "rows {rows} accepted");
        }
    }

    /// TRANSLATE{reqId 1, slot, rows, flags, target, "x", no pairs}
    fn tr(slot: u8, rows: u8, flags: u8, target: &[u8]) -> Vec<u8> {
        let mut b = vec![0x10, 0, 1, slot, rows, flags, target.len() as u8];
        b.extend_from_slice(target);
        b.extend_from_slice(&[0, 1, b'x', 0]);
        b
    }

    #[test]
    fn every_flag_byte() {
        // valid: any chat combination of noCache and plate; a reply with the plate, with or
        // without copy and noCache; nothing else
        for flags in 0..=255u8 {
            let ok = matches!(flags, 0..=3 | 6 | 7 | 14 | 15);
            match parse_message(&tr(126, 3, flags, b"de")) {
                Ok(Message::Translate(t)) => {
                    assert!(ok, "flags {flags} accepted");
                    let want = (flags & 1 != 0, flags & 2 != 0, flags & 4 != 0, flags & 8 != 0);
                    assert_eq!((t.no_cache, t.plate, t.reply, t.copy), want, "flags {flags}");
                }
                Err(ParseError::Range) => assert!(!ok, "flags {flags} refused"),
                _ => panic!("flags {flags}: unexpected result"),
            }
        }
        assert_eq!((FLAG_NO_CACHE, FLAG_PLATE, FLAG_REPLY, FLAG_COPY, FLAGS_MASK), (1, 2, 4, 8, 15));
    }

    #[test]
    fn reply_rows_slot_and_target() {
        let reply = FLAG_PLATE | FLAG_REPLY | FLAG_COPY;
        for rows in 2..=6u8 {
            assert!(matches!(parse_message(&tr(126, rows, reply, b"de")), Ok(Message::Translate(t)) if t.rows == rows));
        }
        assert!(matches!(parse_message(&tr(126, 1, reply, b"de")), Err(ParseError::Range)), "reply on 1 row");
        assert!(matches!(parse_message(&tr(126, 7, reply, b"de")), Err(ParseError::Range)), "reply on 7 rows");
        // the addon uses slot 126, but any request slot is valid; 127 never is
        assert!(matches!(parse_message(&tr(0, 2, reply, b"fr")), Ok(Message::Translate(t)) if t.slot == 0 && t.reply));
        assert!(matches!(parse_message(&tr(127, 2, reply, b"fr")), Err(ParseError::Range)));
        for good in [&b"de"[..], b"zh-TW", b"pt"] {
            assert!(parse_message(&tr(126, 2, reply, good)).is_ok(), "{good:?}");
        }
        for bad in [&b"?"[..], b"-de", b"de1", b"d_e", b"*"] {
            assert!(matches!(parse_message(&tr(126, 2, reply, bad)), Err(ParseError::Range)), "{bad:?}");
            // the language rule is only for replies: a chat request keeps the printable-ASCII rule
            assert!(parse_message(&tr(5, 2, FLAG_PLATE, bad)).is_ok(), "chat {bad:?}");
        }
    }
}
