//! The chat background around each reserved row, sampled in the current capture, so the overlay
//! can make the tags and plates disappear and the translation looks like it sits directly on the
//! chat (the "native look").
//!
//! The plate exists only so that the Helper can find the rows. For each row band of a card:
//! - **left**: the indent between the view's left edge (at most `LEFT_MAX` px away) and the tag;
//! - **right**: the pixels after the plate's right end, up to the view's right edge: the last
//!   `RIGHT_FAR` px of the view (the plate ends about 12% of the chat width before it, and a
//!   faded plate over a busy ground can be measured shorter than it is, so the far end is where
//!   the chat surely shows), never left of the measured plate end, without the pixels the mask
//!   takes for plate, and only when this row's plate reaches the card's plate end (a plate cut
//!   short by a tooltip has the tooltip after it, not the chat) and the card's plate end is
//!   about where the addon draws it (`PLATE_FRACTION`): when every row is cut at the same x
//!   (a frame docked over the right of the chat, always the case for a one-row card under such
//!   a frame), the far end of the view lies under that frame;
//! - in the discovery scan (no VIEW: the view is the whole client width) there is no right
//!   sample, and the left span stays within about one em of the tag (`DISCOVERY_LEFT`), the
//!   indent that is surely chat;
//! - per-channel median of each side, after dropping the pixels that look like text (bright or
//!   saturated outliers, or far from the first median: a glyph or its shadow of a neighbouring
//!   line reaching into the row).
//!
//! The overlay fills the tag, the visible plate pixels and the gaps between rows with a
//! horizontal linear gradient between the two samples (one sample: flat). A plate drawn at a
//! fraction of a pixel also leaves blended pixels just outside the mask (which only takes pixels
//! near the plate colour): the row above and below it and the column after its end. They are
//! looked for only where the edge is known to be fractional: a row above or below when the tag
//! has a partial row there (`cards` measured it on all eight blocks, so a tooltip or a borderless
//! panel starting right next to the plate does not count), the end column when its pixels hold
//! one consistent blend fraction down the band and the column after it does not. Each such pixel
//! that lies between the observed plate colour and the chat colour there (a blend of the two,
//! nothing else: a tooltip edge or a glyph is not) is listed in `CardBg::edges` and painted too.
//!
//! Cost: the side spans are sampled every `SPAN_STEP` columns; the edge rows are scanned only
//! when fractional. Samples within `SETTLE` levels of the previous capture's keep their previous
//! value ([`settle`]), so a game world moving behind a translucent chat does not republish the
//! scene (and re-render the overlay) at every capture.
//!
//! Every fill is opaque: the samples come from the same capture, so they already show the chat
//! line's fade, and a translucent fill would leave a ghost of the faded plate (see
//! `docs\HELPER.md`, deviations).

use tp_bridge::cards::Card;
use tp_bridge::image::{Bgra, Rect};

/// The left sample spans at most this many px left of the tag.
pub const LEFT_MAX: i32 = 48;
/// The right sample spans the last this many px of the view (right of the plate end).
pub const RIGHT_FAR: i32 = 40;
/// Columns skipped next to the tag and the plate (blended edge columns).
pub const EDGE_SKIP: i32 = 2;
/// A row's plate counts as reaching the card's plate end within this many px.
pub const END_SLACK: i32 = 3;
/// Fewest pixels a side needs to give a sample.
pub const MIN_PIXELS: usize = 6;
/// The addon sizes a plate to this fraction of the room right of the tag (`Reserve.Metrics`:
/// 0.88 x (chat width - indent - tag)), so a whole plate ends about 12% of that room before the
/// view's right edge.
pub const PLATE_FRACTION: f32 = 0.88;
/// A card whose widest plate ends more than this many px before where the addon draws it is cut
/// by something over the chat (measured in game: 1 px short): no right sample.
pub const CUT_SLACK: i32 = 24;
/// Discovery scan: the left span reaches at most this fraction of the band height left of the
/// tag (the indent of four spaces in the chat font is about one em wide).
pub const DISCOVERY_LEFT: f32 = 0.75;
/// The side spans are sampled every this many columns (a median needs no more).
pub const SPAN_STEP: i32 = 2;
/// An end column is a blend when the blend fractions of its pixels spread at most this much ...
pub const END_SPREAD: f32 = 0.2;
/// ... and the column after it holds less than this share of that fraction (a panel starting
/// exactly at the plate end fills both columns alike).
pub const END_NEXT_SHARE: f32 = 0.5;
/// A new sample within this many levels (every channel) of the previous capture's, at the same
/// x, keeps the previous value (see [`settle`]).
pub const SETTLE: i32 = 4;
/// An edge pixel is a plate-to-chat blend when it lies within this distance of the line between
/// the two colours ...
pub const BLEND_RESIDUAL: f32 = 12.0;
/// ... with a blend fraction inside (BLEND_MIN, 1 - BLEND_MIN) ...
pub const BLEND_MIN: f32 = 0.03;
/// ... and the two colours at least this far apart (closer, the blend is hardly visible).
pub const BLEND_CONTRAST: f32 = 20.0;
/// Text-like pixels: brighter than the median by more than this (luma)...
pub const BRIGHT: i32 = 28;
/// ... more saturated by more than this (max - min channel) ...
pub const SATURATED: i32 = 40;
/// ... or farther than this from the median on any channel.
pub const FAR: i32 = 48;

