//! Chat layout the way a WoW ScrollingMessageFrame does it (Forever 1.60.1 UI source, report 1):
//! the newest message sits at the bottom of the clip rectangle, older ones stack upward, each
//! message's height follows its wrapped rows, and the container clips what leaves the top.
//! Long lines wrap at word boundaries with an indent (IndentedWordWrap); `|n` starts a new row at
//! the left edge. Inline textures are `units * ppu` px and may start at a fractional x.

use crate::markup::{tokenize, Tok};
use crate::raster::IRect;
use crate::scenario::{Entry, FrameState};

/// Text measurement in px for the frame's font (GDI in fakewow, a fixed width in tests).
pub trait Measure {
    fn width(&mut self, text: &str) -> f64;
}

/// Every character is `per_char` px wide.
pub struct FixedMeasure {
    pub per_char: f64,
}

impl Measure for FixedMeasure {
    fn width(&mut self, text: &str) -> f64 {
        text.chars().count() as f64 * self.per_char
    }
}

/// Indent of wrapped continuation rows, in font heights.
pub const WRAP_INDENT_EM: f64 = 0.6;

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    /// Text drawn with its left edge at `x` (px from the clip's left edge).
    Text { x: f64, w: f64, text: String, rgb: [u8; 3] },
    /// Inline texture: x0..x1 from the clip's left edge, y0..y1 from the row's top.
    Tex { x0: f64, x1: f64, y0: f64, y1: f64, rgb: [u8; 3], link: Option<String> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub items: Vec<Item>,
    pub height: i32,
}

/// Default WoW chat colours of the player chat events (the mock adds every line in white).
pub fn event_rgb(ev: &str, fallback: [u8; 3]) -> [u8; 3] {
    match ev {
        "CHAT_MSG_SAY" => [255, 255, 255],
        "CHAT_MSG_YELL" => [255, 64, 64],
        "CHAT_MSG_EMOTE" => [255, 128, 64],
        "CHAT_MSG_WHISPER" | "CHAT_MSG_BN_WHISPER" => [255, 128, 255],
        "CHAT_MSG_PARTY" | "CHAT_MSG_PARTY_LEADER" => [170, 170, 255],
        "CHAT_MSG_RAID" | "CHAT_MSG_RAID_LEADER" | "CHAT_MSG_INSTANCE_CHAT" | "CHAT_MSG_INSTANCE_CHAT_LEADER" => [255, 127, 0],
        "CHAT_MSG_RAID_WARNING" => [255, 72, 0],
        "CHAT_MSG_GUILD" => [64, 255, 64],
        "CHAT_MSG_OFFICER" => [64, 192, 64],
        "CHAT_MSG_CHANNEL" | "CHAT_MSG_COMMUNITIES_CHANNEL" => [255, 192, 192],
        _ => fallback,
    }
}

enum Atom {
    Word(String, [u8; 3]),
    Space(String, [u8; 3]),
    Tex { w: f64, h: f64, rgb: [u8; 3], link: Option<String> },
    Newline,
}

fn atoms(msg: &str, base: [u8; 3], font_px: f64, ppu: f64) -> Vec<Atom> {
    let mut out = Vec::new();
    let mut color = base;
    let mut link: Option<String> = None;
    for t in tokenize(msg) {
        match t {
            Tok::Text(s) => {
                let mut word = String::new();
                let mut space = String::new();
                for ch in s.chars() {
                    if ch == ' ' {
                        if !word.is_empty() {
                            out.push(Atom::Word(std::mem::take(&mut word), color));
                        }
                        space.push(ch);
                    } else {
                        if !space.is_empty() {
                            out.push(Atom::Space(std::mem::take(&mut space), color));
                        }
                        word.push(ch);
                    }
                }
                if !word.is_empty() {
                    out.push(Atom::Word(word, color));
                }
                if !space.is_empty() {
                    out.push(Atom::Space(space, color));
                }
            }
            Tok::Color(c) => color = c,
            Tok::Reset => color = base,
            Tok::LinkStart(p) => link = Some(p),
            Tok::LinkEnd => link = None,
            Tok::Newline => out.push(Atom::Newline),
            Tok::Texture(x) => {
                let h = if x.h > 0.0 { x.h * ppu } else { font_px };
                let w = if x.w > 0.0 { x.w * ppu } else { h };
                out.push(Atom::Tex { w, h, rgb: x.rgb.unwrap_or([255, 255, 255]), link: link.clone() });
            }
        }
    }
    out
}

