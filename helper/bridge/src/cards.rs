//! Cards: the tags of one request grouped into rows, with the plate pixels the overlay may paint.
//! The algorithm (normative in docs/BRIDGE.md, design steps 7-11 with the P2 changes):
//! 7. plate start: Pobs = per-channel median of `[x1+2, x1+2+max(3, bw)] x [cy - h/4, cy + h/4]`.
//!    It is a plate when its spread is at most SPREAD_MAX and either
//!    - it matches the prediction: the tag's blocks give, per channel, the low reference lo (the
//!      ground seen through a dark block, taken from the block nearest the plate) and the
//!      contrast d = hi - lo, and a plate faded over any ground reads `lo + d * PLATE_RGB / 255`;
//!    - or it passes the design's hue rule (B > G > R, B - R >= 10 alpha, G - R >= 3 alpha, every
//!      channel <= 150), kept for display curves that bend the prediction;
//!    and it is not a cover: no column of the sample window holds an edge, and the plate-start
//!    colour does not fill the rows just above or below the row where the card has no row (a
//!    plate is one row tall). A row next to a covered or non-plate row, with the same Pobs, is
//!    covered too. Mask over yTop..yBot from x1+1 to the view's right edge: pixels within
//!    `min(14 + round(10 (1 - alpha)), 6 + 2 spread)` of the plate colour followed along the row
//!    (the ground's texture shows through a faded plate) and, when they can be told apart, nearer
//!    to it than to the ground behind the plate (`(P - d PLATE / 255) / (1 - alpha)`, or the
//!    indent's ground for a nearly opaque plate); holes of 2 px or less closed when near the plate
//!    colour. The text goes from x1+3 to where the plate run that starts within 6 px of x1 ends
//!    (judged on the 3 middle rows, over a look-ahead window; a short ground-like stretch inside a
//!    faded plate does not end it, a cover or a border does);
//! 8. gaps of 4 px or less between consecutive rows are filled where both neighbouring masks are set
//!    (on either of each band's two rows next to the gap);
//! 9. no plate expected: text from x1+3 to view.right-4, no mask, filled with the median of a
//!    6 x h/2 patch at view.right-10;
//! 10. plate expected (or unknown slot) but not detected: the row is dropped;
//! 11. hits group by (view, slot), ordered by y; a second group of the same slot more than
//!     `1.5 * K * rowPx` away, or a row index that does not increase, starts another card.
//! Rows whose tag alpha is under 0.3 are dropped (the chat line has faded out).

use std::collections::{HashMap, VecDeque};

use crate::{image, msg, tags};

/// Per-pixel paint permission over `rect`: 0 or 255, row-major, `rect.w * rect.h` bytes.
#[derive(Clone, Debug)]
pub struct Mask {
    pub rect: image::Rect,
    pub bits: Vec<u8>,
}

impl Mask {
    /// True when (x, y) (client px) is inside the rect and set.
    pub fn get(&self, x: i32, y: i32) -> bool {
        let (lx, ly) = (x - self.rect.x, y - self.rect.y);
        lx >= 0 && ly >= 0 && lx < self.rect.w && ly < self.rect.h && self.bits[(ly * self.rect.w + lx) as usize] != 0
    }

    /// Number of set pixels.
    pub fn count(&self) -> usize {
        self.bits.iter().filter(|&&b| b != 0).count()
    }
}

/// One reserved row of a card.
#[derive(Clone, Debug)]
pub struct RowBand {
    pub row: u8,
    /// Where the translation text of this row goes.
    pub text_rect: image::Rect,
    /// Plate pixels visible in this capture; None in no-plate mode.
    pub mask: Option<Mask>,
    /// The tag and its blended edges: columns x0 - 1 ..= x1 and rows yTop..=yBot, plus the row
    /// above or below when it is a partial row of the tag. The overlay paints it opaque with the
    /// observed plate colour (Pobs already includes the chat fade) when all 8 blocks passed the
    /// uniformity check (always true for a band: a tag is only reported when they did), so the
    /// tag is hidden once read.
    pub tag_rect: image::Rect,
}

/// All visible rows of one request in one chat view.
#[derive(Clone, Debug)]
pub struct Card {
    pub slot: u8,
    pub view_id: u8,
    /// Rows ordered top to bottom.
    pub bands: Vec<RowBand>,
    /// Small gaps between consecutive rows, filled with the plate colour.
    pub gaps: Vec<image::Rect>,
    pub alpha: f32,
    pub plate: bool,
    /// Observed plate colour (Pobs), or the fill colour in no-plate mode.
    pub plate_rgb: [u8; 3],
    pub text_width_px: i32,
}

/// Tags whose alpha is below this are not shown.
pub const ALPHA_MIN: f32 = 0.3;
/// Largest gap between two rows of one card that is filled.
pub const GAP_FILL_MAX: i32 = 4;
/// Smallest mask tolerance (plus twice the plate's own spread; capped by the design's value).
pub const TOL_MIN: i32 = 6;

struct Bounds {
    id: u8,
    right: i32,
    row_px: f32,
    /// The view's vertical extent (client px, bottom exclusive).
    top: i32,
    bottom: i32,
}

