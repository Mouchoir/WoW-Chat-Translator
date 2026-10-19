//! Ground truth: where each reserved row's tag and plate are on screen (client px), from the layout.

use serde_json::{json, Value};

use crate::layout::{FrameLayout, Item};
use crate::raster::IRect;
use crate::tagcode::{self, TagId, PLATE_RGB};

pub const ROW_LINK: &str = "addon:TranslateProbe:x:";
/// The text colour of an inline card's hint lane (the addon's `ChatButton` LANE_COLOR, ffd100,
/// NORMAL_FONT_COLOR's gold; user request U1, 2026-10-19).
pub const LANE_RGB: [u8; 3] = [255, 209, 0];

#[derive(Clone, Debug, PartialEq)]
pub struct Plate {
    pub view: u8,
    pub id: TagId,
    /// Float edges (x0, y0, x1, y1) of the whole tag and plate, before clipping.
    pub tag_f: [f64; 4],
    pub plate_f: Option<[f64; 4]>,
    /// Visible parts (inside the chat clip rectangle), rounded to pixels.
    pub tag: Option<IRect>,
    pub plate: Option<IRect>,
    pub alpha: f64,
}

impl Plate {
    /// Height of the visible part of the tag.
    pub fn visible_h(&self) -> i32 {
        self.tag.map(|r| r.h).unwrap_or(0)
    }
}

/// Every tagged row of a frame layout.
pub fn plates(l: &FrameLayout) -> Vec<Plate> {
    let mut out = Vec::new();
    for row in &l.rows {
        let tex: Vec<(&f64, &f64, &f64, &f64, &[u8; 3])> = row
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Tex { x0, x1, y0, y1, rgb, link: Some(p) } if p.starts_with(ROW_LINK) => Some((x0, x1, y0, y1, rgb)),
                _ => None,
            })
            .collect();
        if tex.len() < 8 {
            continue;
        }
        let colors: Vec<[u8; 3]> = tex[..8].iter().map(|t| *t.4).collect();
        let Some(id) = tagcode::decode(&colors) else { continue };
        let cx = f64::from(l.clip.x);
        let ry = f64::from(row.y);
        let tag_f = [cx + tex[0].0, ry + tex[0].2, cx + tex[7].1, ry + tex[0].3];
        let plate_f = tex.get(8).filter(|t| *t.4 == PLATE_RGB).map(|t| [cx + t.0, ry + t.2, cx + t.1, ry + t.3]);
        let vis = |f: [f64; 4]| IRect::from_edges(f[0], f[1], f[2], f[3]).intersect(&l.clip);
        out.push(Plate { view: l.frame_id, id, tag_f, plate_f, tag: vis(tag_f), plate: plate_f.and_then(vis), alpha: row.alpha });
    }
    out
}

/// An inline card's hint lane: the chat row right after the last tagged row of a message, written
/// in `LANE_RGB` from its indent (a blank row until the addon reveals the hint, 1.5 s after the
/// click). The Helper must paint it over while it paints the card.
#[derive(Clone, Debug, PartialEq)]
pub struct LaneTruth {
    pub view: u8,
    pub slot: u8,
    /// From the tag's x0 to the plate's end, the row's top and height (client px, not clipped).
    pub rect: IRect,
    /// Entirely inside the chat clip rectangle.
    pub inside: bool,
    /// The hint is written (non-blank text): what a player sees without Translate Companion.
    pub revealed: bool,
    pub alpha: f64,
}

/// Every hint lane of a frame layout: a row whose text starts in `LANE_RGB` (no texture) right after
/// a tagged row of the same message. That row may lie above the chat's top (`FrameLayout::above`):
/// the lane is then the top line, cut by the top or whole, with no row of its card shown.
pub fn lanes(l: &FrameLayout) -> Vec<LaneTruth> {
    let mut out = Vec::new();
    for next in &l.rows {
        let lane_row = matches!(next.items.first(), Some(Item::Text { rgb, .. }) if *rgb == LANE_RGB)
            && !next.items.iter().any(|it| matches!(it, Item::Tex { .. }));
        if !lane_row {
            continue;
        }
        let Some(row) = l.rows.iter().chain(&l.above).find(|r| r.entry == next.entry && r.y + r.height == next.y) else { continue };
        let tex: Vec<(f64, f64, [u8; 3])> = row
            .items
            .iter()
            .filter_map(|it| match it {
                Item::Tex { x0, x1, rgb, link: Some(p), .. } if p.starts_with(ROW_LINK) => Some((*x0, *x1, *rgb)),
                _ => None,
            })
            .collect();
        if tex.len() < 9 || tex[8].2 != PLATE_RGB {
            continue;
        }
        let colors: Vec<[u8; 3]> = tex[..8].iter().map(|t| t.2).collect();
        let Some(id) = tagcode::decode(&colors) else { continue };
        let cx = f64::from(l.clip.x);
        let rect = IRect::from_edges(cx + tex[0].0, f64::from(next.y), cx + tex[8].1, f64::from(next.y + next.height));
        let inside = rect.intersect(&l.clip) == Some(rect);
        let revealed = next.items.iter().any(|it| matches!(it, Item::Text { text, .. } if !text.trim().is_empty()));
        out.push(LaneTruth { view: l.frame_id, slot: id.slot, rect, inside, revealed, alpha: next.alpha });
    }
    out
}