/// Lays one message out in rows (x from the clip's left edge).
pub fn layout_message(entry: &Entry, frame: &FrameState, measure: &mut dyn Measure) -> Vec<Row> {
    let font_px = f64::from(frame.font_px.max(1));
    let width = f64::from(frame.clip[2].max(1));
    let indent = (WRAP_INDENT_EM * font_px).round();
    let base = event_rgb(&entry.ev, entry.rgb);
    let mut rows: Vec<(Vec<Item>, f64)> = vec![(Vec::new(), font_px)];
    let mut x = 0.0f64;
    for a in atoms(&entry.m, base, font_px, frame.ppu) {
        match a {
            Atom::Newline => {
                rows.push((Vec::new(), font_px));
                x = 0.0;
            }
            Atom::Space(s, rgb) => {
                let w = measure.width(&s);
                rows.last_mut().unwrap().0.push(Item::Text { x, w, text: s, rgb });
                x += w;
            }
            Atom::Word(s, rgb) => {
                let w = measure.width(&s);
                if x + w > width && x > indent + 0.5 {
                    rows.push((Vec::new(), font_px));
                    x = indent;
                }
                rows.last_mut().unwrap().0.push(Item::Text { x, w, text: s, rgb });
                x += w;
            }
            Atom::Tex { w, h, rgb, link } => {
                if x + w > width + 0.01 && x > indent + 0.5 {
                    rows.push((Vec::new(), font_px));
                    x = indent;
                }
                let r = rows.last_mut().unwrap();
                r.1 = r.1.max(h);
                r.0.push(Item::Tex { x0: x, x1: x + w, y0: 0.0, y1: h, rgb, link });
                x += w;
            }
        }
    }
    let spacing = frame.spacing_px;
    rows.into_iter()
        .map(|(mut items, h)| {
            let height = (h.ceil() + spacing).round() as i32;
            // textures sit on whole pixels vertically, centred in the row
            for it in items.iter_mut() {
                if let Item::Tex { y0, y1, .. } = it {
                    let th = *y1 - *y0;
                    let top = ((h.ceil() - th) / 2.0).round().max(0.0);
                    *y0 = top;
                    *y1 = top + th;
                }
            }
            Row { items, height }
        })
        .collect()
}

#[derive(Clone, Debug)]
pub struct PlacedRow {
    /// Row top (client px).
    pub y: i32,
    pub height: i32,
    pub items: Vec<Item>,
    pub alpha: f64,
    /// Index of the entry in the history slice.
    pub entry: usize,
}

#[derive(Clone, Debug)]
pub struct FrameLayout {
    pub frame_id: u8,
    pub clip: IRect,
    pub rows: Vec<PlacedRow>,
}

/// Bottom-up layout of a frame's history (oldest first in `entries`), honouring the scroll offset.
/// `alpha(i)` is the fade of entry i. Only rows that touch the clip rectangle are returned.
pub fn layout_frame(frame: &FrameState, entries: &[Entry], alpha: &dyn Fn(usize) -> f64, measure: &mut dyn Measure) -> FrameLayout {
    let clip = IRect::new(frame.clip[0], frame.clip[1], frame.clip[2], frame.clip[3]);
    let mut out = FrameLayout { frame_id: frame.id, clip, rows: Vec::new() };
    let scroll = frame.scroll.max(0) as usize;
    if entries.len() <= scroll {
        return out;
    }
    let mut bottom = clip.bottom();
    for i in (0..entries.len() - scroll).rev() {
        if bottom <= clip.y {
            break;
        }
        let a = alpha(i);
        let rows = layout_message(&entries[i], frame, measure);
        let total: i32 = rows.iter().map(|r| r.height).sum();
        let mut y = bottom - total;
        bottom = y;
        if a <= 0.0 {
            continue; // a faded-out line keeps its place but draws nothing
        }
        for r in rows {
            if y + r.height > clip.y && y < clip.bottom() {
                out.rows.push(PlacedRow { y, height: r.height, items: r.items, alpha: a, entry: i });
            }
            y += r.height;
        }
    }
    out
}