fn median(v: &mut [u8]) -> u8 {
    if v.is_empty() {
        return 0;
    }
    let k = v.len() / 2;
    *v.select_nth_unstable(k).1
}

/// Pixels of a small rectangle collected per channel on the stack (None when empty or larger
/// than SMALL pixels).
const SMALL: usize = 256;
fn small_rect(img: &image::Bgra, x0: i32, x1: i32, y0: i32, y1: i32) -> Option<([[u8; SMALL]; 3], usize)> {
    let mut ch = [[0u8; SMALL]; 3];
    let mut n = 0;
    for y in y0..=y1 {
        for x in x0..=x1 {
            if let Some(p) = img.rgb(x, y) {
                if n == SMALL {
                    return None;
                }
                for c in 0..3 {
                    ch[c][n] = p[c];
                }
                n += 1;
            }
        }
    }
    (n > 0).then_some((ch, n))
}

/// Per-channel median of a rectangle and the largest distance of its pixels from that median
/// (a quarter of the pixels, the farthest, are ignored: they may be edge pixels).
fn median_spread(img: &image::Bgra, x0: i32, x1: i32, y0: i32, y1: i32) -> Option<([u8; 3], i32)> {
    let m = median_rgb(img, x0, x1, y0, y1)?;
    let mut d: Vec<i32> = Vec::new();
    for y in y0..=y1 {
        for x in x0..=x1 {
            if let Some(p) = img.rgb(x, y) {
                d.push((0..3).map(|c| (i32::from(p[c]) - i32::from(m[c])).abs()).max().unwrap_or(0));
            }
        }
    }
    d.sort_unstable();
    let spread = d[(d.len() * 3 / 4).min(d.len() - 1)];
    Some((m, spread))
}

/// Per-channel median of the pixels of a rectangle (clipped to the capture).
fn median_rgb(img: &image::Bgra, x0: i32, x1: i32, y0: i32, y1: i32) -> Option<[u8; 3]> {
    if (x1 - x0 + 1).max(0) as i64 * (y1 - y0 + 1).max(0) as i64 <= SMALL as i64 {
        let (mut ch, n) = small_rect(img, x0, x1, y0, y1)?;
        let [a, b, c] = &mut ch;
        return Some([median(&mut a[..n]), median(&mut b[..n]), median(&mut c[..n])]);
    }
    let mut ch: [Vec<u8>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for y in y0..=y1 {
        for x in x0..=x1 {
            if let Some(p) = img.rgb(x, y) {
                for c in 0..3 {
                    ch[c].push(p[c]);
                }
            }
        }
    }
    if ch[0].is_empty() {
        return None;
    }
    Some([median(&mut ch[0]), median(&mut ch[1]), median(&mut ch[2])])
}

/// The plate rule: B > G > R, B - R >= 10 alpha, every channel <= 150, plus G - R >= 3 alpha
/// (the plate has G - R = 8; the default tooltip backdrop (23,23,48) has 0 and passes the rest
/// even when 5% of the plate shows through it).
fn is_plate(p: [u8; 3], alpha: f32) -> bool {
    let [r, g, b] = [i32::from(p[0]), i32::from(p[1]), i32::from(p[2])];
    b > g
        && g > r
        && (b - r) as f32 >= 10.0 * alpha
        && (g - r) as f32 >= 3.0 * alpha
        && r <= 150
        && g <= 150
        && b <= 150
}

/// The plate colour on channel `c` as a fraction of the tag's own contrast.
fn plate_t(c: usize) -> f32 {
    f32::from(tags::PLATE_RGB[c]) / 255.0
}

/// A tag's blocks read again from the capture, for the plate prediction. With the chat line
/// faded to alpha over a ground g, a block drawn 0 on a channel reads (1 - alpha) g and one drawn
/// 255 reads alpha 255 + (1 - alpha) g, so the plate (drawn PLATE_RGB) reads
/// `lo + (hi - lo) * PLATE_RGB / 255` whatever the ground's colour.
struct TagRefs {
    /// The sync pair's low reference per channel (R and B from B1, G from B0).
    lo: [f32; 3],
    /// The tag's contrast per channel: high reference minus low reference.
    d: [f32; 3],
    /// Per channel, the low reference of the block nearest to the plate that is drawn 0 on that
    /// channel: the ground just before the plate, seen through the faded tag.
    lo_near: [f32; 3],
}

impl TagRefs {
    fn predicted_plate(&self, c: usize) -> f32 {
        self.lo_near[c] + self.d[c] * plate_t(c)
    }
}

/// Mean colour of the pixels fully inside a block whose left edge is `l` (known to +-0.5 px).
fn block_mean(img: &image::Bgra, l: f32, bw: f32, y0: i32, y1: i32) -> Option<[f32; 3]> {
    let (mut a, mut b) = ((l + 0.5).ceil() as i32, (l + bw - 0.5).floor() as i32 - 1);
    if b < a {
        a = (l + bw / 2.0 - 0.5).round() as i32;
        b = a;
    }
    let (mut s, mut n) = ([0f32; 3], 0f32);
    for y in y0..=y1 {
        for x in a..=b {
            let p = img.rgb(x, y)?;
            for c in 0..3 {
                s[c] += f32::from(p[c]);
            }
            n += 1.0;
        }
    }
    (n > 0.0).then(|| [s[0] / n, s[1] / n, s[2] / n])
}

fn tag_refs(img: &image::Bgra, h: &tags::TagHit) -> Option<TagRefs> {
    let cy = (h.y_top + h.y_bot) / 2;
    let q = (h.y_bot - h.y_top + 1) / 4;
    // x0 and x1 are both rounded: their mean locates B0 to +-0.5 px
    let l0 = (h.x0 as f32 + (h.x1 as f32 - 7.0 * h.pitch_px - h.block_px)) / 2.0;
    let mut blocks = [[0f32; 3]; 8];
    for (i, b) in blocks.iter_mut().enumerate() {
        *b = block_mean(img, l0 + i as f32 * h.pitch_px, h.block_px, cy - q, cy + q)?;
    }
    let hi = [blocks[0][0], blocks[1][1], blocks[0][2]];
    let lo = [blocks[1][0], blocks[0][1], blocks[1][2]];
    let d = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
    let cols = tags::tag_colors(h.slot, h.row, h.sess_bit);
    let mut lo_near = lo;
    for (c, v) in lo_near.iter_mut().enumerate() {
        if let Some(i) = (2..8).rev().find(|&i| cols[i][c] == 0) {
            *v = blocks[i][c];
        }
    }
    Some(TagRefs { lo, d, lo_near })
}

/// Largest per-channel difference.
fn dist3(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|c| (a[c] - b[c]).abs()).fold(0.0, f32::max)
}