/// One side's sample: colour and the x it stands for (the middle of the sampled span).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    pub rgb: [u8; 3],
    pub x: i32,
}

/// The background of one row band.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BandBg {
    pub left: Option<Sample>,
    pub right: Option<Sample>,
}

impl BandBg {
    pub fn is_empty(&self) -> bool {
        self.left.is_none() && self.right.is_none()
    }

    /// The background colour at column `x`: linear between the two samples (flat outside them),
    /// the one sample when only one exists, None when there is none.
    pub fn at(&self, x: i32) -> Option<[u8; 3]> {
        match (self.left, self.right) {
            (Some(l), Some(r)) if r.x > l.x => {
                let t = ((x - l.x) as f32 / (r.x - l.x) as f32).clamp(0.0, 1.0);
                Some([0, 1, 2].map(|c| {
                    let v = f32::from(l.rgb[c]) + t * (f32::from(r.rgb[c]) - f32::from(l.rgb[c]));
                    (v + 0.5) as u8
                }))
            }
            (Some(l), _) => Some(l.rgb),
            (None, Some(r)) => Some(r.rgb),
            (None, None) => None,
        }
    }

    /// The colours of columns `x0..x0 + w` (empty when there is no sample).
    pub fn row(&self, x0: i32, w: i32) -> Vec<[u8; 3]> {
        if self.is_empty() || w <= 0 {
            return Vec::new();
        }
        (x0..x0 + w).map(|x| self.at(x).unwrap_or([0, 0, 0])).collect()
    }
}

/// Blended plate pixels `[x0, x1)` on row `y`, painted with the background of band `band`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EdgeRun {
    pub band: usize,
    pub y: i32,
    pub x0: i32,
    pub x1: i32,
}

/// The background of every band of a card (same order as `card.bands`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CardBg {
    pub bands: Vec<BandBg>,
    pub edges: Vec<EdgeRun>,
}

impl CardBg {
    /// Band `i`'s background, each side taken from the band itself, else from the nearest band
    /// that has that side (the chat background is the same a row above or below: a row whose
    /// plate is cut short by a tooltip has no right sample of its own, and a flat fill from its
    /// left sample alone would not match the gradient of its neighbours), else empty.
    pub fn band(&self, i: usize) -> BandBg {
        let n = self.bands.len();
        let side = |get: fn(&BandBg) -> Option<Sample>| -> Option<Sample> {
            if let Some(s) = self.bands.get(i).and_then(get) {
                return Some(s);
            }
            for d in 1..n {
                for j in [i.checked_sub(d), i.checked_add(d).filter(|j| *j < n)].into_iter().flatten() {
                    if let Some(s) = self.bands.get(j).and_then(get) {
                        return Some(s);
                    }
                }
            }
            None
        };
        BandBg { left: side(|b| b.left), right: side(|b| b.right) }
    }

    /// True when no band has a sample (the overlay then falls back to the plate colour).
    pub fn is_empty(&self) -> bool {
        self.bands.iter().all(BandBg::is_empty)
    }

    /// For the scene fingerprint: the samples (kept steady by [`settle`], so that the texture of a
    /// game world behind a translucent chat does not republish the scene at every capture, and
    /// quantized to 4 levels on top) and the edge runs.
    pub fn hash_words(&self) -> Vec<i64> {
        let mut out = Vec::with_capacity(self.bands.len() * 4 + self.edges.len() * 4);
        for b in &self.bands {
            for s in [b.left, b.right] {
                match s {
                    Some(s) => {
                        out.push(i64::from(s.x));
                        out.push(i64::from(s.rgb[0] >> 2) << 16 | i64::from(s.rgb[1] >> 2) << 8 | i64::from(s.rgb[2] >> 2));
                    }
                    None => out.push(-1),
                }
            }
        }
        for e in &self.edges {
            out.extend([e.band as i64, e.y.into(), e.x0.into(), e.x1.into()]);
        }
        out
    }

