//! WoW chat markup: the escape sequences a chat history entry can contain.
//!
//! `|cAARRGGBB` colour, `|r` back to the line colour, `|H<payload>|h<text>|h` hyperlink,
//! `|n` (or a raw newline) line break, `|T<file>:<h>:<w>:...:<R>:<G>:<B>|t` inline texture,
//! `|A..|a` atlas and `|K..|k` protected names (drawn as nothing here), `||` a literal bar.

#[derive(Clone, Debug, PartialEq)]
pub struct Tex {
    /// Height and width in chat-frame units (0 = the font height).
    pub h: f64,
    pub w: f64,
    /// Vertex colour; None = untinted (drawn white).
    pub rgb: Option<[u8; 3]>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Text(String),
    Color([u8; 3]),
    Reset,
    LinkStart(String),
    LinkEnd,
    Newline,
    Texture(Tex),
}

fn hex2(s: &[u8]) -> Option<u8> {
    let d = |c: u8| (c as char).to_digit(16);
    Some((d(s[0])? * 16 + d(s[1])?) as u8)
}

/// Parses the fields after the file name of a `|T` sequence.
fn parse_tex(body: &str) -> Tex {
    let f: Vec<&str> = body.split(':').collect();
    let num = |i: usize| f.get(i).and_then(|v| v.trim().parse::<f64>().ok());
    let h = num(1).unwrap_or(0.0);
    let w = num(2).unwrap_or(0.0);
    let rgb = match (num(11), num(12), num(13)) {
        (Some(r), Some(g), Some(b)) => Some([r.clamp(0.0, 255.0) as u8, g.clamp(0.0, 255.0) as u8, b.clamp(0.0, 255.0) as u8]),
        _ => None,
    };
    Tex { h, w, rgb }
}

/// Splits a message into tokens. Text tokens never contain an escape sequence.
pub fn tokenize(s: &str) -> Vec<Tok> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut text = String::new();
    let mut in_link = false;
    let mut i = 0;
    let flush = |text: &mut String, out: &mut Vec<Tok>| {
        if !text.is_empty() {
            out.push(Tok::Text(std::mem::take(text)));
        }
    };
    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            flush(&mut text, &mut out);
            out.push(Tok::Newline);
            i += 1;
            continue;
        }
        if c != b'|' || i + 1 >= b.len() {
            // copy one UTF-8 character
            let ch = s[i..].chars().next().unwrap_or('?');
            text.push(ch);
            i += ch.len_utf8().max(1);
            continue;
        }
        let k = b[i + 1];
        match k {
            b'|' => {
                text.push('|');
                i += 2;
            }
            b'c' if i + 10 <= b.len() && b[i + 2..i + 10].iter().all(|x| x.is_ascii_hexdigit()) => {
                flush(&mut text, &mut out);
                let rgb = [hex2(&b[i + 4..i + 6]).unwrap_or(255), hex2(&b[i + 6..i + 8]).unwrap_or(255), hex2(&b[i + 8..i + 10]).unwrap_or(255)];
                out.push(Tok::Color(rgb));
                i += 10;
            }
            b'r' => {
                flush(&mut text, &mut out);
                out.push(Tok::Reset);
                i += 2;
            }
            b'n' => {
                flush(&mut text, &mut out);
                out.push(Tok::Newline);
                i += 2;
            }
            b'h' => {
                flush(&mut text, &mut out);
                if in_link {
                    out.push(Tok::LinkEnd);
                    in_link = false;
                }
                i += 2;
            }
            b'H' => {
                flush(&mut text, &mut out);
                let rest = &s[i + 2..];
                match rest.find("|h") {
                    Some(end) => {
                        out.push(Tok::LinkStart(rest[..end].to_string()));
                        in_link = true;
                        i += 2 + end + 2;
                    }
                    None => i = b.len(),
                }
            }
            b'T' => {
                flush(&mut text, &mut out);
                let rest = &s[i + 2..];
                match rest.find("|t") {
                    Some(end) => {
                        out.push(Tok::Texture(parse_tex(&rest[..end])));
                        i += 2 + end + 2;
                    }
                    None => i = b.len(),
                }
            }
            b'A' | b'K' => {
                flush(&mut text, &mut out);
                let close = if k == b'A' { "|a" } else { "|k" };
                let rest = &s[i + 2..];
                i = match rest.find(close) {
                    Some(end) => i + 2 + end + 2,
                    None => b.len(),
                };
            }
            _ => {
                text.push('|');
                i += 1;
            }
        }
    }
    flush(&mut text, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_links_textures_and_newlines() {
        let s = "[Bob]: hi |cffffffff|Hitem:2996::|h[Linen Cloth]|h|r ok |cff66bbff|Haddon:TranslateProbe:t:7:101|h[T]|h|r\
                 |n|cff9fd4ff    |Haddon:TranslateProbe:x:7:101|h|TInterface\\Buttons\\WHITE8X8:14:4:0:0:8:8:0:8:0:8:255:0:255|t|h|r";
        let t = tokenize(s);
        assert_eq!(t[0], Tok::Text("[Bob]: hi ".into()));
        assert_eq!(t[1], Tok::Color([255, 255, 255]));
        assert_eq!(t[2], Tok::LinkStart("item:2996::".into()));
        assert_eq!(t[3], Tok::Text("[Linen Cloth]".into()));
        assert_eq!(t[4], Tok::LinkEnd);
        assert_eq!(t[5], Tok::Reset);
        assert!(t.contains(&Tok::Newline));
        assert!(t.contains(&Tok::LinkStart("addon:TranslateProbe:x:7:101".into())));
        assert!(t.contains(&Tok::Texture(Tex { h: 14.0, w: 4.0, rgb: Some([255, 0, 255]) })));
        assert_eq!(*t.last().unwrap(), Tok::Reset);
    }

    #[test]
    fn literal_bar_utf8_and_ignored_sequences() {
        let t = tokenize("a||b é |Afoo|a c |Kq1|k d");
        assert_eq!(t, vec![Tok::Text("a|b é ".into()), Tok::Text(" c ".into()), Tok::Text(" d".into())]);
    }
}
