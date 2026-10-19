//! The reply card's hint lane (design A.6.4, `docs\BRIDGE.md` section 8c).
//!
//! Under the reply card's last row the addon draws one more row of bubble fill, the lane, with
//! "No status above? Start or update TranslateProbe Helper." in it (text clear of the lane's top
//! and bottom 2 px). The Helper covers the lane whenever it paints the card, so the hint shows
//! exactly when nothing paints it (no timer in the addon).
//!
//! [`find_lane`] looks for it where the card geometry puts it: `row0.yTop + K * rowPx`, within
//! +-2 px. The fill colour is sampled in the lane under the tag column (the hint starts at the
//! plate's x, so that part is plain fill). A column passes when its top 2 and bottom 2 rows are
//! all that colour; a tooltip or a menu over the lane breaks those edges, so it is never painted
//! over. With fewer than 60% of the columns passing the lane is left alone (the hint stays, which
//! is harmless). The overlay paints the passing columns flat with the sampled colour.

use tp_bridge::cards::Card;
use tp_bridge::image::{Bgra, Rect};

/// Rows at the lane's top and at its bottom that must be plain fill.
pub const EDGE: i32 = 2;
/// How far the lane may sit from where the geometry puts it.
pub const SLACK: i32 = 2;
/// Largest per-channel distance of an edge pixel from the fill colour.
pub const TOL: i32 = 8;
/// The tag-column sample must be this uniform (its pixels within this of their median, a quarter
/// of them excepted): else something covers it.
pub const SPREAD_MAX: i32 = 6;
/// Smallest share of passing columns for the lane to be painted.
pub const MIN_SHARE: f32 = 0.6;

/// A lane found in a capture: client rectangle, its fill colour and, per column from `rect.x`,
/// whether that column may be painted.
#[derive(Clone, Debug)]
pub struct Lane {
    pub rect: Rect,
    pub rgb: [u8; 3],
    pub cols: Vec<bool>,
}

fn rect4(r: &Rect) -> [i32; 4] {
    [r.x, r.y, r.w, r.h]
}

impl PartialEq for Lane {
    fn eq(&self, o: &Lane) -> bool {
        rect4(&self.rect) == rect4(&o.rect) && self.rgb == o.rgb && self.cols == o.cols
    }
}

impl Eq for Lane {}

impl Lane {
    /// Columns that may be painted.
    pub fn passing(&self) -> usize {
        self.cols.iter().filter(|c| **c).count()
    }

    /// For the scene fingerprint.
    pub fn hash_words(&self) -> [i64; 6] {
        let mut runs = 0i64;
        let mut prev = false;
        for (i, &c) in self.cols.iter().enumerate() {
            if c != prev {
                runs = runs.wrapping_mul(31).wrapping_add(i as i64 + 1);
                prev = c;
            }
        }
        [
            i64::from(self.rect.x),
            i64::from(self.rect.y),
            i64::from(self.rect.w),
            i64::from(self.rect.h),
            i64::from(self.rgb[0]) << 16 | i64::from(self.rgb[1]) << 8 | i64::from(self.rgb[2]),
            runs,
        ]
    }
}

fn dist(a: [u8; 3], b: [u8; 3]) -> i32 {
    (0..3).map(|c| (i32::from(a[c]) - i32::from(b[c])).abs()).max().unwrap_or(0)
}