    /// Applies `f` (a display-curve correction) to every sample.
    pub fn map_colors(&mut self, f: impl Fn([u8; 3]) -> [u8; 3]) {
        for b in self.bands.iter_mut() {
            for s in [&mut b.left, &mut b.right].into_iter().flatten() {
                s.rgb = f(s.rgb);
            }
        }
    }
}

fn luma(p: [u8; 3]) -> i32 {
    (299 * i32::from(p[0]) + 587 * i32::from(p[1]) + 114 * i32::from(p[2])) / 1000
}

fn chroma(p: [u8; 3]) -> i32 {
    i32::from(p[0].max(p[1]).max(p[2])) - i32::from(p[0].min(p[1]).min(p[2]))
}

fn median3(px: &[[u8; 3]]) -> [u8; 3] {
    let mut ch: [Vec<u8>; 3] = [Vec::with_capacity(px.len()), Vec::with_capacity(px.len()), Vec::with_capacity(px.len())];
    for p in px {
        for c in 0..3 {
            ch[c].push(p[c]);
        }
    }
    [0, 1, 2].map(|c| {
        let v = &mut ch[c];
        let k = v.len() / 2;
        *v.select_nth_unstable(k).1
    })
}

/// Robust background colour of the pixels of `[x0, x1] x [y0, y1]` (inside the capture), None
/// when fewer than `MIN_PIXELS` are available.
pub fn robust_color(img: &Bgra, x0: i32, x1: i32, y0: i32, y1: i32) -> Option<[u8; 3]> {
    robust_color_where(img, x0, x1, y0, y1, 1, |_, _| true)
}

/// [`robust_color`] over every `step`-th column, of the pixels for which `keep(x, y)` holds.
pub fn robust_color_where(img: &Bgra, x0: i32, x1: i32, y0: i32, y1: i32, step: i32, keep: impl Fn(i32, i32) -> bool) -> Option<[u8; 3]> {
    if x1 < x0 || y1 < y0 {
        return None;
    }
    let step = step.max(1);
    let mut px: Vec<[u8; 3]> = Vec::with_capacity((((x1 - x0) / step + 1) * (y1 - y0 + 1)).max(0) as usize);
    for y in y0..=y1 {
        for x in (x0..=x1).step_by(step as usize) {
            if !keep(x, y) {
                continue;
            }
            if let Some(p) = img.rgb(x, y) {
                px.push(p);
            }
        }
    }
    if px.len() < MIN_PIXELS {
        return None;
    }
    let m = median3(&px);
    let (ml, mc) = (luma(m), chroma(m));
    let kept: Vec<[u8; 3]> = px
        .iter()
        .copied()
        .filter(|p| {
            let far = (0..3).any(|c| (i32::from(p[c]) - i32::from(m[c])).abs() > FAR);
            !(far || luma(*p) - ml > BRIGHT || chroma(*p) - mc > SATURATED)
        })
        .collect();
    if kept.len() >= MIN_PIXELS.max(px.len() / 4) {
        Some(median3(&kept))
    } else {
        Some(m)
    }
}

/// Where pixel `c` lies between `plate` (0) and `chat` (1), and its distance from the line
/// between them; None when the two colours are too close to tell.
fn blend_fraction(c: [u8; 3], plate: [u8; 3], chat: [u8; 3]) -> Option<(f32, f32)> {
    let (c, p, g) = (c.map(f32::from), plate.map(f32::from), chat.map(f32::from));
    let d = [g[0] - p[0], g[1] - p[1], g[2] - p[2]];
    let n2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
    if n2 < BLEND_CONTRAST * BLEND_CONTRAST {
        return None;
    }
    let f = ((c[0] - p[0]) * d[0] + (c[1] - p[1]) * d[1] + (c[2] - p[2]) * d[2]) / n2;
    let resid = (0..3).map(|k| (c[k] - (p[k] + f * d[k])).abs()).fold(0.0, f32::max);
    Some((f, resid))
}