fn rgbf(p: [u8; 3]) -> [f32; 3] {
    [f32::from(p[0]), f32::from(p[1]), f32::from(p[2])]
}

/// Prediction tolerance: base, plus the sampled plate's own spread, plus half of how much the
/// ground varies along the tag, plus a share of (1 - alpha) for the ground's texture between
/// the last dark block and the plate sample.
pub const PRED_TOL: f32 = 5.0;
pub const PRED_FADE: f32 = 8.0;
/// The plate sample's spread counts up to this much in the tolerances.
pub const SPREAD_CAP: i32 = 8;
/// A plate start that varies more than this (a border or an edge in the sample) is not a plate.
pub const SPREAD_MAX: i32 = 16;

/// How fast the followed plate colour moves toward each plate column.
pub const TRACK: f32 = 0.25;
/// Columns looked ahead to decide where the plate ends.
pub const END_WIN: usize = 8;
/// Longest stretch of ground-like columns inside a faded plate (the ground's texture).
pub const MAX_BLIP: usize = 32;
/// Smallest distance between the plate and the ground hypothesis for the two-way test (else
/// the tolerance alone decides).
pub const SEP_MIN: f32 = 8.0;
/// Above this alpha the plate is nearly opaque and the ground behind it cannot be estimated.
pub const TWO_HYP_ALPHA: f32 = 0.85;
/// Tolerance (plus the plate's spread) for "this neighbouring row has the plate start's colour".
pub const COVER_TOL: i32 = 6;

/// True when the colour sampled at the plate start also fills the rows just above or below the
/// row, where no other row of this card is: a menu or tooltip covering the plate start, not a
/// plate (a plate is exactly one row tall). A neighbouring row counts when its colour is within
/// the tolerance of Pobs and nearer to Pobs than to `ground` (the ground behind the plate start,
/// which is what a real plate has around it); a plate-like run that reaches the view edge within
/// 1.6 rows is a clipped row of the card. Inconclusive (false) when Pobs and the ground are too
/// close to tell apart.
#[allow(clippy::too_many_arguments)]
fn covered_start(
    img: &image::Bgra,
    h: &tags::TagHit,
    b: &Bounds,
    others: &[(i32, i32)],
    pobs: [u8; 3],
    spread: i32,
    ground: Option<[f32; 3]>,
    (sx0, sx1): (i32, i32),
) -> bool {
    let tol = cover_tol(spread);
    let pf = rgbf(pobs);
    if ground.is_some_and(|g| dist3(pf, g) < SEP_MIN.max(tol)) {
        return false;
    }
    let inside = |y: i32| y >= b.top && y < b.bottom && y >= img.origin_y && y < img.origin_y + img.height as i32;
    let like = |y: i32| {
        median_rgb(img, sx0, sx1, y, y).is_some_and(|m| {
            let (m, dp) = (rgbf(m), dist3(rgbf(m), pf));
            dp <= tol && ground.is_none_or(|g| 2.0 * dp < dist3(m, g))
        })
    };
    let member = |y: i32| others.iter().any(|&(t, bt)| y >= t - 1 && y <= bt + 1);
    let reach = (1.6 * b.row_px).ceil() as i32;
    for (y0, step) in [(h.y_top - 2, -1), (h.y_bot + 2, 1)] {
        let y1 = y0 + step;
        // only a row of the card can be clipped by the view edge, and row 0 has none above it
        let may_clip = step > 0 || h.row > 0;
        if !inside(y0) || !inside(y1) || member(y0) || member(y1) || !(like(y0) && like(y1)) {
            continue;
        }
        let mut y = y1;
        let mut clipped = false;
        for _ in 0..reach {
            let ny = y + step;
            if !inside(ny) {
                clipped = may_clip;
                break;
            }
            if !like(ny) {
                break;
            }
            y = ny;
        }
        if !clipped {
            return true;
        }
    }
    false
}