/// Per-channel median of a rectangle and the distance of its 75th-percentile pixel from it.
fn median_spread(img: &Bgra, x0: i32, x1: i32, y0: i32, y1: i32) -> Option<([u8; 3], i32)> {
    let mut ch: [Vec<u8>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut px: Vec<[u8; 3]> = Vec::new();
    for y in y0..y1 {
        for x in x0..x1 {
            let p = img.rgb(x, y)?;
            for c in 0..3 {
                ch[c].push(p[c]);
            }
            px.push(p);
        }
    }
    if px.is_empty() {
        return None;
    }
    let mut m = [0u8; 3];
    for c in 0..3 {
        let k = ch[c].len() / 2;
        m[c] = *ch[c].select_nth_unstable(k).1;
    }
    let mut d: Vec<i32> = px.iter().map(|p| dist(*p, m)).collect();
    d.sort_unstable();
    Some((m, d[(d.len() * 3 / 4).min(d.len() - 1)]))
}

/// The lane under a reply card of `k` rows (TRANSLATE rows) whose rows are `row_px` apart, in the
/// capture `img`; None when it is not there (hidden, covered, scrolled away) or when fewer than
/// 60% of its columns pass.
pub fn find_lane(img: &Bgra, card: &Card, k: u8, row_px: f32) -> Option<Lane> {
    let rp = row_px.round() as i32;
    if rp < 2 * EDGE + 1 {
        return None;
    }
    // the last visible row tells where row K would start
    let last = card.bands.iter().max_by_key(|b| b.row)?;
    if k == 0 || last.row >= k {
        return None;
    }
    let y_exp = last.text_rect.y + i32::from(k - last.row) * rp;
    // the tag's own columns (tag_rect also holds the blended column before x0) and the plate end
    let tx0 = last.tag_rect.x + 1;
    let tx1 = (last.tag_rect.x + last.tag_rect.w - 1).max(tx0 + 1);
    let end = card.bands.iter().map(|b| b.text_rect.x + b.text_rect.w).max()?;
    if end <= tx1 {
        return None;
    }
    let (img_y0, img_y1) = (img.origin_y, img.origin_y + img.height as i32);
    let (img_x0, img_x1) = (img.origin_x, img.origin_x + img.width as i32);
    if tx0 < img_x0 || end > img_x1 {
        return None;
    }
    let mut best: Option<(usize, i32, Lane)> = None;
    for dy in -SLACK..=SLACK {
        let y0 = y_exp + dy;
        if y0 < img_y0 || y0 + rp > img_y1 {
            continue;
        }
        let Some((fill, spread)) = median_spread(img, tx0, tx1, y0, y0 + rp) else { continue };
        if spread > SPREAD_MAX {
            continue;
        }
        let edge_rows: Vec<i32> = (y0..y0 + EDGE).chain(y0 + rp - EDGE..y0 + rp).collect();
        let cols: Vec<bool> = (tx0..end).map(|x| edge_rows.iter().all(|&y| img.rgb(x, y).is_some_and(|p| dist(p, fill) <= TOL))).collect();
        let n = cols.iter().filter(|c| **c).count();
        let better = match &best {
            None => true,
            Some((bn, bdy, _)) => n > *bn || (n == *bn && dy.abs() < bdy.abs()),
        };
        if better {
            best = Some((n, dy, Lane { rect: Rect { x: tx0, y: y0, w: end - tx0, h: rp }, rgb: fill, cols }));
        }
    }
    let (n, _, lane) = best?;
    (n as f32 >= MIN_SHARE * lane.cols.len() as f32).then_some(lane)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use tp_bridge::cards::{Mask, RowBand};

    pub const BUBBLE: [u8; 3] = [36, 30, 24];
    const PLATE: [u8; 3] = [32, 40, 64];

    /// A BGRA picture (client origin (0, 0)) of `w` x `h` filled with `ground`.
    pub struct Pic {
        pub w: i32,
        pub h: i32,
        pub px: Vec<u8>,
    }

    impl Pic {
        pub fn new(w: i32, h: i32, ground: [u8; 3]) -> Pic {
            let mut p = Pic { w, h, px: vec![0; (w * h * 4) as usize] };
            p.fill(Rect { x: 0, y: 0, w, h }, ground);
            p
        }

        pub fn fill(&mut self, r: Rect, rgb: [u8; 3]) {
            for y in r.y.max(0)..(r.y + r.h).min(self.h) {
                for x in r.x.max(0)..(r.x + r.w).min(self.w) {
                    let i = ((y * self.w + x) * 4) as usize;
                    self.px[i..i + 4].copy_from_slice(&[rgb[2], rgb[1], rgb[0], 255]);
                }
            }
        }

        pub fn img(&self) -> Bgra<'_> {
            Bgra { px: &self.px, width: self.w as u32, height: self.h as u32, stride: (self.w * 4) as u32, origin_x: 0, origin_y: 0 }
        }
    }

    /// A reply card of `k` rows at (40, 50), rows `rp` px apart, its tag 48 px wide, plates to
    /// x 340, inside a bubble of fill; the lane holds bright hint "text" clear of its edges.
    pub fn reply_pic(k: u8, rp: i32) -> (Pic, Card) {
        let mut pic = Pic::new(400, 300, [90, 120, 60]);
        pic.fill(Rect { x: 30, y: 40, w: 330, h: i32::from(k + 1) * rp + 20 }, BUBBLE);
        let mut bands = Vec::new();
        for r in 0..k {
            let y = 50 + i32::from(r) * rp;
            pic.fill(Rect { x: 40, y, w: 48, h: rp }, [255, 0, 255]);
            pic.fill(Rect { x: 88, y, w: 252, h: rp }, PLATE);
            bands.push(RowBand {
                row: r,
                text_rect: Rect { x: 91, y, w: 249, h: rp },
                mask: Some(Mask { rect: Rect { x: 88, y, w: 252, h: rp }, bits: vec![255; (252 * rp) as usize] }),
                tag_rect: Rect { x: 39, y, w: 50, h: rp },
            });
        }
        let lane_y = 50 + i32::from(k) * rp;
        // hint glyphs from the plate's x, clear of the lane's top and bottom 2 px
        for i in 0..20 {
            pic.fill(Rect { x: 92 + i * 11, y: lane_y + EDGE, w: 6, h: rp - 2 * EDGE }, [200, 200, 200]);
        }
        let card = Card { slot: 126, view_id: 200, bands, gaps: Vec::new(), alpha: 1.0, plate: true, plate_rgb: PLATE, text_width_px: 249 };
        (pic, card)
    }

    #[test]
    fn a_clean_lane_is_found_where_the_geometry_puts_it() {
        let (pic, card) = reply_pic(3, 16);
        let lane = find_lane(&pic.img(), &card, 3, 16.0).expect("lane");
        assert_eq!(rect4(&lane.rect), [40, 50 + 3 * 16, 300, 16]);
        assert_eq!(lane.rgb, BUBBLE);
        assert_eq!(lane.passing(), lane.cols.len(), "every column passes, the hint glyphs included");
        // the lane is one row below the last visible row even when the first rows scrolled away
        let mut tail = card.clone();
        tail.bands.remove(0);
        assert_eq!(find_lane(&pic.img(), &tail, 3, 16.0).map(|l| l.rect.y), Some(98));
        // off by one pixel: still found (rounding), and the band covers the glyphs
        let (pic1, mut card1) = reply_pic(2, 15);
        for b in card1.bands.iter_mut() {
            b.text_rect.y -= 1;
        }
        let l1 = find_lane(&pic1.img(), &card1, 2, 15.0).expect("within 2 px");
        assert!((l1.rect.y - 80).abs() <= 1, "{l1:?}");
    }

    #[test]
    fn a_tooltip_over_part_of_the_lane_is_never_painted() {
        let (mut pic, card) = reply_pic(2, 16);
        // a tooltip (dark blue with a grey border) over the right quarter of the lane and below
        let tip = Rect { x: 270, y: 70, w: 120, h: 80 };
        pic.fill(tip, [160, 160, 160]);
        pic.fill(Rect { x: tip.x + 1, y: tip.y + 1, w: tip.w - 2, h: tip.h - 2 }, [23, 23, 48]);
        let lane = find_lane(&pic.img(), &card, 2, 16.0).expect("most of the lane is clean");
        for (i, ok) in lane.cols.iter().enumerate() {
            let x = lane.rect.x + i as i32;
            if x >= tip.x {
                assert!(!ok, "column {x} under the tooltip passes");
            } else {
                assert!(ok, "clean column {x} fails");
            }
        }
        // a tooltip over most of it: left alone (the hint stays)
        let big = Rect { x: 120, y: 60, w: 260, h: 90 };
        pic.fill(big, [23, 23, 48]);
        assert!(find_lane(&pic.img(), &card, 2, 16.0).is_none());
    }

    #[test]
    fn no_lane_no_paint() {
        // the card's rows end at the bubble's bottom: below them is the game world
        let (mut pic, card) = reply_pic(2, 16);
        pic.fill(Rect { x: 0, y: 82, w: 400, h: 218 }, [90, 120, 60]);
        for x in (0..400).step_by(3) {
            pic.fill(Rect { x, y: 82, w: 1, h: 218 }, [140, 90, 40]);
        }
        assert!(find_lane(&pic.img(), &card, 2, 16.0).is_none());
        // the lane outside the capture, or a row count that does not leave room for it
        let (pic, card) = reply_pic(2, 16);
        let short = Bgra { px: &pic.px[..(400 * 85 * 4) as usize], width: 400, height: 85, stride: 1600, origin_x: 0, origin_y: 0 };
        assert!(find_lane(&short, &card, 2, 16.0).is_none());
        assert!(find_lane(&pic.img(), &card, 1, 16.0).is_none(), "row 1 is visible: K cannot be 1");
        assert!(find_lane(&pic.img(), &card, 2, 4.0).is_none(), "rows too small for 2 + 2 edge rows");
    }
}