/// True when pixel `c` is a blend of `plate` and `chat`, neither one nor the other.
pub fn is_blend(c: [u8; 3], plate: [u8; 3], chat: [u8; 3]) -> bool {
    blend_fraction(c, plate, chat).is_some_and(|(f, resid)| f > BLEND_MIN && f < 1.0 - BLEND_MIN && resid <= BLEND_RESIDUAL)
}

/// True when column `x` of rows `ys` is the blended end of a plate: its pixels hold one share of
/// plate (spread at most `END_SPREAD`) and the column after it far less (a panel or a tooltip
/// starting exactly there fills both columns alike).
fn end_column_blends(img: &Bgra, x: i32, ys: &[i32], plate: [u8; 3], bg: &BandBg) -> bool {
    let shares = |x: i32| -> Vec<f32> {
        ys.iter()
            .filter_map(|&y| match (img.rgb(x, y), bg.at(x)) {
                (Some(c), Some(g)) => blend_fraction(c, plate, g).map(|(f, _)| 1.0 - f),
                _ => None,
            })
            .collect()
    };
    let here = shares(x);
    if here.len() < 3 {
        return false;
    }
    let mean = here.iter().sum::<f32>() / here.len() as f32;
    if mean <= BLEND_MIN || here.iter().any(|f| (f - mean).abs() > END_SPREAD) {
        return false;
    }
    let next = shares(x + 1);
    let next_mean = if next.is_empty() { 0.0 } else { next.iter().sum::<f32>() / next.len() as f32 };
    next_mean < END_NEXT_SHARE * mean
}

/// Appends to `out` the runs of pixels `(x, y)`, x in `xs`, that are plate-to-chat blends.
fn blend_runs(img: &Bgra, band: usize, y: i32, xs: impl Iterator<Item = i32>, plate: [u8; 3], bg: &BandBg, out: &mut Vec<EdgeRun>) {
    let mut run: Option<(i32, i32)> = None;
    for x in xs {
        let ok = matches!((img.rgb(x, y), bg.at(x)), (Some(c), Some(g)) if is_blend(c, plate, g));
        run = match (run, ok) {
            (Some((a, b)), true) if b == x => Some((a, x + 1)),
            (prev, true) => {
                if let Some((a, b)) = prev {
                    out.push(EdgeRun { band, y, x0: a, x1: b });
                }
                Some((x, x + 1))
            }
            (prev, false) => {
                if let Some((a, b)) = prev {
                    out.push(EdgeRun { band, y, x0: a, x1: b });
                }
                None
            }
        };
    }
    if let Some((a, b)) = run {
        out.push(EdgeRun { band, y, x0: a, x1: b });
    }
}