/// One `lanes` item of the truth file.
pub fn lane_json(t: &LaneTruth, ui_shown: bool) -> Value {
    json!({
        "view": t.view, "slot": t.slot, "rect": t.rect.to_array(), "inside": t.inside, "revealed": t.revealed,
        "visible": ui_shown && t.alpha > 0.0, "alpha": (t.alpha * 100.0).round() / 100.0,
    })
}

/// Parts of the plate (and tag) hidden by covering rectangles.
pub fn covered(p: &Plate, covers: &[IRect]) -> Vec<IRect> {
    let mut out = Vec::new();
    for c in covers {
        for r in [p.tag, p.plate].into_iter().flatten() {
            if let Some(i) = r.intersect(c) {
                out.push(i);
            }
        }
    }
    out
}

/// One `plates` item of the truth file.
pub fn item_json(p: &Plate, ui_shown: bool, covers: &[IRect]) -> Value {
    let visible = ui_shown && p.alpha > 0.0 && p.tag.is_some();
    json!({
        "view": p.view, "slot": p.id.slot, "row": p.id.row, "sess": p.id.sess_bit,
        "tag": p.tag.map(|r| r.to_array()), "plate": p.plate.map(|r| r.to_array()),
        "visible": visible, "alpha": (p.alpha * 100.0).round() / 100.0,
        "covered": covered(p, covers).iter().map(|r| r.to_array()).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{layout_frame, FixedMeasure};
    use crate::scenario::{Entry, FrameState};

    fn tag_row(id: TagId) -> String {
        let mut s = String::from("|n|cff9fd4ff    |Haddon:TranslateProbe:x:7:101|h");
        for c in tagcode::encode(id) {
            s.push_str(&format!("|TInterface\\Buttons\\WHITE8X8:14:4:0:0:8:8:0:8:0:8:{}:{}:{}|t", c[0], c[1], c[2]));
        }
        s.push_str("|TInterface\\Buttons\\WHITE8X8:14:290:0:0:8:8:0:8:0:8:32:40:64|t|h|r");
        s
    }

    #[test]
    fn plates_found_with_geometry() {
        let f = FrameState { id: 1, name: String::new(), shown: true, clip: [28, 686, 563, 253], font_px: 20, ppu: 1.40625, spacing_px: 0.0, scroll: 0 };
        let a = TagId { slot: 3, row: 0, sess_bit: 1 };
        let b = TagId { slot: 3, row: 1, sess_bit: 1 };
        let entries = vec![
            Entry { m: format!("[Bob]: hi{}{}", tag_row(a), tag_row(b)), rgb: [255, 255, 255], ev: "CHAT_MSG_SAY".into(), ts: 0.0 },
            Entry { m: "[Ann]: newer".into(), rgb: [255, 255, 255], ev: "CHAT_MSG_SAY".into(), ts: 0.0 },
        ];
        let mut m = FixedMeasure { per_char: 10.0 };
        let l = layout_frame(&f, &entries, &|_| 1.0, &mut m);
        let p = plates(&l);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].id, a);
        assert_eq!(p[1].id, b);
        // rows: text, tag a, tag b, then the newer line at the bottom (939 = 686 + 253)
        assert_eq!(p[1].tag.unwrap().y, 939 - 20 - 20);
        assert_eq!(p[0].tag.unwrap().y, 939 - 20 - 40);
        let t = p[0].tag.unwrap();
        assert_eq!(t.x, 68); // 28 + 4 spaces of 10 px
        assert_eq!(t.w, 45); // 8 * 5.625
        let pl = p[0].plate.unwrap();
        assert_eq!(pl.x, 113);
        assert_eq!(pl.w, 408); // 290 * 1.40625 = 407.8
        let cov = covered(&p[0], &[IRect::new(300, 800, 400, 200)]);
        assert_eq!(cov.len(), 1);
        assert_eq!(cov[0].x, 300);
    }

    #[test]
    fn hint_lanes_under_the_last_tagged_row() {
        let f = FrameState { id: 1, name: String::new(), shown: true, clip: [28, 686, 563, 253], font_px: 20, ppu: 1.40625, spacing_px: 0.0, scroll: 0 };
        let a = TagId { slot: 3, row: 0, sess_bit: 1 };
        let b = TagId { slot: 3, row: 1, sess_bit: 1 };
        let lane = |hint: &str| format!("|n|cffffd100    |Haddon:TranslateProbe:x:7:101|h{hint}|h|r|cff66bbff|Haddon:TranslateProbe:d:7:101|h[Download]|h|r");
        let gloss = "|n|cffe6cc80    |Haddon:TranslateProbe:x:7:101|hGloss: lol = laughing|h|r";
        let entries = vec![
            Entry { m: format!("[Bob]: hi{}{}{}{}", tag_row(a), tag_row(b), lane("No translation above? "), gloss), rgb: [255, 255, 255], ev: "CHAT_MSG_SAY".into(), ts: 0.0 },
            Entry { m: format!("[Ann]: newer{}", tag_row(TagId { slot: 4, row: 0, sess_bit: 1 })), rgb: [255, 255, 255], ev: "CHAT_MSG_SAY".into(), ts: 0.0 },
        ];
        let mut m = FixedMeasure { per_char: 10.0 };
        let l = layout_frame(&f, &entries, &|_| 1.0, &mut m);
        let ls = lanes(&l);
        assert_eq!(ls.len(), 1, "a lane under slot 3 only (slot 4 has none: the next row is not a lane)");
        let t = &ls[0];
        assert_eq!((t.view, t.slot), (1, 3));
        // rows: text, tag a, tag b, lane, glossary, then the newer line and its row at the bottom
        assert_eq!(t.rect.y, 939 - 40 - 40);
        assert_eq!((t.rect.x, t.rect.h), (68, 20));
        assert_eq!(t.rect.right(), 68 + 45 + 408);
        assert!(t.inside && t.revealed);
        // before the reveal the lane is a blank row
        let entries = vec![Entry { m: format!("[Bob]: hi{}{}", tag_row(a), lane(" ").replace("[Download]", "")), rgb: [255, 255, 255], ev: "CHAT_MSG_SAY".into(), ts: 0.0 }];
        let l = layout_frame(&f, &entries, &|_| 1.0, &mut m);
        let ls = lanes(&l);
        assert_eq!(ls.len(), 1);
        assert!(!ls[0].revealed);
    }

    #[test]
    fn a_lane_whose_rows_left_the_top_is_still_a_lane() {
        // the card's rows above the chat's top, its lane the top line (whole, then cut by the top)
        let a = TagId { slot: 3, row: 0, sess_bit: 1 };
        let lane = "|n|cffffd100    |Haddon:TranslateProbe:x:7:101|hNo translation above? |h|r|cff66bbff|Haddon:TranslateProbe:d:7:101|h[Download]|h|r";
        let mut entries = vec![Entry { m: format!("[Bob]: hi{}{}", tag_row(a), lane), rgb: [255, 255, 255], ev: "CHAT_MSG_SAY".into(), ts: 0.0 }];
        for i in 0..3 {
            entries.push(Entry { m: format!("[Ann]: line {i}"), rgb: [255, 255, 255], ev: "CHAT_MSG_SAY".into(), ts: 0.0 });
        }
        let mut m = FixedMeasure { per_char: 10.0 };
        for (h, inside) in [(80, true), (70, false)] {
            let f = FrameState { id: 1, name: String::new(), shown: true, clip: [28, 686, 563, h], font_px: 20, ppu: 1.40625, spacing_px: 0.0, scroll: 0 };
            let l = layout_frame(&f, &entries, &|_| 1.0, &mut m);
            assert!(!l.rows.iter().any(|r| r.items.iter().any(|it| matches!(it, Item::Tex { .. }))), "no tag row shows");
            let ls = lanes(&l);
            assert_eq!(ls.len(), 1, "clip {h}");
            assert_eq!((ls[0].slot, ls[0].inside), (3, inside), "clip {h}");
            assert_eq!(ls[0].rect.y, 686 + h - 80);
        }
    }
}