/// True when Pobs is the plate faded over the ground seen just before it (any ground colour).
fn matches_prediction(pobs: [u8; 3], spread: i32, r: &TagRefs, alpha: f32) -> bool {
    (0..3).all(|c| {
        let tol = PRED_TOL
            + spread.min(SPREAD_CAP) as f32
            + 0.5 * (r.lo_near[c] - r.lo[c]).abs()
            + PRED_FADE * (1.0 - alpha);
        (f32::from(pobs[c]) - r.predicted_plate(c)).abs() <= tol
    })
}

/// Groups tag hits into cards and measures their plates.
/// `plate_expected(slot)`: Some(true/false) from the TRANSLATE flags, None for an unknown slot.
/// `sess_bit`: the current session's bit; hits with the other bit are ignored when Some.
pub fn build(
    img: &image::Bgra,
    hits: &[tags::TagHit],
    views: &[msg::View],
    plate_expected: &dyn Fn(u8) -> Option<bool>,
    sess_bit: Option<u8>,
) -> Vec<Card> {
    let img_right = img.origin_x + img.width as i32;
    let bounds_of = |h: &tags::TagHit| -> Bounds {
        let cy = (h.y_top + h.y_bot) / 2;
        for v in views {
            let (vx, vy, vw, vh) = (i32::from(v.x), i32::from(v.y), i32::from(v.w), i32::from(v.h));
            if h.x0 >= vx && h.x0 < vx + vw && cy >= vy && cy < vy + vh {
                let row = if v.row_px > 0 { f32::from(v.row_px) } else { (h.y_bot - h.y_top + 1) as f32 * 1.25 };
                return Bounds { id: v.id, right: (vx + vw).min(img_right), row_px: row, top: vy, bottom: vy + vh };
            }
        }
        Bounds {
            id: 0,
            right: img_right,
            row_px: (h.y_bot - h.y_top + 1) as f32 * 1.25,
            top: img.origin_y,
            bottom: img.origin_y + img.height as i32,
        }
    };
    // group by (view, slot), ordered by y
    let mut groups: Vec<((u8, u8), Vec<(&tags::TagHit, Bounds)>)> = Vec::new();
    for h in hits {
        if h.alpha < ALPHA_MIN || sess_bit.is_some_and(|s| s != h.sess_bit) {
            continue;
        }
        let b = bounds_of(h);
        let key = (b.id, h.slot);
        match groups.iter_mut().find(|g| g.0 == key) {
            Some(g) => g.1.push((h, b)),
            None => groups.push((key, vec![(h, b)])),
        }
    }
    let mut cards = Vec::new();
    for (key, mut members) in groups {
        members.sort_by_key(|m| (m.0.y_top, m.0.x0));
        let k_rows = members.iter().map(|m| m.0.row).max().unwrap_or(0) as f32 + 1.0;
        let mut current: Vec<(&tags::TagHit, Bounds)> = Vec::new();
        let flush = |cur: &mut Vec<(&tags::TagHit, Bounds)>, cards: &mut Vec<Card>| {
            if !cur.is_empty() {
                if let Some(c) = make_card(img, key, cur, plate_expected) {
                    cards.push(c);
                }
                cur.clear();
            }
        };
        for m in members {
            if let Some(prev) = current.last() {
                let far = (m.0.y_top - prev.0.y_top) as f32 > 1.5 * k_rows * m.1.row_px;
                if far || m.0.row <= prev.0.row {
                    flush(&mut current, &mut cards);
                }
            }
            current.push(m);
        }
        flush(&mut current, &mut cards);
    }
    cards
}

/// The area the overlay paints over to hide a tag: the tag (x0..x1, yTop..yBot) plus its blended
/// edges. x0 and x1 are rounded sub-pixel edges, so column x0 - 1 can hold part of B0 and column
/// x1 part of B7: both are always included. A row just above or below is included when it is a
/// partial row of the tag (a consistent blend of the ground and every block's colour), never
/// when it holds something else (the text of the line above, a cover).
fn tag_rect_of(img: &image::Bgra, h: &tags::TagHit) -> image::Rect {
    let top = if partial_row(img, h, h.y_top - 1, -1) { h.y_top - 1 } else { h.y_top };
    let bot = if partial_row(img, h, h.y_bot + 1, 1) { h.y_bot + 1 } else { h.y_bot };
    image::Rect { x: h.x0 - 1, y: top, w: (h.x1 - h.x0 + 2).max(0), h: bot - top + 1 }
}

/// Smallest block-to-ground contrast that tells something about a blend.
const PARTIAL_MIN_CONTRAST: f32 = 40.0;
/// Largest distance of a pixel from the ground-to-block blend line.
const PARTIAL_RESIDUAL: f32 = 24.0;
/// Largest spread of the blend fractions over the blocks of a partial row.
const PARTIAL_SPREAD: f32 = 0.2;