/// Samples the background of every band of `card`. `view` is the card's chat view (client px);
/// `discovery`: no VIEW is known and `view` is the whole client width.
pub fn sample_card(img: &Bgra, card: &Card, view: Rect, discovery: bool) -> CardBg {
    let img_x0 = img.origin_x;
    let img_x1 = img.origin_x + img.width as i32; // exclusive
    let view_left = view.x.max(img_x0);
    let view_right = (view.x + view.w).min(img_x1); // exclusive
    // the card's plate end: the right end of the widest row (all rows of a card have the same
    // plate width)
    let plate_end = card.bands.iter().map(|b| b.text_rect.x + b.text_rect.w).max().unwrap_or(0);
    // where the addon draws it: PLATE_FRACTION of the room between the tag and the view's edge
    let tag_end = card.bands.iter().map(|b| b.tag_rect.x + b.tag_rect.w).min().unwrap_or(0);
    let expected_end = tag_end + (PLATE_FRACTION * (view.x + view.w - tag_end).max(0) as f32) as i32;
    let right_known = card.plate && !discovery && plate_end >= expected_end - CUT_SLACK;
    let mut edges = Vec::new();
    let bands = card
        .bands
        .iter()
        .enumerate()
        .map(|(i, b)| {
            // the tag's own rows (the band rows, without the blended edge rows)
            let (y0, y1) = (b.text_rect.y, b.text_rect.y + b.text_rect.h - 1);
            let left = {
                let x1 = b.tag_rect.x - 1 - EDGE_SKIP;
                let reach = if discovery { ((DISCOVERY_LEFT * b.text_rect.h as f32).round() as i32).max(4) } else { LEFT_MAX };
                let x0 = view_left.max(b.tag_rect.x - reach);
                robust_color_where(img, x0, x1, y0, y1, SPAN_STEP, |_, _| true).map(|rgb| Sample { rgb, x: (x0 + x1) / 2 })
            };
            let reaches_end = b.text_rect.x + b.text_rect.w >= plate_end - END_SLACK;
            let right = if right_known && reaches_end {
                let x1 = view_right - 1 - 1;
                let x0 = (plate_end + EDGE_SKIP).max(x1 + 1 - RIGHT_FAR);
                let plate_px = |x: i32, y: i32| b.mask.as_ref().is_some_and(|m| m.get(x, y));
                robust_color_where(img, x0, x1, y0, y1, SPAN_STEP, |x, y| !plate_px(x, y)).map(|rgb| Sample { rgb, x: (x0 + x1) / 2 })
            } else {
                None
            };
            let bg = BandBg { left, right };
            if let (true, Some(m)) = (card.plate, &b.mask) {
                // the rows just above and below the plate, where the tag has a partial row (the
                // plate shares its rows) and the plate row next to it is visible ...
                let x_end = b.text_rect.x + b.text_rect.w;
                let (top, bottom) = (m.rect.y, m.rect.y + m.rect.h);
                let partial_above = b.tag_rect.y < top;
                let partial_below = b.tag_rect.y + b.tag_rect.h > bottom;
                for (y, src, partial) in [(top - 1, top, partial_above), (bottom, bottom - 1, partial_below)] {
                    if partial {
                        let xs = (m.rect.x..m.rect.x + m.rect.w).filter(|&x| m.get(x, src) || (x == x_end && reaches_end));
                        blend_runs(img, i, y, xs, card.plate_rgb, &bg, &mut edges);
                    }
                }
                // ... and the column after the plate's end, when it holds a blend of its own
                if reaches_end && x_end < view_right {
                    let ys: Vec<i32> = (top..bottom).filter(|&y| !m.get(x_end, y)).collect();
                    if end_column_blends(img, x_end, &ys, card.plate_rgb, &bg) {
                        for &y in &ys {
                            blend_runs(img, i, y, std::iter::once(x_end), card.plate_rgb, &bg, &mut edges);
                        }
                    }
                }
            }
            bg
        })
        .collect();
    CardBg { bands, edges }
}