/// SMF fade: fully visible for `visible` s after `since_ms`, then linear to 0 over `duration` s.
pub fn fade_alpha(now_ms: f64, since_ms: f64, visible_s: f64, duration_s: f64) -> f64 {
    let age = (now_ms - since_ms) / 1000.0;
    if age <= visible_s {
        1.0
    } else if duration_s <= 0.0 {
        0.0
    } else {
        (1.0 - (age - visible_s) / duration_s).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> FrameState {
        FrameState { id: 1, name: String::new(), shown: true, clip: [28, 686, 563, 253], font_px: 20, ppu: 1.40625, spacing_px: 0.0, scroll: 0 }
    }

    fn entry(m: &str) -> Entry {
        Entry { m: m.into(), rgb: [255, 255, 255], ev: "CHAT_MSG_SAY".into(), ts: 0.0 }
    }

    fn tag_row(n_tex: usize) -> String {
        let mut s = String::from("|n|cff9fd4ff    |Haddon:TranslateProbe:x:7:101|h");
        for _ in 0..n_tex {
            s.push_str("|TInterface\\Buttons\\WHITE8X8:14:4:0:0:8:8:0:8:0:8:255:0:255|t");
        }
        s.push_str("|TInterface\\Buttons\\WHITE8X8:14:290:0:0:8:8:0:8:0:8:32:40:64|t|h|r");
        s
    }

    #[test]
    fn reserved_rows_start_on_new_rows_after_the_indent() {
        let mut m = FixedMeasure { per_char: 10.0 };
        let e = entry(&format!("[Bob]: hello there{}{}", tag_row(8), tag_row(8)));
        let rows = layout_message(&e, &frame(), &mut m);
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|r| r.height == 20));
        let texes: Vec<(f64, f64)> = rows[1]
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Tex { x0, x1, .. } => Some((*x0, *x1)),
                _ => None,
            })
            .collect();
        assert_eq!(texes.len(), 9);
        assert!((texes[0].0 - 40.0).abs() < 1e-9, "after 4 spaces");
        assert!((texes[1].0 - (40.0 + 5.625)).abs() < 1e-9);
        assert!((texes[8].1 - texes[8].0 - 290.0 * 1.40625).abs() < 1e-9);
    }

    #[test]
    fn long_text_wraps_with_indent() {
        let mut m = FixedMeasure { per_char: 10.0 };
        let words = vec!["word"; 30].join(" ");
        let rows = layout_message(&entry(&words), &frame(), &mut m);
        assert!(rows.len() >= 3);
        let first_x = |r: &Row| match &r.items[0] {
            Item::Text { x, .. } => *x,
            _ => -1.0,
        };
        assert_eq!(first_x(&rows[0]), 0.0);
        assert_eq!(first_x(&rows[1]), 12.0);
        for r in &rows {
            let right = r.items.iter().map(|i| if let Item::Text { x, w, .. } = i { x + w } else { 0.0 }).fold(0.0, f64::max);
            assert!(right <= 563.0 + 1e-9);
        }
    }

    #[test]
    fn bottom_up_with_scroll_and_clip() {
        let mut m = FixedMeasure { per_char: 10.0 };
        let entries: Vec<Entry> = (0..30).map(|i| entry(&format!("line {i}"))).collect();
        let f = frame();
        let l = layout_frame(&f, &entries, &|_| 1.0, &mut m);
        let last = l.rows.iter().max_by_key(|r| r.y).unwrap();
        assert_eq!(last.entry, 29);
        assert_eq!(last.y + last.height, 686 + 253);
        // 253 px of clip, 20 px rows: 12 full rows and one clipped at the top
        assert_eq!(l.rows.len(), 13);
        let mut f2 = f.clone();
        f2.scroll = 3;
        let l2 = layout_frame(&f2, &entries, &|_| 1.0, &mut m);
        assert_eq!(l2.rows.iter().max_by_key(|r| r.y).unwrap().entry, 26);
    }

    #[test]
    fn fade_curve() {
        assert_eq!(fade_alpha(1000.0, 0.0, 5.0, 2.0), 1.0);
        assert!((fade_alpha(6000.0, 0.0, 5.0, 2.0) - 0.5).abs() < 1e-9);
        assert_eq!(fade_alpha(9000.0, 0.0, 5.0, 2.0), 0.0);
    }
}