/// True when row `r` is a partial (anti-aliased) row of the tag: at every block's centre the
/// pixel is ground + f * (block - ground) with the same f (at least 0.08) for all blocks. The
/// ground of each block is read one row further out (`dir` = -1 above the tag, 1 below), so a
/// textured background behind the chat does not disturb the test.
fn partial_row(img: &image::Bgra, h: &tags::TagHit, r: i32, dir: i32) -> bool {
    let cy = (h.y_top + h.y_bot) / 2;
    let mut fs: Vec<f32> = Vec::with_capacity(8);
    for i in 0..8 {
        let cx = (h.x0 as f32 + i as f32 * h.pitch_px + h.block_px / 2.0 - 0.5).round() as i32;
        let (Some(c), Some(p), Some(g)) =
            (img.rgb(cx, cy).map(rgbf), img.rgb(cx, r).map(rgbf), img.rgb(cx, r + dir).map(rgbf))
        else {
            return false;
        };
        let d = [c[0] - g[0], c[1] - g[1], c[2] - g[2]];
        let n2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
        if n2 < PARTIAL_MIN_CONTRAST * PARTIAL_MIN_CONTRAST {
            continue;
        }
        let f = ((p[0] - g[0]) * d[0] + (p[1] - g[1]) * d[1] + (p[2] - g[2]) * d[2]) / n2;
        let resid = (0..3).map(|k| (p[k] - (g[k] + f * d[k])).abs()).fold(0.0, f32::max);
        if resid > PARTIAL_RESIDUAL {
            return false;
        }
        fs.push(f);
    }
    if fs.len() < 4 {
        return false;
    }
    let mean = fs.iter().sum::<f32>() / fs.len() as f32;
    mean >= 0.08 && fs.iter().all(|f| (f - mean).abs() <= PARTIAL_SPREAD)
}

/// Tolerance for "the same colour as the plate start" (row medians: little noise).
fn cover_tol(spread: i32) -> f32 {
    (COVER_TOL + spread.min(COVER_TOL)) as f32
}

/// What a row's plate start looks like.
struct Probe {
    /// Observed plate colour (per-channel median of the sample window).
    pobs: [u8; 3],
    /// How much the sample window varies around Pobs.
    spread: i32,
    refs: Option<TagRefs>,
    /// The ground in the indent left of the tag, on the same rows.
    ground: Option<[u8; 3]>,
    /// Passes the colour rules (prediction or hue) with a uniform enough sample.
    plate: bool,
    /// The plate-start colour also fills a row above or below where the card has no row, or the
    /// sample window holds an edge.
    covered: bool,
}

/// Samples a row's plate start; None when nothing can be sampled.
fn probe(img: &image::Bgra, h: &tags::TagHit, b: &Bounds, members: &[(&tags::TagHit, Bounds)]) -> Option<Probe> {
    let cy = (h.y_top + h.y_bot) / 2;
    let q = (h.y_bot - h.y_top + 1) / 4;
    let bwi = (h.block_px.round() as i32).max(3);
    let (sx0, sx1) = (h.x1 + 2, h.x1 + 2 + bwi);
    let (pobs, spread) = median_spread(img, sx0, sx1, cy - q, cy + q)?;
    let refs = tag_refs(img, h);
    // the plate faded over the ground seen just before it (any ground), or the design's hue rule
    // (kept: it holds under display curves that bend the prediction)
    let predicted = refs.as_ref().is_some_and(|r| matches_prediction(pobs, spread, r, h.alpha));
    let plate = spread <= SPREAD_MAX && (predicted || is_plate(pobs, h.alpha));
    // every column of the sample window (after its first, which may hold the gap after B7) has
    // the plate start's colour: an edge or a border inside it is a cover starting there
    let col_tol = cover_tol(spread) + 4.0;
    let edge = ((sx0 + 1)..=sx1)
        .any(|x| median_rgb(img, x, x, cy - q, cy + q).is_some_and(|m| dist3(rgbf(m), rgbf(pobs)) > col_tol));
    let others: Vec<(i32, i32)> =
        members.iter().filter(|m| !std::ptr::eq(m.0, h)).map(|m| (m.0.y_top, m.0.y_bot)).collect();
    let ground = median_rgb(img, h.x0 - 5, h.x0 - 2, cy - q, cy + q);
    // the ground just before the plate, seen through the tag's dark blocks (independent of
    // what covers the plate start), else (a nearly opaque tag) the ground in the indent
    let local = match &refs {
        Some(r) if h.alpha <= TWO_HYP_ALPHA => {
            let k = 1.0 - h.alpha;
            Some([0, 1, 2].map(|c| (r.lo_near[c] / k).clamp(0.0, 255.0)))
        }
        _ => ground.map(rgbf),
    };
    let covered = plate && (edge || covered_start(img, h, b, &others, pobs, spread, local, (sx0, sx1)));
    Some(Probe { pobs, spread, refs, ground, plate, covered })
}