/// Keeps `prev`'s samples where `new`'s moved by at most `SETTLE` levels (same x): `new` and
/// `prev` belong to the same card. The fill then follows a slowly drifting ground in steps of a
/// few levels instead of republishing the scene at every capture.
pub fn settle(new: &mut CardBg, prev: &CardBg) {
    if new.bands.len() != prev.bands.len() {
        return;
    }
    let close = |a: &Sample, b: &Sample| a.x == b.x && (0..3).all(|c| (i32::from(a.rgb[c]) - i32::from(b.rgb[c])).abs() <= SETTLE);
    for (n, p) in new.bands.iter_mut().zip(&prev.bands) {
        for (ns, ps) in [(&mut n.left, &p.left), (&mut n.right, &p.right)] {
            if let (Some(a), Some(b)) = (ns.as_ref(), ps.as_ref()) {
                if close(a, b) {
                    *ns = Some(*b);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tp_bridge::cards::{Mask, RowBand};

    fn img_of(w: u32, h: u32, f: impl Fn(i32, i32) -> [u8; 3]) -> Vec<u8> {
        let mut px = vec![0u8; (w * h * 4) as usize];
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let c = f(x, y);
                let i = ((y as u32 * w + x as u32) * 4) as usize;
                px[i..i + 4].copy_from_slice(&[c[2], c[1], c[0], 255]);
            }
        }
        px
    }

    #[test]
    fn gradient_between_two_samples() {
        let b = BandBg { left: Some(Sample { rgb: [0, 100, 200], x: 10 }), right: Some(Sample { rgb: [100, 100, 0], x: 110 }) };
        assert_eq!(b.at(10), Some([0, 100, 200]));
        assert_eq!(b.at(60), Some([50, 100, 100]));
        assert_eq!(b.at(110), Some([100, 100, 0]));
        assert_eq!(b.at(0), Some([0, 100, 200]), "flat left of the left sample");
        assert_eq!(b.at(500), Some([100, 100, 0]), "flat right of the right sample");
        let one = BandBg { left: None, right: Some(Sample { rgb: [7, 8, 9], x: 3 }) };
        assert_eq!(one.at(-50), Some([7, 8, 9]));
        assert_eq!(BandBg::default().at(0), None);
        assert_eq!(b.row(59, 3), vec![[49, 100, 102], [50, 100, 100], [51, 100, 98]]);
    }

    #[test]
    fn text_like_pixels_are_ignored() {
        // A dark green ground with 30% of its pixels bright white or saturated yellow (glyphs).
        let px = img_of(40, 10, |x, y| if (x * 7 + y * 3) % 10 < 3 { if x % 2 == 0 { [255, 255, 255] } else { [255, 210, 0] } } else { [20, 60, 30] });
        let img = Bgra { px: &px, width: 40, height: 10, stride: 160, origin_x: 0, origin_y: 0 };
        assert_eq!(robust_color(&img, 0, 39, 0, 9), Some([20, 60, 30]));
        assert_eq!(robust_color(&img, 0, 1, 0, 1), None, "too few pixels");
        assert_eq!(robust_color(&img, 5, 4, 0, 9), None, "empty span");
    }

    #[test]
    fn blends_are_told_from_other_colours() {
        let (p, g) = ([32, 40, 64], [120, 100, 40]);
        assert!(is_blend([76, 70, 52], p, g), "half way");
        assert!(!is_blend(p, p, g), "the plate itself");
        assert!(!is_blend(g, p, g), "the chat itself");
        assert!(!is_blend([16, 16, 24], p, g), "a tooltip");
        assert!(!is_blend([255, 255, 255], p, g), "a glyph");
        assert!(!is_blend([36, 44, 60], p, [40, 45, 60]), "too little contrast to tell");
    }

    #[test]
    fn card_bands_get_left_and_right_samples() {
        // ground: red rises from 0 to 200 left to right; one row with a tag at 20..67 and a plate
        // ending at 360 in a view 0..400 (0.88 of the room right of the tag, as the addon sizes it).
        let px = img_of(400, 40, |x, _| [(x / 2) as u8, 40, 90]);
        let img = Bgra { px: &px, width: 400, height: 40, stride: 1600, origin_x: 0, origin_y: 0 };
        // the mask spans x 68 to the view's right edge, set on the plate pixels only (as built by
        // `cards::build`: the chat after the plate does not look like the plate)
        let band = |y: i32, right: i32| RowBand {
            row: 0,
            text_rect: Rect { x: 70, y, w: right - 70, h: 12 },
            mask: Some(Mask { rect: Rect { x: 68, y, w: 332, h: 12 }, bits: (0..332 * 12).map(|i| if 68 + i % 332 < right { 255 } else { 0 }).collect() }),
            tag_rect: Rect { x: 19, y, w: 49, h: 12 },
        };
        let card = Card { slot: 1, view_id: 0, bands: vec![band(5, 360), band(20, 200)], gaps: vec![], alpha: 1.0, plate: true, plate_rgb: [32, 40, 64], text_width_px: 290 };
        let bg = sample_card(&img, &card, Rect { x: 0, y: 0, w: 400, h: 40 }, false);
        let b0 = bg.bands[0];
        let l = b0.left.unwrap();
        assert_eq!(l.x, 8, "middle of 0..=16");
        assert_eq!(l.rgb, [4, 40, 90]);
        let r = b0.right.unwrap();
        assert_eq!(r.x, (362 + 398) / 2, "the last 40 px of the view, right of the plate end");
        assert!((i32::from(r.rgb[0]) - r.x / 2).abs() <= 1, "{r:?}");
        // the gradient at a plate column matches the ground there within 1 level
        let at = b0.at(250).unwrap();
        assert!((i32::from(at[0]) - 125).abs() <= 1, "{at:?}");
        // the second row's plate ends early (a cover): no right sample of its own
        assert!(bg.bands[1].right.is_none());
        assert!(bg.bands[1].left.is_some());
        // ... so it borrows the right sample of the row above, and keeps its own left one
        assert_eq!(bg.band(1).right, bg.bands[0].right);
        assert_eq!(bg.band(1).left, bg.bands[1].left);
        // the ground is not a blend of the plate and itself: no edge pixels
        assert!(bg.edges.is_empty(), "{:?}", bg.edges);
        // fallback to a neighbour when a band has nothing
        let mut bg2 = bg.clone();
        bg2.bands[1] = BandBg::default();
        assert_eq!(bg2.band(1), bg2.bands[0]);
        assert!(!bg2.is_empty());
        assert!(CardBg::default().is_empty());
    }

    #[test]
    fn blended_rows_and_end_column_become_edge_runs() {
        // A flat ground; a plate on rows 10..=19, x 50..=189, drawn at fractional edges: row 9 and
        // row 20 hold 40% of it (the tag has partial rows there too), column 190 holds 50% of it
        // (rows 10..=19). A tooltip covers x >= 120 of row 20.
        let (g, p) = ([120u8, 100, 40], [32u8, 40, 64]);
        let mix = |f: f32| [0, 1, 2].map(|c| (f32::from(g[c]) + f * (f32::from(p[c]) - f32::from(g[c])) + 0.5) as u8);
        let px = img_of(220, 30, |x, y| {
            let inside_x = (50..190).contains(&x);
            match (y, x) {
                (20, 120..=200) => [16, 16, 24],
                (10..=19, 190) => mix(0.5),
                (9 | 20, _) if inside_x => mix(0.4),
                (10..=19, _) if inside_x => p,
                _ => g,
            }
        });
        let img = Bgra { px: &px, width: 220, height: 30, stride: 880, origin_x: 0, origin_y: 0 };
        let band = |partial: bool| RowBand {
            row: 0,
            text_rect: Rect { x: 50, y: 10, w: 140, h: 10 },
            mask: Some(Mask { rect: Rect { x: 48, y: 10, w: 172, h: 10 }, bits: (0..1720).map(|i| if (2..142).contains(&(i % 172)) { 255 } else { 0 }).collect() }),
            tag_rect: if partial { Rect { x: 0, y: 9, w: 48, h: 12 } } else { Rect { x: 0, y: 10, w: 48, h: 10 } },
        };
        let card = |partial: bool| Card { slot: 1, view_id: 0, bands: vec![band(partial)], gaps: vec![], alpha: 1.0, plate: true, plate_rgb: p, text_width_px: 140 };
        let bg = sample_card(&img, &card(true), Rect { x: 0, y: 0, w: 220, h: 30 }, false);
        assert!(bg.bands[0].left.is_none(), "the tag starts at the view edge");
        assert_eq!(bg.bands[0].right.map(|s| s.rgb), Some(g));
        let mut e = bg.edges.clone();
        e.sort_by_key(|r| (r.y, r.x0));
        assert_eq!(e[0], EdgeRun { band: 0, y: 9, x0: 50, x1: 190 }, "{e:?}");
        assert_eq!(e[1], EdgeRun { band: 0, y: 10, x0: 190, x1: 191 }, "{e:?}");
        assert_eq!(e.iter().filter(|r| r.x0 == 190 && r.x1 == 191 && (10..20).contains(&r.y)).count(), 10, "the end column");
        let r20: Vec<&EdgeRun> = e.iter().filter(|r| r.y == 20).collect();
        assert_eq!(r20, vec![&EdgeRun { band: 0, y: 20, x0: 50, x1: 120 }], "never on the tooltip");
        // without partial tag rows, the rows above and below are not plate edges: only the end
        // column (a blend of its own) is painted
        let bg = sample_card(&img, &card(false), Rect { x: 0, y: 0, w: 220, h: 30 }, false);
        assert!(bg.edges.iter().all(|r| r.x0 == 190 && r.x1 == 191 && (10..20).contains(&r.y)), "{:?}", bg.edges);
        assert_eq!(bg.edges.len(), 10);
    }

    /// A one-row card over a dark chat with plate rows at whole pixels.
    fn dark_card(img_w: u32, plate_end: i32, f: impl Fn(i32, i32) -> [u8; 3]) -> (Vec<u8>, Card) {
        let px = img_of(img_w, 40, f);
        let band = RowBand {
            row: 0,
            text_rect: Rect { x: 70, y: 10, w: plate_end - 70, h: 12 },
            mask: Some(Mask { rect: Rect { x: 68, y: 10, w: img_w as i32 - 68, h: 12 }, bits: (0..(img_w as i32 - 68) * 12).map(|i| if 68 + i % (img_w as i32 - 68) < plate_end { 255 } else { 0 }).collect() }),
            tag_rect: Rect { x: 19, y: 10, w: 49, h: 12 },
        };
        (px, Card { slot: 1, view_id: 1, bands: vec![band], gaps: vec![], alpha: 1.0, plate: true, plate_rgb: [32, 40, 64], text_width_px: plate_end - 70 })
    }

    #[test]
    fn a_frame_over_the_right_of_the_chat_is_never_sampled() {
        // A frame from x 300 to the view's right edge covers every row of the card (a one-row card
        // here): the plate seems to end at 300, far before the 360 the addon draws, so the far end
        // of the view (under the frame) is not taken for the chat.
        let chat = [18u8, 18, 20];
        let (px, card) = dark_card(400, 300, |x, y| if x >= 300 { [200, 180, 140] } else if (10..22).contains(&y) && (68..300).contains(&x) { [32, 40, 64] } else { chat });
        let img = Bgra { px: &px, width: 400, height: 40, stride: 1600, origin_x: 0, origin_y: 0 };
        let bg = sample_card(&img, &card, Rect { x: 0, y: 0, w: 400, h: 40 }, false);
        assert!(bg.bands[0].right.is_none(), "{bg:?}");
        assert_eq!(bg.band(0).at(250), Some(chat), "a flat fill from the left sample");
        assert!(bg.edges.is_empty(), "the frame's edge column is not a plate blend: {:?}", bg.edges);
        // a whole plate (ending at 360) keeps its right sample
        let (px, card) = dark_card(400, 360, |x, y| if (10..22).contains(&y) && (68..360).contains(&x) { [32, 40, 64] } else { chat });
        let img = Bgra { px: &px, width: 400, height: 40, stride: 1600, origin_x: 0, origin_y: 0 };
        assert_eq!(sample_card(&img, &card, Rect { x: 0, y: 0, w: 400, h: 40 }, false).bands[0].right.map(|s| s.rgb), Some(chat));
    }

    #[test]
    fn discovery_samples_only_the_indent() {
        // No VIEW: the view is the whole client. The game world shows left of x 8 and right of the
        // chat (x >= 400): the left span stays within one em of the tag, and no right sample.
        let chat = [18u8, 18, 20];
        let (px, card) = dark_card(600, 360, |x, y| {
            if !(8..400).contains(&x) {
                [140, 165, 200]
            } else if (10..22).contains(&y) && (68..360).contains(&x) {
                [32, 40, 64]
            } else {
                chat
            }
        });
        let img = Bgra { px: &px, width: 600, height: 40, stride: 2400, origin_x: 0, origin_y: 0 };
        let bg = sample_card(&img, &card, Rect { x: 0, y: 0, w: 600, h: 40 }, true);
        let b = bg.bands[0];
        assert!(b.right.is_none());
        assert_eq!(b.left.map(|s| s.rgb), Some(chat), "{b:?}");
        assert_eq!(b.left.unwrap().x, (19 - 9 + 16) / 2, "x 10..=16: 9 px (0.75 em) left of the tag");
    }

    #[test]
    fn a_panel_next_to_a_plate_is_not_a_plate_edge() {
        // WoW's default tooltip background (23, 23, 48) passes for a blend of the plate and a dark
        // chat. A borderless panel starts on the row right under the plate (whole-pixel edges, no
        // partial tag row) and another exactly at the plate end: neither is painted.
        let (chat, panel) = ([12u8, 12, 14], [23u8, 23, 48]);
        assert!(is_blend(panel, [32, 40, 64], chat));
        let (px, card) = dark_card(400, 360, |x, y| {
            if (y >= 22 && (100..380).contains(&x)) || ((10..22).contains(&y) && x >= 360 && x < 380) {
                panel
            } else if (10..22).contains(&y) && (68..360).contains(&x) {
                [32, 40, 64]
            } else {
                chat
            }
        });
        let img = Bgra { px: &px, width: 400, height: 40, stride: 1600, origin_x: 0, origin_y: 0 };
        let bg = sample_card(&img, &card, Rect { x: 0, y: 0, w: 400, h: 40 }, false);
        assert!(bg.edges.is_empty(), "{:?}", bg.edges);
    }

    #[test]
    fn small_sample_moves_settle_on_the_previous_value() {
        let s = |r: u8, x: i32| Some(Sample { rgb: [r, 20, 30], x });
        let prev = CardBg { bands: vec![BandBg { left: s(100, 5), right: s(50, 300) }], edges: Vec::new() };
        let mut new = CardBg { bands: vec![BandBg { left: s(103, 5), right: s(56, 300) }], edges: Vec::new() };
        settle(&mut new, &prev);
        assert_eq!(new.bands[0].left, prev.bands[0].left, "3 levels: the previous value stays");
        assert_eq!(new.bands[0].right, s(56, 300), "6 levels: the new value");
        let mut moved = CardBg { bands: vec![BandBg { left: s(101, 6), right: None }], edges: Vec::new() };
        settle(&mut moved, &prev);
        assert_eq!(moved.bands[0].left, s(101, 6), "another place: the new value");
        assert_eq!(moved.hash_words(), CardBg { bands: vec![BandBg { left: s(101, 6), right: None }], edges: Vec::new() }.hash_words());
    }
}