/// The mask and the text rectangle of one plate.
fn plate_band(img: &image::Bgra, h: &tags::TagHit, b: &Bounds, pr: &Probe) -> Option<RowBand> {
    let height = h.y_bot - h.y_top + 1;
    let cy = (h.y_top + h.y_bot) / 2;
    let (pobs, spread) = (pr.pobs, pr.spread);
    // the design's tolerance, tightened to what the plate itself varies by: a solid plate
    // captures exactly, and the default tooltip backdrop (23,23,48) is only 16 away from it
    let tol = (14 + (10.0 * (1.0 - h.alpha)).round() as i32).min(TOL_MIN + 2 * spread);
    let tolf = tol as f32;
    let mx0 = h.x1 + 1;
    let mw = b.right.min(img.origin_x + img.width as i32) - mx0;
    if mw <= 0 || h.y_top < img.origin_y || h.y_bot >= img.origin_y + img.height as i32 {
        return None;
    }
    let n = mw as usize;
    // Where the plate ends, the row shows the ground it was blended over. Under a faded plate
    // that ground is (P - d PLATE / 255) / (1 - alpha) for a plate pixel P (followed along the
    // row: the ground has a texture); under a nearly opaque plate, the ground in the indent left
    // of the tag stands for it. Each pixel goes to whichever of the two is nearer when they are
    // far enough apart to tell; else the tolerance alone decides.
    let faded = h.alpha <= TWO_HYP_ALPHA;
    let indent = pr.ground.map(rgbf);
    let pf = rgbf(pobs);
    let sep_min = SEP_MIN.max(3.0 * spread as f32);
    // the ground hypothesis next to a plate colour, when far enough from it to tell them apart
    let (dpl, inv_k) = match &pr.refs {
        Some(r) => ([0, 1, 2].map(|c| r.d[c] * plate_t(c)), 1.0 / (1.0 - h.alpha).max(0.05)),
        None => ([0.0; 3], 0.0),
    };
    let has_refs = pr.refs.is_some();
    let ground_at = |pref: [f32; 3]| -> Option<[f32; 3]> {
        // the ground behind a plate pixel p faded to alpha: (p - d PLATE / 255) / (1 - alpha)
        let g = if faded {
            if !has_refs {
                return None;
            }
            [0, 1, 2].map(|c| ((pref[c] - dpl[c]) * inv_k).clamp(0.0, 255.0))
        } else {
            indent?
        };
        (dist3(pref, g) >= sep_min).then_some(g)
    };
    // the pixel pass compares against these per column; no ground hypothesis = a far colour
    const FAR: i32 = 1 << 20;
    // byte offset of (mx0, y); x and y are inside the capture here
    let stride = img.stride as usize;
    let at = |y: i32| (y - img.origin_y) as usize * stride + (mx0 - img.origin_x) as usize * 4;
    // 1. per column: the plate colour followed along the row, from the median of the 3 middle
    //    rows (a cover over the middle row stops the text), and the ground hypothesis there
    let mut pref_col = vec![[0i32; 3]; n];
    let mut gref_col = vec![[FAR; 3]; n];
    let mut plate_col = vec![false; n];
    // columns that are neither the plate nor the ground behind it (a cover, a border)
    let mut other_col = vec![false; n];
    let mut pref = pf;
    let mut g = ground_at(pref);
    let (r0, r1) = ((cy - 1).max(img.origin_y), (cy + 1).min(img.origin_y + img.height as i32 - 1));
    let (o0, o1, o2) = (at(r0), at((r0 + 1).min(r1)), at(r1));
    let px = &img.px;
    for rx in 0..n {
        // (values are 0..255: adding 0.5 and truncating rounds them)
        pref_col[rx] = pref.map(|v| (v + 0.5) as i32);
        if let Some(g) = g {
            gref_col[rx] = g.map(|v| (v + 0.5) as i32);
        }
        // median of the 3 middle rows (R at +2, G at +1, B at +0 of each BGRA pixel)
        let k = 4 * rx;
        let med = |c: usize| {
            let (a, b2, d) = (px[o0 + k + 2 - c], px[o1 + k + 2 - c], px[o2 + k + 2 - c]);
            f32::from(a.max(b2).min(a.min(b2).max(d)))
        };
        let col = [med(0), med(1), med(2)];
        let dp = dist3(col, pref);
        let dg = g.map(|g| dist3(col, g));
        let is = dp <= tolf && dg.is_none_or(|dg| dp <= dg);
        plate_col[rx] = is;
        other_col[rx] = !is && dp > tolf && dg.is_none_or(|dg| dg > tolf);
        // follow the ground's texture under the plate with the columns that are clearly plate
        // (half the tolerance, and at most half as far from the plate as from the ground), never
        // further than the tolerance from Pobs
        if is && dp <= 0.5 * tolf && dg.is_none_or(|dg| 2.0 * dp <= dg) {
            for c in 0..3 {
                pref[c] = (pref[c] + TRACK * (col[c] - pref[c])).clamp(pf[c] - tolf, pf[c] + tolf);
            }
            g = ground_at(pref);
        }
    }
    // 2. the mask: each pixel against its column's plate colour and ground hypothesis
    let mut bits = vec![0u8; n * height as usize];
    let mut dist = vec![0i32; n];
    for ry in 0..height as usize {
        let base = at(h.y_top + ry as i32);
        let src = &img.px[base..base + 4 * n];
        let row = &mut bits[ry * n..(ry + 1) * n];
        for ((((p, q), g), d), cell) in
            src.chunks_exact(4).zip(&pref_col).zip(&gref_col).zip(dist.iter_mut()).zip(row.iter_mut())
        {
            let (r, gg, bb) = (i32::from(p[2]), i32::from(p[1]), i32::from(p[0]));
            let dp = (r - q[0]).abs().max((gg - q[1]).abs()).max((bb - q[2]).abs());
            *d = dp;
            *cell = if dp <= tol && dp <= (r - g[0]).abs().max((gg - g[1]).abs()).max((bb - g[2]).abs()) { 255 } else { 0 };
        }
        // holes of 2 px or less are closed when they are near the plate colour (noise), not
        // across a real occluder such as a frame border
        close_holes(row, 2, |i| dist[i] <= 2 * tol);
    }
    // 3. text: from x1+3 to the end of the plate run that starts within 6 px of x1. The run ends
    //    at the first column from which at least 3/4 of the next END_WIN columns are not plate,
    //    so a few misread columns neither end it early nor carry it into the ground. A stretch of
    //    up to MAX_BLIP columns that look like the ground, after which the plate resumes, is the
    //    ground's texture showing through a faded plate, not its end; a cover or a border ends it.
    let mut text_right = h.x1 + 3;
    if let Some(start) = (0..n.min(6)).find(|&i| plate_col[i]) {
        let (mut nonp, mut oth) = (vec![0u32; n + 1], vec![0u32; n + 1]);
        for i in 0..n {
            nonp[i + 1] = nonp[i] + u32::from(!plate_col[i]);
            oth[i + 1] = oth[i] + u32::from(other_col[i]);
        }
        let ends_at = |e: usize| {
            let w = (e + END_WIN).min(n);
            !plate_col[e] && 4 * (nonp[w] - nonp[e]) >= 3 * (w - e) as u32
        };
        let resumes_at = |f: usize| {
            let w = f + END_WIN;
            w <= n && plate_col[f] && 4 * (nonp[w] - nonp[f]) <= (w - f) as u32
        };
        let mut pos = start;
        let end = loop {
            let Some(e) = (pos..n).find(|&e| ends_at(e)) else { break n };
            match (e..(e + MAX_BLIP + 1).min(n)).find(|&f| resumes_at(f)) {
                Some(f) if oth[f] == oth[e] => pos = f,
                _ => break e,
            }
        };
        let last = (start..end).rev().find(|&i| plate_col[i]).unwrap_or(start);
        text_right = mx0 + last as i32 + 1;
    }
    let x = h.x1 + 3;
    Some(RowBand {
        row: h.row,
        text_rect: image::Rect { x, y: h.y_top, w: (text_right - x).max(0), h: height },
        mask: Some(Mask { rect: image::Rect { x: mx0, y: h.y_top, w: mw, h: height }, bits }),
        tag_rect: tag_rect_of(img, h),
    })
}

fn make_card(
    img: &image::Bgra,
    key: (u8, u8),
    members: &[(&tags::TagHit, Bounds)],
    plate_expected: &dyn Fn(u8) -> Option<bool>,
) -> Option<Card> {
    let (view_id, slot) = key;
    let expected = plate_expected(slot);
    let plate_mode = expected != Some(false);
    let mut bands = Vec::new();
    let mut pobs_list = Vec::new();
    let mut alphas = Vec::new();
    if !plate_mode {
        for (h, b) in members {
            let height = h.y_bot - h.y_top + 1;
            let cy = (h.y_top + h.y_bot) / 2;
            let q = height / 4;
            let fill = median_rgb(img, b.right - 13, b.right - 8, cy - q, cy + q).unwrap_or(tags::PLATE_RGB);
            pobs_list.push(fill);
            alphas.push(h.alpha);
            let x = h.x1 + 3;
            bands.push(RowBand {
                row: h.row,
                text_rect: image::Rect { x, y: h.y_top, w: (b.right - 4 - x).max(0), h: height },
                mask: None,
                tag_rect: tag_rect_of(img, h),
            });
        }
    } else {
        // 1. each row's plate start: sampled, tested against the prediction and the hue rule,
        //    and checked for a cover (the same colour above or below the row)
        let probes: Vec<Option<Probe>> = members.iter().map(|(h, b)| probe(img, h, b, members)).collect();
        // 2. a row next to a covered or non-plate row, with the same colour at its plate start, is
        //    covered too (the middle rows of a cover over several rows have no free side to test)
        let mut covered: Vec<bool> = probes.iter().map(|p| p.as_ref().is_none_or(|p| !p.plate || p.covered)).collect();
        loop {
            let mut changed = false;
            for i in 0..members.len() {
                let Some(pi) = &probes[i] else { continue };
                if covered[i] {
                    continue;
                }
                let near = |j: usize| {
                    covered[j]
                        && probes[j].as_ref().is_some_and(|pj| {
                            let gap = (members[i].0.y_top - members[j].0.y_bot).max(members[j].0.y_top - members[i].0.y_bot);
                            gap <= GAP_FILL_MAX + 2 && dist3(rgbf(pi.pobs), rgbf(pj.pobs)) <= cover_tol(pi.spread.max(pj.spread))
                        })
                };
                if (i > 0 && near(i - 1)) || (i + 1 < members.len() && near(i + 1)) {
                    covered[i] = true;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        // 3. the mask and the text rectangle of every plate that is not covered
        for (i, (h, b)) in members.iter().enumerate() {
            let Some(p) = &probes[i] else { continue };
            if covered[i] {
                continue;
            }
            let Some(band) = plate_band(img, h, b, p) else { continue };
            pobs_list.push(p.pobs);
            alphas.push(h.alpha);
            bands.push(band);
        }
    }
    if bands.is_empty() {
        return None;
    }
    // gap fill between consecutive rows
    let mut gaps = Vec::new();
    for w in bands.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        // the rows of the tags themselves (tag_rect also covers blended edge rows)
        let top = a.text_rect.y + a.text_rect.h;
        let n = b.text_rect.y - top;
        if n < 1 || n > GAP_FILL_MAX {
            continue;
        }
        let (Some(ma), Some(mb)) = (&a.mask, &b.mask) else { continue };
        let ya = ma.rect.y + ma.rect.h - 1;
        let yb = mb.rect.y;
        // a band's edge row may be partly covered (fractional row positions): use two rows
        let set = |x: i32| (ma.get(x, ya) || ma.get(x, ya - 1)) && (mb.get(x, yb) || mb.get(x, yb + 1));
        let x0 = ma.rect.x.max(mb.rect.x);
        let x1 = (ma.rect.x + ma.rect.w).min(mb.rect.x + mb.rect.w);
        let mut x = x0;
        while x < x1 {
            if set(x) {
                let s = x;
                while x < x1 && set(x) {
                    x += 1;
                }
                gaps.push(image::Rect { x: s, y: top, w: x - s, h: n });
            } else {
                x += 1;
            }
        }
    }
    let mut ch: [Vec<u8>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for p in &pobs_list {
        for c in 0..3 {
            ch[c].push(p[c]);
        }
    }
    let plate_rgb = [median(&mut ch[0]), median(&mut ch[1]), median(&mut ch[2])];
    let alpha = alphas.iter().sum::<f32>() / alphas.len() as f32;
    let text_width_px = bands.iter().map(|b| b.text_rect.w).max().unwrap_or(0);
    Some(Card { slot, view_id, bands, gaps, alpha, plate: plate_mode, plate_rgb, text_width_px })
}

/// Sets every run of `max` or fewer unset cells between two set cells, when every cell of the
/// run passes `near`.
fn close_holes(row: &mut [u8], max: usize, near: impl Fn(usize) -> bool) {
    let n = row.len();
    let mut i = row.iter().position(|&c| c != 0).unwrap_or(n);
    while i < n {
        // past a set run, an unset run [start, i) closed when a set cell follows it
        while i < n && row[i] != 0 {
            i += 1;
        }
        let start = i;
        while i < n && row[i] == 0 {
            i += 1;
        }
        if i < n && i - start <= max && (start..i).all(&near) {
            row[start..i].fill(255);
        }
    }
}

/// Keeps `text_width_px` sticky: the maximum over the card's captures of the last 2 s (design
/// step 7), so a tooltip passing over a plate does not re-wrap the text. Cards are keyed by
/// (view, slot). The caller keeps one instance per scene.
#[derive(Default)]
pub struct StickyWidth {
    seen: HashMap<(u8, u8), VecDeque<(u64, i32)>>,
}

/// How long a measured width is remembered.
pub const STICKY_MS: u64 = 2000;

impl StickyWidth {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records this capture's widths and replaces each card's width with the 2 s maximum.
    pub fn apply(&mut self, cards: &mut [Card], now_ms: u64) {
        for c in cards.iter_mut() {
            let q = self.seen.entry((c.view_id, c.slot)).or_default();
            q.push_back((now_ms, c.text_width_px));
            while q.front().is_some_and(|e| now_ms.saturating_sub(e.0) > STICKY_MS) {
                q.pop_front();
            }
            c.text_width_px = q.iter().map(|e| e.1).max().unwrap_or(c.text_width_px);
        }
        self.seen.retain(|_, q| q.back().is_some_and(|e| now_ms.saturating_sub(e.0) <= STICKY_MS));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holes_closed() {
        let mut r = [255, 0, 0, 255, 0, 0, 0, 255, 0, 255];
        close_holes(&mut r, 2, |i| i != 8);
        assert_eq!(r, [255, 255, 255, 255, 0, 0, 0, 255, 0, 255]);
    }

    #[test]
    fn plate_rule() {
        assert!(is_plate([32, 40, 64], 1.0));
        assert!(!is_plate([23, 23, 48], 1.0)); // tooltip background: G is not above R
        assert!(!is_plate([32, 40, 160], 1.0));
        assert!(!is_plate([40, 40, 64], 1.0));
    }

    #[test]
    fn sticky_width_keeps_the_max_for_2s() {
        let card = |w| Card {
            slot: 3,
            view_id: 1,
            bands: vec![],
            gaps: vec![],
            alpha: 1.0,
            plate: true,
            plate_rgb: [32, 40, 64],
            text_width_px: w,
        };
        let mut s = StickyWidth::new();
        let mut c = [card(300)];
        s.apply(&mut c, 1000);
        let mut c = [card(120)];
        s.apply(&mut c, 1500);
        assert_eq!(c[0].text_width_px, 300);
        let mut c = [card(120)];
        s.apply(&mut c, 3200);
        assert_eq!(c[0].text_width_px, 120);
    }
}
