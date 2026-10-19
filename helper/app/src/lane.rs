//! Hint lanes (design A.6.4 for the reply card; under every card since 2026-10-19, user request
//! U1; `docs\BRIDGE.md` section 8c).
//!
//! The addon can never learn whether the Helper (Translate Companion) runs. So under the last
//! reserved row of every card that waits for it, it draws one more row, the lane, with a gold hint
//! shown 1.5 s after the rows: "No translation above? Start Translate Companion." and a
//! [Download] link or button under a bubble or an inline chat line, "No status above? Start
//! Translate Companion." under the reply card. The Helper covers the lane of every card it
//! paints, whatever the card's state, so the hint shows exactly when no Helper paints it (no
//! timer, no channel back into the game). An addon older than build 4 (`scene::LANES_BUILD`)
//! draws a lane under the reply card only, and the `/tpr test rows` card never has one: the
//! capture searches lanes only where the addon draws them (`SceneMachine::lanes_for`).
//!
//! Two kinds, both one row of the card's row pitch right under the card's last row, from the
//! tag's columns to the plate's end:
//!
//! - **Window lanes** (VIEW ids 200..255: bubbles and the reply card), [`find_window_lane`]: the
//!   opaque bubble fill at `row0.yTop + K * rowPx` (+-2 px), the hint and the button from the
//!   plate's x, clear of its top and bottom (the addon's margin is 2 units and 2 px at least, its
//!   font a point under the text box). Only under the request's last row (the addon draws no lane
//!   under a bubble whose last row is hidden). The fill colour is the median of the lane's tag
//!   column over the rows every candidate position shares (it must be uniform, else something
//!   covers it) and it must be the card's own fill (its background sample): a tooltip's or a
//!   menu's body is not. A column passes when its top 2 and bottom 2 rows are that colour (or the
//!   hint's own glyph edge, should it reach them at a small scale); a tooltip or a menu over the
//!   lane breaks those edges, so it is never painted over. The overlay paints the passing columns
//!   flat with that colour.
//! - **Chat lanes** (inline rows under a chat line), [`find_chat_lane`]: one chat line right under
//!   the card's last tag row (the row above it must be the request's last row, K - 1; a last row
//!   cut by the top of the chat is measured from its bottom), the hint in gold (NORMAL_FONT_COLOR)
//!   from the tag's x0, then a [Download] link in the light blue of the addon's links (66bbff),
//!   over the translucent chat background; a single space until the hint shows. The chat font's
//!   glyphs and shadows reach the line's top and bottom rows, and the ground shows the game world
//!   through the chat, so:
//!   - the ground is first the chat background of the card's last row (`backdrop`: left of the
//!     tag and, when the card has one, at the far end of the view), the gradient the overlay paints
//!     the rows above with and the lane with;
//!   - a pixel is *ground* within the ground tolerance, *lane ink* when it is the ground blended
//!     toward the hint's gold (`is_gold`) or the link's light blue (`is_link_blue`), or when it
//!     lies within `NEAR_INK` px of such a pixel and is a mix of the ground, black and a lane
//!     colour (`is_glyph_mix`: antialiased edges and the 1 px text shadow), else *other ink*
//!     (brighter) or anything else;
//!   - when a column fails against that gradient (a textured world behind the chat, a darker or
//!     lighter feature neither sample sees), the lane is judged again against a **local ground**:
//!     per column, the median of the lane's own plain pixels (not lane ink, not next to it, not
//!     much brighter than the gradient: text never becomes ground) over `LOCAL_R` columns on each
//!     side, with a tolerance from their texture (`CHAT_TOL` to `TOL_MAX`);
//!   - the lane is only taken for one when its tag column holds no other ink (`TAG_INK_MAX`):
//!     the next chat line, the glossary row (pale gold) and the romanized name row (9fd4ff) all
//!     start with glyphs of another colour at the left, where the lane is blank, gold or blue;
//!   - a column passes when its top 2 and bottom 2 rows are ground or lane ink, it lies under no
//!     **cover edge** (a straight horizontal edge across `RUN_MIN` columns or more inside the
//!     lane: a tooltip's border, a panel's top; texture and glyphs make none) and, under the
//!     plate, the card's last row shows its plate there (mask): a game tooltip over the lane
//!     almost always covers the row above it too.
//! - **Orphan chat lanes** ([`find_orphan_chat_lane`]): once a card's rows have scrolled above the
//!   top of the chat, its lane can still be the chat's top line, with no card to find it under.
//!   The capture remembers each chat lane it found ([`ChatLaneMemo`]) and, while the card is gone,
//!   looks for the lane a whole number of line pitches higher, in the top line of the view (the
//!   row above it cut by the top). There it is judged against the chat background sampled on the
//!   lane's own line (the indent and the room right of the plate's end), with the gradient only,
//!   the cover edges, and the edge rows that are visible; a lane cut by the top keeps its visible
//!   rows.
//!
//! With fewer than 60% of the columns passing a lane is left alone: the hint stays visible there.
//! A lane that does not exist (an older addon) is never searched; where it is searched and the
//! addon drew none, under a window card there is only fill (painted with fill), under a chat card
//! the next line or nothing.

use tp_bridge::cards::Card;
use tp_bridge::image::{Bgra, Rect};

use crate::backdrop::{self, BandBg, CardBg, Sample};

/// Rows at the lane's top and at its bottom that must be plain fill (window) or ground or lane
/// ink (chat).
pub const EDGE: i32 = 2;
/// How far the lane may sit from where the geometry puts it.
pub const SLACK: i32 = 2;
/// Window lanes: largest per-channel distance of an edge pixel from the fill colour.
pub const TOL: i32 = 8;
/// Window lanes: the tag-column sample must be this uniform (its pixels within this of their
/// median, a quarter of them excepted): else something covers it.
pub const SPREAD_MAX: i32 = 6;
/// Window lanes: the fill must be within this many levels (every channel) of the card's own
/// background sample (the bubble fill next to its last row).
pub const FILL_TOL: i32 = 8;
/// A chat lane found under rows that did not move is searched again at most this often (ms); in
/// between it keeps its passing columns and takes each capture's fill (`app.rs`).
pub const RESEARCH_MS: u64 = 100;
/// Smallest share of passing columns for the lane to be painted.
pub const MIN_SHARE: f32 = 0.6;
/// Chat lanes: a pixel within this many levels (every channel) of the ground gradient is ground;
/// the local model's tolerance starts here.
pub const CHAT_TOL: i32 = 24;
/// Chat lanes, local model: the largest ground tolerance (texture behind the chat).
pub const TOL_MAX: i32 = 48;
/// Chat lanes, local model: columns on each side whose plain pixels give a column's ground.
pub const LOCAL_R: usize = 6;
/// Chat lanes, local model: one estimate every this many columns.
pub const LOCAL_STEP: usize = 5;
/// Chat lanes, local model: most plain pixels one estimate uses (evenly picked from its window).
pub const LOCAL_POOL: usize = 96;
/// Chat lanes, local model: fewest plain pixels around a column for a local ground.
pub const LOCAL_MIN: usize = 8;
/// Chat lanes, local model: a pixel this much brighter (luma) than the gradient is never ground
/// (text of another line, a tooltip's text or border).
pub const LOCAL_BRIGHT: i32 = 40;
/// Chat lanes, local model: the local ground stays within this many levels (every channel) of the
/// gradient.
pub const LOCAL_MAX: i32 = 64;
/// Chat lanes: a gold ink pixel rises at least this much on red from the ground.
pub const GOLD_MIN: i32 = 30;
/// Chat lanes: the green of a gold text colour, as estimated from a blend (NORMAL_FONT_COLOR's is
/// 209; yellow's 255 and orange's 128 are out), inclusive.
pub const GOLD_G: (i32, i32) = (165, 235);
/// Chat lanes: the largest blue of a gold text colour, as estimated from a blend (the glossary
/// row's pale gold has 128).
pub const GOLD_B_MAX: i32 = 95;
/// Chat lanes: the red of the links' light blue (66bbff: 102), as estimated from a blend taking
/// its blue as full; the name row's 9fd4ff (159) and the party's (170) are out. Inclusive.
pub const LINK_R: (i32, i32) = (65, 135);
/// Chat lanes: the green of the links' light blue (187), estimated the same way. Inclusive.
pub const LINK_G: (i32, i32) = (155, 220);
/// Chat lanes: pixels this close (px, any direction) to lane ink belong to its glyph (edges and
/// the 1 px text shadow).
pub const NEAR_INK: i32 = 2;
/// Chat lanes: a pixel next to lane ink joins it only when it is a mix of the ground, the text
/// shadow and the lane's colours within this many levels (`is_glyph_mix`): never a bright border
/// or a tooltip's body.
pub const FRINGE_RESID: f32 = 12.0;
/// The inline lane's colours (the addon's `ChatButton`): the hint in NORMAL_FONT_COLOR's gold
/// (ffd100) and the [Download] link in the light blue of the addon's links (66bbff).
pub const LANE_GOLD: [u8; 3] = [255, 209, 0];
pub const LANE_LINK: [u8; 3] = [102, 187, 255];
/// Chat lanes: the tag column may hold at most this share of other ink (text of another line).
pub const TAG_INK_MAX: f32 = 0.03;
/// Chat lanes: other ink is at least this much brighter (luma) than the ground.
pub const INK_LUMA: i32 = 16;
/// Cover edges: two vertically adjacent pixels at least this far apart...
pub const EDGE_JUMP: i32 = 16;
/// ...along a run in which each pixel is within this of its left neighbour on one of the two rows
/// (a cover's border or body is flat, a textured world is not)...
pub const RUN_UNI: i32 = 8;
/// ...of at least this many columns mark a cover's edge inside the lane.
pub const RUN_MIN: usize = 12;
/// Columns failed on each side of a cover edge (rounded corners, blended ends).
pub const RUN_PAD: usize = 3;
/// Orphan chat lanes: fewest visible rows of a lane cut by the top of the chat.
pub const ORPHAN_MIN_ROWS: i32 = 3;
/// Orphan chat lanes: the lane lies less than this share of a line under the top of the view (the
/// row above it mostly cut: with more of it showing, the tag scan finds the card).
pub const ORPHAN_ROW_ABOVE: f32 = 0.6;
/// Orphan chat lanes: fewest lane ink pixels in the visible part (a blank line needs no paint).
pub const ORPHAN_MIN_INK: usize = 3;
/// Orphan chat lanes: share of the remembered ink pixels of the visible rows that must be ink again.
pub const ORPHAN_INK_MATCH: f32 = 0.6;
/// Orphan chat lanes: at most this share of new ink pixels (plus `ORPHAN_MIN_INK`) over them.
pub const ORPHAN_INK_EXTRA: f32 = 0.5;
/// Orphan chat lanes: most line pitches the chat may have moved up since the lane was seen.
pub const ORPHAN_MAX_LINES: i32 = 8;
/// Orphan chat lanes: a memo not found again for this long is dropped.
pub const ORPHAN_KEEP_MS: u64 = 1500;

/// Where a lane was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaneKind {
    /// Under a window card: flat bubble fill.
    Window,
    /// Under an inline chat card: the chat background (a gradient).
    Chat,
}

impl LaneKind {
    pub fn name(self) -> &'static str {
        match self {
            LaneKind::Window => "window",
            LaneKind::Chat => "chat",
        }
    }
}

/// A lane found in a capture: client rectangle, its fill and, per column from `rect.x`, whether
/// that column may be painted.
#[derive(Clone, Debug)]
pub struct Lane {
    pub rect: Rect,
    pub kind: LaneKind,
    /// The fill: a window lane's bubble fill (`left` only, flat) or a chat lane's background
    /// samples (a gradient between them, flat with one).
    pub fill: BandBg,
    pub cols: Vec<bool>,
}

fn rect4(r: &Rect) -> [i32; 4] {
    [r.x, r.y, r.w, r.h]
}

impl PartialEq for Lane {
    fn eq(&self, o: &Lane) -> bool {
        rect4(&self.rect) == rect4(&o.rect) && self.kind == o.kind && self.fill == o.fill && self.cols == o.cols
    }
}

impl Eq for Lane {}

impl Lane {
    /// A window lane: flat `rgb` over `rect`.
    pub fn flat(rect: Rect, rgb: [u8; 3], cols: Vec<bool>) -> Lane {
        Lane { rect, kind: LaneKind::Window, fill: BandBg { left: Some(Sample { rgb, x: rect.x }), right: None }, cols }
    }

    /// Columns that may be painted.
    pub fn passing(&self) -> usize {
        self.cols.iter().filter(|c| **c).count()
    }

    /// The fill colour at the lane's left (a window lane's only colour).
    pub fn rgb(&self) -> [u8; 3] {
        self.fill.at(self.rect.x).unwrap_or([0, 0, 0])
    }

    /// A chat lane's colour at its right end, when it has a right sample (a gradient).
    pub fn rgb_right(&self) -> Option<[u8; 3]> {
        self.fill.right.map(|_| self.fill.at(self.rect.x + self.rect.w - 1).unwrap_or([0, 0, 0]))
    }

    /// The fill colour of every column of the lane.
    pub fn colors(&self) -> Vec<[u8; 3]> {
        (self.rect.x..self.rect.x + self.rect.w).map(|x| self.fill.at(x).unwrap_or([0, 0, 0])).collect()
    }

    /// Applies `f` (a display-curve correction) to the fill.
    pub fn map_colors(&mut self, f: impl Fn([u8; 3]) -> [u8; 3]) {
        for s in [&mut self.fill.left, &mut self.fill.right].into_iter().flatten() {
            s.rgb = f(s.rgb);
        }
    }

    /// For the scene fingerprint: rectangle, kind, the fill quantized to 4 levels (kept steady by
    /// [`settle`]) and the runs of passing columns.
    pub fn hash_words(&self) -> Vec<i64> {
        let mut runs = 0i64;
        let mut prev = false;
        for (i, &c) in self.cols.iter().enumerate() {
            if c != prev {
                runs = runs.wrapping_mul(31).wrapping_add(i as i64 + 1);
                prev = c;
            }
        }
        let mut out = vec![
            i64::from(self.rect.x),
            i64::from(self.rect.y),
            i64::from(self.rect.w),
            i64::from(self.rect.h),
            self.kind as i64,
            runs,
        ];
        for s in [self.fill.left, self.fill.right] {
            match s {
                Some(s) => {
                    out.push(i64::from(s.x));
                    out.push(i64::from(s.rgb[0] >> 2) << 16 | i64::from(s.rgb[1] >> 2) << 8 | i64::from(s.rgb[2] >> 2));
                }
                None => out.push(-1),
            }
        }
        out
    }
}

/// Keeps `prev`'s fill samples where `new`'s moved by at most `backdrop::SETTLE` levels (same
/// rectangle and sample x), like the cards' background: a drifting ground behind a translucent
/// chat does not republish the scene at every capture.
pub fn settle(new: &mut Lane, prev: &Lane) {
    if rect4(&new.rect) != rect4(&prev.rect) || new.kind != prev.kind {
        return;
    }
    let close = |a: &Sample, b: &Sample| a.x == b.x && (0..3).all(|c| (i32::from(a.rgb[c]) - i32::from(b.rgb[c])).abs() <= backdrop::SETTLE);
    for (ns, ps) in [(&mut new.fill.left, &prev.fill.left), (&mut new.fill.right, &prev.fill.right)] {
        if let (Some(a), Some(b)) = (ns.as_ref(), ps.as_ref()) {
            if close(a, b) {
                *ns = Some(*b);
            }
        }
    }
}

fn dist(a: [u8; 3], b: [u8; 3]) -> i32 {
    let d = |c: usize| (i32::from(a[c]) - i32::from(b[c])).abs();
    d(0).max(d(1)).max(d(2))
}

fn luma(p: [u8; 3]) -> i32 {
    (299 * i32::from(p[0]) + 587 * i32::from(p[1]) + 114 * i32::from(p[2])) / 1000
}

/// Per-channel median of `px` and the distance of its 75th-percentile pixel from it.
fn median_spread_of(px: &[[u8; 3]]) -> Option<([u8; 3], i32)> {
    if px.is_empty() {
        return None;
    }
    let mut m = [0u8; 3];
    let mut ch: Vec<u8> = Vec::with_capacity(px.len());
    for (c, mc) in m.iter_mut().enumerate() {
        ch.clear();
        ch.extend(px.iter().map(|p| p[c]));
        let k = ch.len() / 2;
        *mc = *ch.select_nth_unstable(k).1;
    }
    let mut d: Vec<i32> = px.iter().map(|p| dist(*p, m)).collect();
    let k = (d.len() * 3 / 4).min(d.len() - 1);
    let spread = *d.select_nth_unstable(k).1;
    Some((m, spread))
}

/// Per-channel median of a rectangle and the distance of its 75th-percentile pixel from it.
fn median_spread(img: &Bgra, x0: i32, x1: i32, y0: i32, y1: i32) -> Option<([u8; 3], i32)> {
    let mut px: Vec<[u8; 3]> = Vec::new();
    for y in y0..y1 {
        for x in x0..x1 {
            px.push(img.rgb(x, y)?);
        }
    }
    median_spread_of(&px)
}

/// The card's last visible row: the band with the highest row number.
fn last_band(card: &Card) -> Option<(usize, &tp_bridge::cards::RowBand)> {
    card.bands.iter().enumerate().max_by_key(|(_, b)| b.row)
}

/// True when `inner` lies entirely inside `outer` (both client rectangles).
fn inside(inner: Rect, outer: Rect) -> bool {
    inner.x >= outer.x && inner.y >= outer.y && inner.x + inner.w <= outer.x + outer.w && inner.y + inner.h <= outer.y + outer.h
}

fn img_rect(img: &Bgra) -> Rect {
    Rect { x: img.origin_x, y: img.origin_y, w: img.width as i32, h: img.height as i32 }
}

/// The candidate offsets from the expected position, nearest first.
fn offsets() -> impl Iterator<Item = i32> {
    (0..=SLACK).flat_map(|d| if d == 0 { vec![0] } else { vec![-d, d] })
}

/// The lane under a window card (a bubble or the reply card) in the capture `img`: rows `row_px`
/// apart, `k` the request's row count (None for a slot the Helper does not know: "Not received";
/// the lane is then taken right under the last visible row), `bg` the card's background samples.
/// None when it is not there (hidden, covered, scrolled away, outside `view`, the request's last
/// row hidden, a fill that is not the card's own) or when fewer than 60% of its columns pass.
pub fn find_window_lane(img: &Bgra, card: &Card, k: Option<u8>, row_px: f32, view: Rect, bg: &CardBg) -> Option<Lane> {
    let rp = row_px.round() as i32;
    if rp < 2 * EDGE + 1 {
        return None;
    }
    // the last visible row tells where row K would start; the addon draws no lane under a card
    // whose last row is hidden (cut by the thread, or under a tooltip)
    let (li, last) = last_band(card)?;
    if k.is_some_and(|k| last.row + 1 != k) {
        return None;
    }
    let k = k.unwrap_or(last.row + 1);
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
    // the card's own fill, sampled next to its last row: a tooltip's or a menu's body is not it
    let own = bg.band(li).left.map(|s| s.rgb);
    // the fill: the median of the tag column over the rows every candidate shares (each candidate
    // then only checks its edge rows); per candidate when the lane is too short for that
    let shared = if rp > 2 * SLACK { median_spread(img, tx0, tx1, y_exp + SLACK, y_exp + rp - SLACK) } else { None };
    let mut best: Option<(usize, i32, Lane)> = None;
    for dy in offsets() {
        let y0 = y_exp + dy;
        let rect = Rect { x: tx0, y: y0, w: end - tx0, h: rp };
        if !inside(rect, img_rect(img)) || !inside(rect, view) {
            continue;
        }
        let Some((fill, spread)) = shared.or_else(|| median_spread(img, tx0, tx1, y0, y0 + rp)) else { continue };
        if spread > SPREAD_MAX || own.is_some_and(|o| dist(o, fill) > FILL_TOL) {
            continue;
        }
        let edge_rows: Vec<i32> = (y0..y0 + EDGE).chain(y0 + rp - EDGE..y0 + rp).collect();
        let cols: Vec<bool> = (tx0..end).map(|x| edge_rows.iter().all(|&y| window_edge_ok(img, x, y, fill))).collect();
        let n = cols.iter().filter(|c| **c).count();
        let all = n == cols.len();
        // candidates come nearest first: only a strictly better one replaces the best
        if best.as_ref().is_none_or(|(bn, _, _)| n > *bn) {
            best = Some((n, dy, Lane::flat(rect, fill, cols)));
        }
        if all {
            break;
        }
    }
    let (n, _, lane) = best?;
    (n as f32 >= MIN_SHARE * lane.cols.len() as f32).then_some(lane)
}

/// A window lane's edge pixel: the fill (within `TOL`), or the hint's own glyph reaching the edge
/// row at a small UI scale: gold or link-blue lane ink over the fill, or, within `NEAR_INK` px of
/// such ink, a mix of the fill, the text shadow and a lane colour (`is_glyph_mix`). A tooltip's or
/// a menu's border and body are neither.
fn window_edge_ok(img: &Bgra, x: i32, y: i32, fill: [u8; 3]) -> bool {
    let Some(p) = img.rgb(x, y) else { return false };
    if dist(p, fill) <= TOL {
        return true;
    }
    let ink = |p: [u8; 3]| is_gold(p, fill) || is_link_blue(p, fill);
    if ink(p) {
        return true;
    }
    let near = (y - NEAR_INK..=y + NEAR_INK).any(|yy| (x - NEAR_INK..=x + NEAR_INK).any(|xx| img.rgb(xx, yy).is_some_and(ink)));
    near && (is_glyph_mix(p, fill, LANE_GOLD) || is_glyph_mix(p, fill, LANE_LINK))
}

/// True when `p` is the ground `g` blended toward a warm gold (WoW's NORMAL_FONT_COLOR
/// (255, 209, 0) and its neighbours, such as ffcc00 or e6b800). The text colour is estimated
/// from the blend, taking its red as full: `a = (p.r - g.r) / (255 - g.r)`, then its green and
/// blue `g + (p - g) / a`, which must be `GOLD_G` and at most `GOLD_B_MAX`. Red must rise by
/// `GOLD_MIN` or more (a fainter edge pixel says too little). White, yellow (255, 255, 0), the
/// pale gold of the glossary row (230, 204, 128), orange and the channel colours fail over any
/// ground.
pub fn is_gold(p: [u8; 3], g: [u8; 3]) -> bool {
    let d = [0, 1, 2].map(|c| i32::from(p[c]) - i32::from(g[c]));
    let room = 255 - i32::from(g[0]);
    if d[0] < GOLD_MIN || room < GOLD_MIN {
        return false;
    }
    // est = g + d / a with a = d.r / room, compared after multiplying by d.r (integers only)
    let est = |c: usize| i32::from(g[c]) * d[0] + d[c] * room;
    let (lo, hi) = GOLD_G;
    est(1) >= lo * d[0] && est(1) <= hi * d[0] && est(2) <= GOLD_B_MAX * d[0]
}

/// True when `p` is a mix of the ground `g`, the text shadow (black) and the text colour `c`:
/// `p = k g + f c` with k, f in 0..1 and `k + f <= 1.15` (k: the ground's share, darkened by the
/// shadow; f: the glyph's coverage), within `FRINGE_RESID` on every channel. A glyph's
/// antialiased edge over its shadow, or the shadow alone, is such a mix; a tooltip's border
/// (brighter than any mix) or its navy body over a brighter ground (bluer than the ground while
/// darker) is not.
pub fn is_glyph_mix(p: [u8; 3], g: [u8; 3], c: [u8; 3]) -> bool {
    glyph_mix_within(p, g, c, FRINGE_RESID, 1.15)
}

/// [`is_glyph_mix`] with its residual limit and largest sum of the two shares.
fn glyph_mix_within(p: [u8; 3], g: [u8; 3], c: [u8; 3], max_resid: f32, max_sum: f32) -> bool {
    let (p, g, c) = (p.map(f32::from), g.map(f32::from), c.map(f32::from));
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let (gg, gc, cc, gp, cp) = (dot(g, g), dot(g, c), dot(c, c), dot(g, p), dot(c, p));
    let det = gg * cc - gc * gc;
    let (mut k, mut f) = if det.abs() > 1.0 { ((gp * cc - gc * cp) / det, (gg * cp - gc * gp) / det) } else { (0.0, cp / cc.max(1.0)) };
    // the least-squares solution, then each share clamped to 0..1 with the other refitted
    for _ in 0..2 {
        k = k.clamp(0.0, 1.0);
        f = if cc > 0.0 { ((cp - k * gc) / cc).clamp(0.0, 1.0) } else { 0.0 };
        k = if gg > 0.0 { ((gp - f * gc) / gg).clamp(0.0, 1.0) } else { 0.0 };
    }
    let resid = (0..3).map(|i| (k * g[i] + f * c[i] - p[i]).abs()).fold(0.0, f32::max);
    k + f <= max_sum && resid <= max_resid
}

/// A pixel that may belong to a lane glyph next to it: a mix of the ground, the shadow and the
/// hint's gold or the link's blue (`is_glyph_mix`), over a ground whose pixels stray up to
/// `texture` levels beyond `CHAT_TOL` (the residual and the shares' sum may grow with it).
fn is_fringe_within(p: [u8; 3], g: [u8; 3], texture: f32) -> bool {
    [LANE_GOLD, LANE_LINK].iter().any(|c| glyph_mix_within(p, g, *c, FRINGE_RESID + texture / 2.0, 1.15 + texture / 100.0))
}

/// True when `p` is the ground `g` blended toward the light blue of the addon's links (66bbff:
/// [T], [Download]): the text colour estimated with its blue taken as full must have the red
/// `LINK_R` and the green `LINK_G`; blue must rise by `GOLD_MIN` or more.
pub fn is_link_blue(p: [u8; 3], g: [u8; 3]) -> bool {
    let d = [0, 1, 2].map(|c| i32::from(p[c]) - i32::from(g[c]));
    let room = 255 - i32::from(g[2]);
    if d[2] < GOLD_MIN || room < GOLD_MIN {
        return false;
    }
    // est = g + d / a with a = d.b / room, compared after multiplying by d.b (integers only)
    let est = |c: usize| i32::from(g[c]) * d[2] + d[c] * room;
    let ((rlo, rhi), (glo, ghi)) = (LINK_R, LINK_G);
    est(0) >= rlo * d[2] && est(0) <= rhi * d[2] && est(1) >= glo * d[2] && est(1) <= ghi * d[2]
}

/// What a chat lane pixel is against a ground model (`LaneGrid::class_in`).
const GROUND: u8 = 0;
/// The lane's own ink: the ground blended toward the hint's gold or the link's blue.
const CORE: u8 = 1;
/// Brighter than the ground in another colour (another line's text, a bright border).
const INK: u8 = 2;
/// Anything else (a cover's body, a shadow).
const OTHER: u8 = 3;

/// The ground a chat lane's pixels are judged against (a colour and a tolerance per column).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ground {
    /// The gradient between the row samples, within `CHAT_TOL`.
    Gradient,
    /// The local ground and its texture (`LaneGrid::ground`).
    Local,
}

impl Ground {
    /// Index of the grid's verdict memos.
    fn id(self) -> usize {
        match self {
            Ground::Gradient => 0,
            Ground::Local => 1,
        }
    }
}

/// A local ground estimate: the median and the texture (75th-percentile distance), or None when
/// too few plain pixels were found (the gradient stays).
type Estimate = Option<([u8; 3], i32)>;

/// The plain pixels the local ground is taken from, and its estimates per block of columns.
struct LocalPool {
    /// The plain pixels of every column, column after column.
    pool: Vec<[u8; 3]>,
    /// Where each column's pixels start in `pool` (one more entry: the end).
    starts: Vec<usize>,
    /// Per block of `LOCAL_STEP` columns: None not made yet, else its estimate.
    blocks: Vec<Option<Estimate>>,
}

/// How a candidate lane is judged.
struct Judge<'a> {
    /// First row of the candidate (it may lie above the visible rows: an orphan cut by the top).
    y0: i32,
    /// Its height (one line pitch).
    lh: i32,
    /// First visible row (y0, or the view's top for an orphan cut by it).
    vy0: i32,
    /// The tag column ends here (the plate's x).
    plate_x0: i32,
    /// Per column: the card's last row shows its plate above it (always true left of the plate).
    above: &'a [bool],
}

/// The pixels of every row a candidate lane may use. Lane ink (against the gradient of the row
/// samples) is found when first needed and kept for the other candidates.
struct LaneGrid {
    rect: Rect,
    /// The grid's pixels, row by row (read once).
    px: Vec<[u8; 3]>,
    /// The gradient ground of each column (the row samples).
    grad: Vec<[u8; 3]>,
    /// Lane ink against the gradient, per pixel.
    core: Vec<bool>,
    /// Lane ink within 1 and 2 px of each pixel (made when first needed).
    near: [Option<Vec<bool>>; 2],
    /// Per ground (`Ground::id`), what `edge_ok` and `other_ink` said of each pixel: 0 not asked,
    /// 1 yes, 2 no (the candidates share most of their rows).
    edge_memo: [Vec<u8>; 2],
    ink_memo: [Vec<u8>; 2],
    /// The local ground, once a column fails against the gradient.
    local: Option<LocalPool>,
}

impl LaneGrid {
    /// None when `rect` is not entirely inside the capture.
    fn new(img: &Bgra, rect: Rect, fill: &BandBg) -> Option<LaneGrid> {
        if !inside(rect, img_rect(img)) || rect.w <= 0 || rect.h <= 0 {
            return None;
        }
        let n = (rect.w * rect.h) as usize;
        let w = rect.w as usize;
        let grad: Vec<[u8; 3]> = (rect.x..rect.x + rect.w).map(|x| fill.at(x).unwrap_or([0, 0, 0])).collect();
        let mut px: Vec<[u8; 3]> = Vec::with_capacity(n);
        for y in rect.y..rect.y + rect.h {
            let o = (y - img.origin_y) as usize * img.stride as usize + (rect.x - img.origin_x) as usize * 4;
            match img.px.get(o..o + w * 4) {
                Some(bytes) => px.extend(bytes.chunks_exact(4).map(|b| [b[2], b[1], b[0]])),
                None => px.extend((rect.x..rect.x + rect.w).map(|x| img.rgb(x, y).unwrap_or([0, 0, 0]))),
            }
        }
        // lane ink rises by GOLD_MIN on red (gold) or blue (the link) at least: a quick test first
        let mut core = Vec::with_capacity(n);
        for row in px.chunks_exact(w) {
            core.extend(row.iter().zip(&grad).map(|(p, g)| {
                (i32::from(p[0]) - i32::from(g[0]) >= GOLD_MIN || i32::from(p[2]) - i32::from(g[2]) >= GOLD_MIN) && (is_gold(*p, *g) || is_link_blue(*p, *g))
            }));
        }
        Some(LaneGrid { rect, px, grad, core, near: [None, None], edge_memo: [vec![0; n], vec![0; n]], ink_memo: [vec![0; n], vec![0; n]], local: None })
    }

    fn w(&self) -> usize {
        self.rect.w as usize
    }

    fn index(&self, x: i32, y: i32) -> usize {
        ((y - self.rect.y) * self.rect.w + (x - self.rect.x)) as usize
    }

    fn pixel(&self, x: i32, y: i32) -> [u8; 3] {
        self.px[self.index(x, y)]
    }

    fn col(&self, x: i32) -> usize {
        (x - self.rect.x) as usize
    }

    /// The grid's pixels on row `y`.
    fn row(&self, y: i32) -> &[[u8; 3]] {
        let o = ((y - self.rect.y) * self.rect.w) as usize;
        &self.px[o..o + self.w()]
    }

    /// Lane ink against the gradient.
    fn core(&self, x: i32, y: i32) -> bool {
        self.core[self.index(x, y)]
    }

    /// Lane ink within `r` px (inside the grid): from the dilation of the whole grid's lane ink
    /// when [`ensure_near`](Self::ensure_near) made it, else looked up around the pixel.
    fn near_core(&self, x: i32, y: i32, r: i32) -> bool {
        if let Some(m) = (1..=2).contains(&r).then(|| self.near[(r - 1) as usize].as_ref()).flatten() {
            return m[self.index(x, y)];
        }
        let g = self.rect;
        let (x0, x1) = ((x - r).max(g.x), (x + r).min(g.x + g.w - 1));
        let (y0, y1) = ((y - r).max(g.y), (y + r).min(g.y + g.h - 1));
        (y0..=y1).any(|yy| (x0..=x1).any(|xx| self.core(xx, yy)))
    }

    /// Makes the dilation of radius `r` (1 or 2) for [`near_core`](Self::near_core), for passes
    /// that ask about every pixel.
    fn ensure_near(&mut self, r: i32) {
        let k = (r - 1) as usize;
        if self.near[k].is_none() {
            self.near[k] = Some(self.dilate(r));
        }
    }

    /// The grid's lane ink dilated by `r` px (a square of 2 r + 1).
    fn dilate(&self, r: i32) -> Vec<bool> {
        let (w, h) = (self.rect.w, self.rect.h);
        let mut horiz = vec![false; self.core.len()];
        for y in 0..h {
            let row = &self.core[(y * w) as usize..((y + 1) * w) as usize];
            for x in 0..w {
                horiz[(y * w + x) as usize] = row[(x - r).max(0) as usize..=(x + r).min(w - 1) as usize].iter().any(|v| *v);
            }
        }
        let mut out = vec![false; self.core.len()];
        for y in 0..h {
            for x in 0..w {
                out[(y * w + x) as usize] = ((y - r).max(0)..=(y + r).min(h - 1)).any(|yy| horiz[(yy * w + x) as usize]);
            }
        }
        out
    }

    /// Starts the local ground on rows `y0..y1` (the expected lane's rows: a cover from below has
    /// its edge found by `cover_cols` first), once: the plain pixels of every column (not lane
    /// ink, not within `NEAR_INK` px of it, not more than `LOCAL_BRIGHT` brighter than the
    /// gradient). Its estimates are made per block of columns when first asked (`ground`).
    fn init_local(&mut self, y0: i32, y1: i32) {
        if self.local.is_some() {
            return;
        }
        let w = self.w();
        self.ensure_near(NEAR_INK);
        let mut pool: Vec<[u8; 3]> = Vec::with_capacity(w * (y1 - y0).max(0) as usize);
        let mut starts: Vec<usize> = Vec::with_capacity(w + 1);
        for i in 0..w {
            starts.push(pool.len());
            let x = self.rect.x + i as i32;
            let gl = luma(self.grad[i]);
            for y in y0.max(self.rect.y)..y1.min(self.rect.y + self.rect.h) {
                // not a glyph's antialiased edge or its shadow either
                if self.near_core(x, y, NEAR_INK) {
                    continue;
                }
                let p = self.pixel(x, y);
                if luma(p) - gl <= LOCAL_BRIGHT {
                    pool.push(p);
                }
            }
        }
        starts.push(pool.len());
        self.local = Some(LocalPool { pool, starts, blocks: vec![None; w.div_ceil(LOCAL_STEP)] });
    }

    /// The ground colour and tolerance of column `i`. The gradient: the row samples, `CHAT_TOL`.
    /// The local ground: the median of the plain pixels of the `LOCAL_R` columns on each side (2
    /// or 4 times as many where glyphs leave fewer than `LOCAL_MIN`; at most `LOCAL_POOL`, evenly
    /// picked), one estimate per block of `LOCAL_STEP` columns, kept within `LOCAL_MAX` of the
    /// gradient; the tolerance 2.5 times their texture (the 75th-percentile distance from the
    /// median: a step between two grounds widens it too) plus 8, from `CHAT_TOL` to `TOL_MAX`.
    /// Too few plain pixels: the gradient.
    fn ground(&mut self, m: Ground, i: usize) -> ([u8; 3], i32) {
        let grad = self.grad[i];
        let Some(lp) = self.local.as_mut().filter(|_| m == Ground::Local) else { return (grad, CHAT_TOL) };
        let b = i / LOCAL_STEP;
        if lp.blocks[b].is_none() {
            let w = lp.starts.len() - 1;
            let c = (b * LOCAL_STEP + LOCAL_STEP / 2).min(w - 1);
            let mut win: Vec<[u8; 3]> = Vec::with_capacity(LOCAL_POOL);
            for r in [LOCAL_R, 2 * LOCAL_R, 4 * LOCAL_R] {
                let span = &lp.pool[lp.starts[c.saturating_sub(r)]..lp.starts[(c + r + 1).min(w)]];
                win.clear();
                win.extend(span.iter().step_by(span.len().div_ceil(LOCAL_POOL).max(1)));
                if win.len() >= LOCAL_MIN {
                    break;
                }
            }
            lp.blocks[b] = Some(median_spread_of(&win).filter(|_| win.len() >= LOCAL_MIN));
        }
        match lp.blocks[b] {
            Some(Some((med, tex))) => (
                [0, 1, 2].map(|c| i32::from(med[c]).clamp(i32::from(grad[c]) - LOCAL_MAX, i32::from(grad[c]) + LOCAL_MAX).clamp(0, 255) as u8),
                (5 * tex / 2 + 8).clamp(CHAT_TOL, TOL_MAX),
            ),
            _ => (grad, CHAT_TOL),
        }
    }

    fn class_in(&mut self, m: Ground, x: i32, y: i32) -> u8 {
        let (g, tol) = self.ground(m, self.col(x));
        let p = self.pixel(x, y);
        if dist(p, g) <= tol {
            GROUND
        } else if self.core(x, y) || is_gold(p, g) || is_link_blue(p, g) {
            CORE
        } else if luma(p) - luma(g) >= INK_LUMA {
            INK
        } else {
            OTHER
        }
    }

    /// What an edge row may hold: ground, lane ink, or a glyph's antialiased edge or shadow next
    /// to lane ink (a mix of the ground, black and the lane's colours, `is_fringe_within`; over a
    /// textured ground the mix may stray as far as the texture does).
    fn edge_ok(&mut self, m: Ground, x: i32, y: i32) -> bool {
        let k = self.index(x, y);
        if self.edge_memo[m.id()][k] == 0 {
            let ok = match self.class_in(m, x, y) {
                GROUND | CORE => true,
                _ => {
                    let (g, tol) = self.ground(m, self.col(x));
                    let texture = (tol - CHAT_TOL).max(0) as f32;
                    self.near_core(x, y, NEAR_INK) && is_fringe_within(self.pixel(x, y), g, texture)
                }
            };
            self.edge_memo[m.id()][k] = if ok { 1 } else { 2 };
        }
        self.edge_memo[m.id()][k] == 1
    }

    /// Text of another colour, with no lane ink next to it.
    fn other_ink(&mut self, m: Ground, x: i32, y: i32) -> bool {
        let k = self.index(x, y);
        if self.ink_memo[m.id()][k] == 0 {
            let ink = self.class_in(m, x, y) == INK && !self.near_core(x, y, NEAR_INK);
            self.ink_memo[m.id()][k] = if ink { 1 } else { 2 };
        }
        self.ink_memo[m.id()][k] == 1
    }

    /// Columns under a cover's straight horizontal edge inside rows `y0..y1` (each row against the
    /// one above it, from `y0 + 1`): a run of at least `RUN_MIN` columns whose pixels jump by
    /// `EDGE_JUMP` or more from the row above (not both lane ink) while one of the two rows stays
    /// flat along the run (`RUN_UNI`) and holds no lane ink, plus `RUN_PAD` columns on each side.
    /// A tooltip's border or a panel's top (or bottom) makes one, also right under the hint's
    /// glyphs; a glyph's stroke and its shadow are shorter, and a textured world is not flat.
    fn cover_cols(&self, y0: i32, y1: i32) -> Vec<bool> {
        let w = self.w();
        let mut out = vec![false; w];
        let mark = |s: usize, e: usize, out: &mut Vec<bool>| {
            if e - s >= RUN_MIN {
                for c in out.iter_mut().take((e + RUN_PAD).min(w)).skip(s.saturating_sub(RUN_PAD)) {
                    *c = true;
                }
            }
        };
        let first = (y0 + 1).max(self.rect.y + 1);
        for y in first..y1.min(self.rect.y + self.rect.h) {
            let (here, above) = (self.row(y), self.row(y - 1));
            let mut start = 0usize;
            let mut run = false;
            // the previous pixel pair and whether each is lane ink (known for a hit only: a run
            // goes on only from a hit)
            let mut prev = ([0u8; 3], [0u8; 3], true, true);
            for i in 0..w {
                let x = self.rect.x + i as i32;
                let (p, up) = (here[i], above[i]);
                let jump = dist(p, up) >= EDGE_JUMP;
                let (cp, cu) = if jump { (self.core(x, y), self.core(x, y - 1)) } else { (true, true) };
                let hit = jump && !(cp && cu);
                let flat_low = !cp && !prev.2 && dist(p, prev.0) <= RUN_UNI;
                let flat_up = !cu && !prev.3 && dist(up, prev.1) <= RUN_UNI;
                let cont = hit && run && (flat_low || flat_up);
                if run && !cont {
                    mark(start, i, &mut out);
                    run = false;
                }
                if hit && !run {
                    start = i;
                    run = true;
                }
                prev = (p, up, cp, cu);
            }
            if run {
                mark(start, w, &mut out);
            }
        }
        out
    }

    /// The passing columns of a candidate against the ground `m` (None: its tag column holds
    /// other ink, so it is not a lane). Every visible edge row is checked: the bottom two, and the
    /// top two when the lane is not cut by the top of the view.
    fn judge(&mut self, m: Ground, j: &Judge, covers: &[bool]) -> Option<Vec<bool>> {
        let tx0 = self.rect.x;
        let y1 = j.y0 + j.lh;
        // a lane, not the next line: no other ink in the tag column
        let total = ((y1 - j.vy0) * (j.plate_x0 - tx0)).max(0) as usize;
        let limit = (TAG_INK_MAX * total as f32) as usize;
        let mut ink = 0usize;
        'scan: for y in j.vy0..y1 {
            for x in tx0..j.plate_x0 {
                ink += usize::from(self.other_ink(m, x, y));
                if ink > limit {
                    break 'scan;
                }
            }
        }
        if total == 0 || ink > limit {
            return None;
        }
        let rows = edge_rows(j);
        let w = self.w();
        Some((0..w).map(|i| j.above[i] && !covers[i] && rows.iter().all(|&y| self.edge_ok(m, tx0 + i as i32, y))).collect())
    }

    /// [`judge`](Self::judge) against the gradient; when a column the cover edges and the mask
    /// leave open fails there and `local_rows` is given, that column is judged again against the
    /// local ground of those rows (the whole candidate when its tag column failed).
    fn judge_best(&mut self, j: &Judge, covers: &[bool], local_rows: Option<(i32, i32)>) -> Option<Vec<bool>> {
        let first = self.judge(Ground::Gradient, j, covers);
        let Some((ly0, ly1)) = local_rows else { return first };
        let w = self.w();
        let open = |i: usize| j.above[i] && !covers[i];
        if first.as_ref().is_some_and(|c| (0..w).all(|i| c[i] || !open(i))) {
            return first;
        }
        self.init_local(ly0, ly1);
        match first {
            Some(mut cols) => {
                let rows = edge_rows(j);
                let tx0 = self.rect.x;
                for (i, ok) in cols.iter_mut().enumerate() {
                    if !*ok && open(i) {
                        *ok = rows.iter().all(|&y| self.edge_ok(Ground::Local, tx0 + i as i32, y));
                    }
                }
                Some(cols)
            }
            None => self.judge(Ground::Local, j, covers),
        }
    }
}

/// The edge rows a candidate checks: its bottom two, and its top two unless the view cuts it.
fn edge_rows(j: &Judge) -> Vec<i32> {
    let y1 = j.y0 + j.lh;
    let top: Vec<i32> = if j.vy0 == j.y0 { (j.y0..j.y0 + EDGE).collect() } else { Vec::new() };
    top.into_iter().chain((y1 - EDGE).max(j.vy0)..y1).collect()
}

/// True when the indent `[x0, x1)` of rows `y0..y1` holds ink (more than `TAG_INK_MAX` of its
/// pixels): lane ink, or anything brighter than its ground by `INK_LUMA` and beyond its texture.
/// The ground is the indent's own median, the tolerance twice its texture plus 8 (`CHAT_TOL` to
/// `TOL_MAX`). Fewer than 3 columns: no check.
fn indent_ink(img: &Bgra, (x0, x1): (i32, i32), y0: i32, y1: i32) -> bool {
    if x1 - x0 < 3 {
        return false;
    }
    let mut px: Vec<[u8; 3]> = Vec::with_capacity(((x1 - x0) * (y1 - y0)).max(0) as usize);
    for y in y0..y1 {
        for x in x0..x1 {
            if let Some(p) = img.rgb(x, y) {
                px.push(p);
            }
        }
    }
    let Some((g, s)) = median_spread_of(&px) else { return false };
    let tol = (2 * s + 8).clamp(CHAT_TOL, TOL_MAX);
    let ink = px.iter().filter(|&&p| is_gold(p, g) || is_link_blue(p, g) || (dist(p, g) > tol && luma(p) - luma(g) >= INK_LUMA)).count();
    ink as f32 > TAG_INK_MAX * px.len() as f32
}

/// The indent left of a tag column starting at `tx0`, `[x0, x1)`: up to `backdrop::LEFT_MAX` px,
/// inside the view and the capture, `backdrop::EDGE_SKIP` px clear of the tag column.
fn indent_of(tx0: i32, view: Rect, lim: Rect) -> (i32, i32) {
    ((tx0 - 1 - backdrop::LEFT_MAX).max(view.x).max(lim.x), tx0 - 1 - backdrop::EDGE_SKIP)
}

/// The line pitch of a chat card: measured on its own rows, else the VIEW's `row_px`, else its
/// row height + 1.
pub fn chat_pitch(card: &Card, row_px: f32) -> Option<f32> {
    let (_, last) = last_band(card)?;
    let first = card.bands.iter().min_by_key(|b| b.row)?;
    Some(if last.row > first.row {
        (last.text_rect.y - first.text_rect.y) as f32 / f32::from(last.row - first.row)
    } else if row_px > 0.0 {
        row_px
    } else {
        last.text_rect.h as f32 + 1.0
    })
}

/// The lane under an inline chat card in the capture `img`. `k`: the request's row count (None
/// for a slot the Helper does not know: the last visible row counts as the last); `row_px`: the
/// chat's line pitch (VIEW rowPx); `view`: the card's chat view (the whole client in discovery);
/// `bg`: the card's background samples (`backdrop::sample_card`): the lane's ground is its last
/// row's (the same gradient the overlay paints the rows above with). None when no lane is there
/// (the next chat line, the glossary row, nothing), when it is covered, outside the view, or when
/// fewer than 60% of its columns pass. The expected position is tried first, then up to `SLACK` px
/// off; a candidate whose every column passes ends the search.
pub fn find_chat_lane(img: &Bgra, card: &Card, k: Option<u8>, row_px: f32, view: Rect, bg: &CardBg) -> Option<Lane> {
    find_chat_lane_ink(img, card, k, row_px, view, bg).map(|(l, _)| l)
}

/// [`find_chat_lane`] with the lane's ink: per pixel of its rectangle (row by row), lane ink (the
/// hint's gold or the link's blue against the row samples), for [`ChatLaneMemo`].
pub fn find_chat_lane_ink(img: &Bgra, card: &Card, k: Option<u8>, row_px: f32, view: Rect, bg: &CardBg) -> Option<(Lane, Vec<bool>)> {
    if !card.plate {
        return None;
    }
    let (li, last) = last_band(card)?;
    if k.is_some_and(|k| last.row + 1 != k) {
        // the request's last row is hidden: whatever hides it may hide the lane too
        return None;
    }
    let mask = last.mask.as_ref()?;
    let lh = chat_pitch(card, row_px)?.round() as i32;
    if lh < 2 * EDGE + 1 {
        return None;
    }
    // one line pitch under the last row's top; a last row cut by the top of the chat (its visible
    // rows only) is measured from its bottom, with the height of a whole row of the card
    let full_h = card.bands.iter().filter(|b| b.text_rect.y > view.y + 1).map(|b| b.text_rect.h).max().unwrap_or(lh);
    let y_exp = if last.text_rect.y <= view.y + 1 && last.text_rect.h < full_h {
        last.text_rect.y + last.text_rect.h - full_h + lh
    } else {
        last.text_rect.y + lh
    };
    let tx0 = last.tag_rect.x + 1;
    let plate_x0 = mask.rect.x;
    let end = card.bands.iter().map(|b| b.text_rect.x + b.text_rect.w).max()?;
    if end <= plate_x0 || plate_x0 <= tx0 {
        return None;
    }
    // the rows of every candidate, inside the capture and the view
    let lim = img_rect(img);
    let (top, bot) = ((y_exp - SLACK).max(lim.y).max(view.y), (y_exp + lh + SLACK).min(lim.y + lim.h).min(view.y + view.h));
    if bot - top < lh {
        return None;
    }
    let fill = bg.band(li);
    if fill.is_empty() {
        return None;
    }
    let mut grid = LaneGrid::new(img, Rect { x: tx0, y: top, w: end - tx0, h: bot - top }, &fill)?;
    // under the plate, a column counts only where the card's last row shows its plate (mask)
    let plate_rows: Vec<i32> = (mask.rect.y + mask.rect.h - 3..mask.rect.y + mask.rect.h).collect();
    let above: Vec<bool> = (tx0..end).map(|x| x < plate_x0 || plate_rows.iter().any(|&y| mask.get(x, y))).collect();
    // the cover edges of the expected lane's rows, from the first row after the last tag row and
    // its blended partial row (its plate's and tag's own bottom edge is no cover), the same for
    // every candidate; the local ground from the expected lane's rows, made when first needed
    let after = (last.text_rect.y + last.text_rect.h).max(last.tag_rect.y + last.tag_rect.h);
    let covers = grid.cover_cols(after.max(top), (y_exp + lh).min(bot));
    let local_rows = (y_exp.max(top), (y_exp + lh).min(bot));
    // a lane, not the next line: its indent holds no ink (a timestamp, a channel, a name, the
    // addon's own "TranslateProbe:" prefix start there; the lane holds only spaces); checked once,
    // on the expected rows
    if indent_ink(img, indent_of(tx0, view, lim), local_rows.0, local_rows.1) {
        return None;
    }
    // the most columns a candidate can pass: those under the plate (mask) and no cover edge
    let open = (0..above.len()).filter(|&i| above[i] && !covers[i]).count();
    let mut best: Option<(usize, i32, Vec<bool>)> = None;
    for dy in offsets() {
        let y0 = y_exp + dy;
        if y0 < top || y0 + lh > bot {
            continue;
        }
        let j = Judge { y0, lh, vy0: y0, plate_x0, above: &above };
        let Some(cols) = grid.judge_best(&j, &covers, Some(local_rows)) else { continue };
        let n = cols.iter().filter(|c| **c).count();
        let all = n == open;
        // candidates come nearest first: only a strictly better one replaces the best
        if best.as_ref().is_none_or(|(bn, _, _)| n > *bn) {
            best = Some((n, dy, cols));
        }
        if all {
            break;
        }
    }
    let (n, dy, cols) = best?;
    // the lane follows the card's last painted row: a row the band does not hold (a fractional
    // line position) is painted with it
    let y0 = y_exp + dy;
    let first_row = after.clamp(y0 - SLACK, y0);
    let rect = Rect { x: tx0, y: first_row, w: end - tx0, h: y0 + lh - first_row };
    if (n as f32) < MIN_SHARE * cols.len() as f32 {
        return None;
    }
    let ink = (rect.y..rect.y + rect.h)
        .flat_map(|y| (rect.x..rect.x + rect.w).map(move |x| (x, y)))
        .map(|(x, y)| y >= grid.rect.y && y < grid.rect.y + grid.rect.h && grid.core(x, y))
        .collect();
    Some((Lane { rect, kind: LaneKind::Chat, fill, cols }, ink))
}

/// A chat lane the capture found, remembered to follow it once its card's rows have left the top
/// of the chat (an orphan lane, [`find_orphan_chat_lane`]).
#[derive(Clone, Debug)]
pub struct ChatLaneMemo {
    pub view_id: u8,
    pub slot: u8,
    /// The lane where it was last found (client px, not cut by the view).
    pub rect: Rect,
    /// The tag column ends here (the plate's x).
    pub plate_x0: i32,
    /// The chat's line pitch (px, fractional).
    pub pitch: f32,
    /// The view it was found in: a new VIEW (moved, resized, another font) drops the memo.
    pub view: Rect,
    /// When it was last found (ms, the capture clock).
    pub at_ms: u64,
    /// Lane ink of `rect` when it was found whole under its card (row by row; empty when not
    /// known): an orphan is the line whose ink matches it, not a blank line or another message.
    pub ink: Vec<bool>,
}

impl ChatLaneMemo {
    /// True when the memo was made in a view of the rectangle `view`.
    pub fn same_view(&self, view: Rect) -> bool {
        rect4(&self.view) == rect4(&view)
    }

    /// The memo of a chat lane found under `card` with its ink (`find_chat_lane_ink`; None when
    /// the card has no plate mask).
    pub fn of(card: &Card, lane: &Lane, ink: Vec<bool>, row_px: f32, view: Rect, at_ms: u64) -> Option<ChatLaneMemo> {
        let (_, last) = last_band(card)?;
        let plate_x0 = last.mask.as_ref()?.rect.x;
        let pitch = chat_pitch(card, row_px)?;
        Some(ChatLaneMemo { view_id: card.view_id, slot: card.slot, rect: lane.rect, plate_x0, pitch, view, at_ms, ink })
    }

    /// How well the lane ink of the grid on rows `vy0..y1`, the lane's whole rectangle starting at
    /// `y0`, matches the memo's ink there: Some(the pixels both hold) when at least
    /// `ORPHAN_INK_MATCH` of the memo's ink pixels there are ink again (`ORPHAN_MIN_INK` at least)
    /// and few new ones are (`ORPHAN_INK_EXTRA`).
    fn ink_match(&self, grid: &LaneGrid, y0: i32, vy0: i32, y1: i32) -> Option<usize> {
        let w = self.rect.w as usize;
        if self.ink.len() != w * self.rect.h as usize {
            return None;
        }
        let (mut want, mut both, mut now) = (0usize, 0usize, 0usize);
        for y in vy0..y1 {
            let r = (y - y0) as usize;
            for i in 0..w {
                let was = self.ink[r * w + i];
                let is = grid.core(self.rect.x + i as i32, y);
                want += usize::from(was);
                now += usize::from(is);
                both += usize::from(was && is);
            }
        }
        let ok = want >= ORPHAN_MIN_INK
            && both as f32 >= ORPHAN_INK_MATCH * want as f32
            && (now - both) as f32 <= ORPHAN_INK_EXTRA * want as f32 + ORPHAN_MIN_INK as f32;
        ok.then_some(both)
    }
}

/// The lane of `memo` while its card is not in the capture: the chat moved up by a whole number
/// of line pitches (0 to `ORPHAN_MAX_LINES`) and the lane is now the top line of `view`, the row
/// above it cut by the view's top (a lane lower down with its card missing is under a cover: it
/// is left alone). A candidate must show the memo's own ink at its offset (`ink_match`): the
/// hint's glyphs, so a blank line or another message is never taken; the one matching most of it
/// wins. Judged on its visible rows (at least `ORPHAN_MIN_ROWS`) against the chat background
/// sampled on its own line (left of the tag column, right of the plate's end), with the gradient
/// only, the cover edges and the visible edge rows. Returns the lane (its rectangle cut to the
/// view) and where its whole rectangle lies now, for the memo.
pub fn find_orphan_chat_lane(img: &Bgra, memo: &ChatLaneMemo, view: Rect) -> Option<(Lane, Rect)> {
    let lh = memo.rect.h;
    let (tx0, end, plate_x0) = (memo.rect.x, memo.rect.x + memo.rect.w, memo.plate_x0);
    if lh < 2 * EDGE + 1 || plate_x0 <= tx0 || end <= plate_x0 || memo.pitch <= 0.0 {
        return None;
    }
    let lim = img_rect(img);
    let vy = view.y.max(lim.y);
    let bottom = (view.y + view.h).min(lim.y + lim.h);
    // where the lane can be: a whole number of lines up, in the top line, the row above it
    // mostly cut by the top (a row with more of it showing is found by the tag scan)
    let mut bases: Vec<i32> = (0..=ORPHAN_MAX_LINES)
        .map(|n| (memo.rect.y as f32 - n as f32 * memo.pitch).round() as i32)
        .filter(|&y| ((y - vy) as f32) < ORPHAN_ROW_ABOVE * lh as f32 && y + SLACK + lh >= vy + ORPHAN_MIN_ROWS)
        .collect();
    bases.dedup();
    // every candidate that passes: the one covering the most lane ink (the lane's own rows, not
    // a blank line next to them), then the most passing columns
    let mut best: Option<(usize, usize, i32, Vec<bool>, BandBg)> = None;
    for base in bases {
        for dy in offsets() {
            let y0 = base + dy;
            let vy0 = y0.max(vy);
            let y1 = y0 + lh;
            if y0 - lh >= vy || y1 - vy0 < ORPHAN_MIN_ROWS || y1 > bottom || tx0 < lim.x || end > lim.x + lim.w {
                continue;
            }
            // the chat background on the lane's own line: its indent and the room after the plate
            let indent = indent_of(tx0, view, lim);
            let left = backdrop::robust_color(img, indent.0, indent.1 - 1, vy0, y1 - 1).map(|rgb| Sample { rgb, x: (indent.0 + indent.1 - 1) / 2 });
            let rx1 = (view.x + view.w).min(lim.x + lim.w) - 1 - backdrop::EDGE_SKIP;
            let rx0 = (end + backdrop::EDGE_SKIP).max(rx1 - backdrop::RIGHT_FAR);
            let right = backdrop::robust_color(img, rx0, rx1, vy0, y1 - 1).map(|rgb| Sample { rgb, x: (rx0 + rx1) / 2 });
            let fill = BandBg { left, right };
            if fill.is_empty() {
                continue;
            }
            let Some(mut grid) = LaneGrid::new(img, Rect { x: tx0, y: vy0, w: end - tx0, h: y1 - vy0 }, &fill) else { continue };
            // the hint itself: its remembered ink, at that offset (not a blank line, not another
            // message, not the lane a row off)
            let Some(ink) = memo.ink_match(&grid, y0, vy0, y1) else { continue };
            let above = vec![true; (end - tx0) as usize];
            // a lane, not another line: its indent holds no ink
            if indent_ink(img, indent, vy0, y1) {
                continue;
            }
            let j = Judge { y0, lh, vy0, plate_x0, above: &above };
            let covers = grid.cover_cols(vy0, y1);
            let Some(cols) = grid.judge_best(&j, &covers, None) else { continue };
            let n = cols.iter().filter(|c| **c).count();
            if (n as f32) < MIN_SHARE * cols.len() as f32 {
                continue;
            }
            if best.as_ref().is_none_or(|(bi, bn, _, _, _)| (ink, n) > (*bi, *bn)) {
                best = Some((ink, n, y0, cols, fill));
            }
        }
    }
    let (_, _, y0, cols, fill) = best?;
    let vy0 = y0.max(vy);
    let lane = Lane { rect: Rect { x: tx0, y: vy0, w: end - tx0, h: y0 + lh - vy0 }, kind: LaneKind::Chat, fill, cols };
    Some((lane, Rect { x: tx0, y: y0, w: end - tx0, h: lh }))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::backdrop::BandBg;
    use tp_bridge::cards::{Mask, RowBand};

    pub const BUBBLE: [u8; 3] = [36, 30, 24];
    const PLATE: [u8; 3] = [32, 40, 64];
    /// WoW's NORMAL_FONT_COLOR, the hint's colour.
    pub const GOLD: [u8; 3] = [255, 209, 0];
    /// The addon's links' light blue (66bbff), the [Download] link's colour.
    pub const LINK: [u8; 3] = [102, 187, 255];

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
                    self.set(x, y, rgb);
                }
            }
        }

        pub fn set(&mut self, x: i32, y: i32, rgb: [u8; 3]) {
            let i = ((y * self.w + x) * 4) as usize;
            self.px[i..i + 4].copy_from_slice(&[rgb[2], rgb[1], rgb[0], 255]);
        }

        pub fn get(&self, x: i32, y: i32) -> [u8; 3] {
            let i = ((y * self.w + x) * 4) as usize;
            [self.px[i + 2], self.px[i + 1], self.px[i]]
        }

        pub fn img(&self) -> Bgra<'_> {
            Bgra { px: &self.px, width: self.w as u32, height: self.h as u32, stride: (self.w * 4) as u32, origin_x: 0, origin_y: 0 }
        }

        /// `a` of `rgb` over what is there.
        pub fn blend(&mut self, x: i32, y: i32, rgb: [u8; 3], a: f32) {
            let o = self.get(x, y);
            self.set(x, y, [0, 1, 2].map(|c| (f32::from(o[c]) + a * (f32::from(rgb[c]) - f32::from(o[c])) + 0.5) as u8));
        }

        /// Chat-like glyphs of `rgb` from x0 to x1 over rows y0..y1 (the whole line: capitals and
        /// descenders reach its top and bottom rows), with antialiased sides and the chat font's
        /// 1 px black shadow at (+1, +1).
        pub fn glyphs(&mut self, x0: i32, x1: i32, y0: i32, y1: i32, rgb: [u8; 3]) {
            let mut x = x0;
            let mut n = 0;
            while x + 4 < x1 {
                let w = 2 + n % 3;
                let (top, bot) = if n % 4 == 0 { (y0, y1) } else if n % 4 == 2 { (y0 + 3, y1) } else { (y0 + 3, y1 - 3) };
                for y in top..bot {
                    self.blend(x + w + 1, y + 1, [0, 0, 0], 0.6);
                    self.blend(x - 1, y, rgb, 0.4);
                    for xx in x..x + w {
                        self.set(xx, y, rgb);
                    }
                    self.blend(x + w, y, rgb, 0.3);
                }
                x += w + 3;
                n += 1;
            }
        }
    }

    /// A card of `k` rows at (40, 50), rows `rp` px apart, its tag 48 px wide, plates to x 340.
    fn card_at(slot: u8, view_id: u8, k: u8, rp: i32) -> Card {
        let bands = (0..k)
            .map(|r| {
                let y = 50 + i32::from(r) * rp;
                RowBand {
                    row: r,
                    text_rect: Rect { x: 91, y, w: 249, h: rp },
                    mask: Some(Mask { rect: Rect { x: 88, y, w: 252, h: rp }, bits: vec![255; (252 * rp) as usize] }),
                    tag_rect: Rect { x: 39, y, w: 50, h: rp },
                }
            })
            .collect();
        Card { slot, view_id, bands, gaps: Vec::new(), alpha: 1.0, plate: true, plate_rgb: PLATE, text_width_px: 249 }
    }

    fn draw_rows(pic: &mut Pic, k: u8, rp: i32) {
        for r in 0..k {
            let y = 50 + i32::from(r) * rp;
            pic.fill(Rect { x: 40, y, w: 48, h: rp }, [255, 0, 255]);
            pic.fill(Rect { x: 88, y, w: 252, h: rp }, PLATE);
        }
    }

    const VIEW: Rect = Rect { x: 0, y: 0, w: 400, h: 300 };

    /// A window card's background samples: the bubble fill next to every row.
    fn bubble_bg() -> CardBg {
        CardBg { bands: vec![BandBg { left: Some(Sample { rgb: BUBBLE, x: 35 }), right: None }; 6], edges: Vec::new() }
    }

    /// A reply card of `k` rows at (40, 50), rows `rp` px apart, its tag 48 px wide, plates to
    /// x 340, inside a bubble of fill; the lane holds bright hint "text" clear of its edges.
    pub fn reply_pic(k: u8, rp: i32) -> (Pic, Card) {
        let mut pic = Pic::new(400, 300, [90, 120, 60]);
        pic.fill(Rect { x: 30, y: 40, w: 330, h: i32::from(k + 1) * rp + 20 }, BUBBLE);
        draw_rows(&mut pic, k, rp);
        let lane_y = 50 + i32::from(k) * rp;
        // hint glyphs from the plate's x, clear of the lane's top and bottom 2 px
        for i in 0..20 {
            pic.fill(Rect { x: 92 + i * 11, y: lane_y + EDGE, w: 6, h: rp - 2 * EDGE }, [200, 200, 200]);
        }
        (pic, card_at(126, 200, k, rp))
    }

    #[test]
    fn a_clean_lane_is_found_where_the_geometry_puts_it() {
        let (pic, card) = reply_pic(3, 16);
        let lane = find_window_lane(&pic.img(), &card, Some(3), 16.0, VIEW, &bubble_bg()).expect("lane");
        assert_eq!(rect4(&lane.rect), [40, 50 + 3 * 16, 300, 16]);
        assert_eq!(lane.rgb(), BUBBLE);
        assert_eq!(lane.kind, LaneKind::Window);
        assert_eq!(lane.rgb_right(), None, "flat");
        assert_eq!(lane.passing(), lane.cols.len(), "every column passes, the hint glyphs included");
        // the lane is one row below the last visible row even when the first rows scrolled away
        let mut tail = card.clone();
        tail.bands.remove(0);
        assert_eq!(find_window_lane(&pic.img(), &tail, Some(3), 16.0, VIEW, &bubble_bg()).map(|l| l.rect.y), Some(98));
        // off by one pixel: still found (rounding), and the band covers the glyphs
        let (pic1, mut card1) = reply_pic(2, 15);
        for b in card1.bands.iter_mut() {
            b.text_rect.y -= 1;
        }
        let l1 = find_window_lane(&pic1.img(), &card1, Some(2), 15.0, VIEW, &bubble_bg()).expect("within 2 px");
        assert!((l1.rect.y - 80).abs() <= 1, "{l1:?}");
    }

    #[test]
    fn a_bubble_lane_is_found_for_every_card_even_an_unknown_slot() {
        // an incoming bubble (slot 101, 2 rows): same rule as the reply card
        let (pic, mut card) = reply_pic(2, 16);
        card.slot = 101;
        let known = find_window_lane(&pic.img(), &card, Some(2), 16.0, VIEW, &bubble_bg()).expect("bubble lane");
        assert_eq!(known.rect.y, 82);
        // "Not received" (no TRANSLATE, rows unknown): right under the last visible row
        let unknown = find_window_lane(&pic.img(), &card, None, 16.0, VIEW, &bubble_bg()).expect("unknown slot");
        assert_eq!(unknown, known);
        // a lane below the view's bottom (the thread clips it) is left alone
        let short = Rect { x: 0, y: 0, w: 400, h: 90 };
        assert!(find_window_lane(&pic.img(), &card, Some(2), 16.0, short, &bubble_bg()).is_none());
    }

    #[test]
    fn a_tooltip_over_part_of_the_lane_is_never_painted() {
        let (mut pic, card) = reply_pic(2, 16);
        // a tooltip (dark blue with a grey border) over the right quarter of the lane and below
        let tip = Rect { x: 270, y: 70, w: 120, h: 80 };
        pic.fill(tip, [160, 160, 160]);
        pic.fill(Rect { x: tip.x + 1, y: tip.y + 1, w: tip.w - 2, h: tip.h - 2 }, [23, 23, 48]);
        let lane = find_window_lane(&pic.img(), &card, Some(2), 16.0, VIEW, &bubble_bg()).expect("most of the lane is clean");
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
        assert!(find_window_lane(&pic.img(), &card, Some(2), 16.0, VIEW, &bubble_bg()).is_none());
    }

    #[test]
    fn no_lane_no_paint() {
        // the card's rows end at the bubble's bottom: below them is the game world
        let (mut pic, card) = reply_pic(2, 16);
        pic.fill(Rect { x: 0, y: 82, w: 400, h: 218 }, [90, 120, 60]);
        for x in (0..400).step_by(3) {
            pic.fill(Rect { x, y: 82, w: 1, h: 218 }, [140, 90, 40]);
        }
        assert!(find_window_lane(&pic.img(), &card, Some(2), 16.0, VIEW, &bubble_bg()).is_none());
        // the lane outside the capture, or a row count that does not leave room for it
        let (pic, card) = reply_pic(2, 16);
        let short = Bgra { px: &pic.px[..(400 * 85 * 4) as usize], width: 400, height: 85, stride: 1600, origin_x: 0, origin_y: 0 };
        assert!(find_window_lane(&short, &card, Some(2), 16.0, VIEW, &bubble_bg()).is_none());
        assert!(find_window_lane(&pic.img(), &card, Some(1), 16.0, VIEW, &bubble_bg()).is_none(), "row 1 is visible: K cannot be 1");
        assert!(find_window_lane(&pic.img(), &card, Some(2), 4.0, VIEW, &bubble_bg()).is_none(), "rows too small for 2 + 2 edge rows");
    }

    // ------------------------------------------------------------------------------ chat lanes

    /// The chat ground at (x, y): a 30% black panel over a world that goes from dark green to
    /// brown across the view, with a little texture.
    fn chat_ground(x: i32, y: i32) -> [u8; 3] {
        let t = x as f32 / 399.0;
        let tex = ((x * 7 + y * 13) % 9) as f32 - 4.0;
        let world = [40.0 + 60.0 * t + tex, 70.0 - 10.0 * t + tex, 40.0 - 10.0 * t + tex];
        world.map(|v| (0.7 * v + 0.5) as u8)
    }

    /// An inline card of `k` rows (tag at the indent, plate after it) in a chat whose lines are
    /// `rp` px apart; under it, `below` (rp rows from y 50 + k rp).
    fn chat_pic(k: u8, rp: i32) -> (Pic, Card, CardBg) {
        let mut pic = Pic::new(400, 300, [0, 0, 0]);
        for y in 0..300 {
            for x in 0..400 {
                pic.set(x, y, chat_ground(x, y));
            }
        }
        draw_rows(&mut pic, k, rp);
        let card = card_at(7, 1, k, rp);
        let bg = backdrop::sample_card(&pic.img(), &card, VIEW, false);
        (pic, card, bg)
    }

    fn lane_y(k: u8, rp: i32) -> i32 {
        50 + i32::from(k) * rp
    }

    /// The addon's inline lane: the hint in gold from `x0`, then "[Download]" in the links' blue,
    /// both ending before the plate's end (x 340).
    fn hint(pic: &mut Pic, x0: i32, ly: i32, rp: i32) {
        pic.glyphs(x0, 260, ly, ly + rp, GOLD);
        pic.glyphs(266, 330, ly, ly + rp, LINK);
    }

    #[test]
    fn a_gold_hint_line_under_a_chat_card_is_covered() {
        for (k, rp) in [(1u8, 17), (2, 17), (3, 20)] {
            let (mut pic, card, bg) = chat_pic(k, rp);
            let ly = lane_y(k, rp);
            // the hint from the tag's x0 (after the row's indent, as the addon writes it), then the
            // link, glyphs touching the line's top and bottom rows, with their shadow
            hint(&mut pic, 42, ly, rp);
            let lane = find_chat_lane(&pic.img(), &card, Some(k), rp as f32, VIEW, &bg).expect("chat lane");
            assert_eq!(lane.kind, LaneKind::Chat);
            assert_eq!(rect4(&lane.rect), [40, ly, 300, rp], "k {k}");
            assert_eq!(lane.passing(), lane.cols.len(), "k {k}: every column passes");
            // the fill follows the ground: within a few levels at each column
            for (i, c) in lane.colors().iter().enumerate() {
                let x = lane.rect.x + i as i32;
                let g = chat_ground(x, ly + rp / 2);
                assert!(dist(*c, g) <= 8, "k {k} column {x}: fill {c:?} vs ground {g:?}");
            }
            // the hint from the plate's x (a blank tag column), or the link alone: found too
            let (mut pic2, card2, bg2) = chat_pic(k, rp);
            hint(&mut pic2, 92, ly, rp);
            let l2 = find_chat_lane(&pic2.img(), &card2, Some(k), rp as f32, VIEW, &bg2).expect("hint at the plate");
            assert_eq!(l2.passing(), l2.cols.len(), "k {k}");
            let (mut pic4, card4, bg4) = chat_pic(k, rp);
            pic4.glyphs(42, 110, ly, ly + rp, LINK);
            let l4 = find_chat_lane(&pic4.img(), &card4, Some(k), rp as f32, VIEW, &bg4).expect("the link alone");
            assert_eq!(l4.passing(), l4.cols.len(), "k {k}");
            // an empty lane (before the hint shows): found, painting it is harmless
            let (pic3, card3, bg3) = chat_pic(k, rp);
            assert!(find_chat_lane(&pic3.img(), &card3, Some(k), rp as f32, VIEW, &bg3).is_some(), "k {k}: empty lane");
        }
    }

    #[test]
    fn the_next_chat_line_or_the_glossary_row_is_never_taken_for_a_lane() {
        let (k, rp) = (2u8, 17);
        let ly = lane_y(k, rp);
        // the next message, from the chat's left edge: white, the party colour, a channel colour,
        // the guild green, the system yellow
        for rgb in [[255, 255, 255], [170, 170, 255], [255, 192, 192], [64, 255, 64], [255, 255, 0], [255, 128, 64]] {
            let (mut pic, card, bg) = chat_pic(k, rp);
            pic.glyphs(2, 300, ly, ly + rp, rgb);
            assert!(find_chat_lane(&pic.img(), &card, Some(k), rp as f32, VIEW, &bg).is_none(), "next line in {rgb:?}");
        }
        // the glossary row (pale gold e6cc80) and the name row (light blue 9fd4ff) start at the
        // indent
        for rgb in [[230, 204, 128], [159, 212, 255]] {
            let (mut pic, card, bg) = chat_pic(k, rp);
            pic.glyphs(16, 300, ly, ly + rp, rgb);
            assert!(find_chat_lane(&pic.img(), &card, Some(k), rp as f32, VIEW, &bg).is_none(), "row in {rgb:?}");
        }
        // gold is told from those colours over any ground
        for g in [[12, 12, 14], [60, 70, 50], [90, 80, 60]] {
            let over = |c: [u8; 3], a: f32| [0, 1, 2].map(|i| (f32::from(g[i]) + a * (f32::from(c[i]) - f32::from(g[i]))) as u8);
            for a in [0.4f32, 0.7, 1.0] {
                assert!(is_gold(over(GOLD, a), g), "gold {a} over {g:?}");
                assert!(is_link_blue(over(LINK, a), g), "link blue {a} over {g:?}");
                assert!(!is_link_blue(over(GOLD, a), g) && !is_gold(over(LINK, a), g));
                for c in [[255, 255, 255], [230, 204, 128], [255, 128, 64], [255, 192, 192], [64, 255, 64], [255, 255, 0], [170, 170, 255], [159, 212, 255]] {
                    assert!(!is_gold(over(c, a), g) && !is_link_blue(over(c, a), g), "{c:?} {a} over {g:?}");
                }
            }
        }
    }

    #[test]
    fn a_cover_over_a_chat_lane_is_never_painted() {
        let (k, rp) = (2u8, 17);
        let ly = lane_y(k, rp);
        // a tooltip whose top edge lies inside the lane, over its right part: those columns fail
        let (mut pic, card, bg) = chat_pic(k, rp);
        hint(&mut pic, 42, ly, rp);
        let tip = Rect { x: 260, y: ly + 3, w: 130, h: 80 };
        pic.fill(tip, [160, 160, 160]);
        pic.fill(Rect { x: tip.x + 1, y: tip.y + 1, w: tip.w - 2, h: tip.h - 2 }, [23, 23, 48]);
        let lane = find_chat_lane(&pic.img(), &card, Some(k), rp as f32, VIEW, &bg).expect("most of it is clean");
        for (i, ok) in lane.cols.iter().enumerate() {
            let x = lane.rect.x + i as i32;
            if x >= tip.x {
                assert!(!ok, "column {x} under the tooltip passes");
            } else if x < tip.x - RUN_PAD as i32 {
                // the cover edge's own run fails RUN_PAD columns more (rounded corners)
                assert!(ok, "clean column {x} fails");
            }
        }
        // a tooltip over the last row and the lane: the plate above is cut there (mask), the
        // columns fail even where the tooltip's body passes for ground
        let (mut pic, mut card, bg) = chat_pic(k, rp);
        hint(&mut pic, 42, ly, rp);
        let tip = Rect { x: 240, y: ly - 8, w: 150, h: 90 };
        pic.fill(tip, chat_ground(300, ly));
        let last = card.bands.last_mut().unwrap();
        let m = last.mask.as_mut().unwrap();
        for y in m.rect.y..m.rect.y + m.rect.h {
            for x in tip.x..m.rect.x + m.rect.w {
                if y >= tip.y {
                    m.bits[((y - m.rect.y) * m.rect.w + (x - m.rect.x)) as usize] = 0;
                }
            }
        }
        let lane = find_chat_lane(&pic.img(), &card, Some(k), rp as f32, VIEW, &bg).expect("left part");
        assert!(lane.cols.iter().enumerate().all(|(i, ok)| *ok == (lane.rect.x + (i as i32) < tip.x)), "{:?}", lane.cols);
        // most of the lane covered: left alone
        let (mut pic, card, bg) = chat_pic(k, rp);
        pic.fill(Rect { x: 60, y: ly + 2, w: 330, h: 60 }, [23, 23, 48]);
        assert!(find_chat_lane(&pic.img(), &card, Some(k), rp as f32, VIEW, &bg).is_none());
    }

    #[test]
    fn no_chat_lane_where_it_cannot_be() {
        let (k, rp) = (2u8, 17);
        let ly = lane_y(k, rp);
        let (mut pic, card, bg) = chat_pic(k, rp);
        hint(&mut pic, 42, ly, rp);
        // the lane below the chat's bottom edge (the view clips it)
        let clipped = Rect { x: 0, y: 0, w: 400, h: ly + rp - 4 };
        assert!(find_chat_lane(&pic.img(), &card, Some(k), rp as f32, clipped, &bg).is_none());
        // the request's last row is not visible (K = 3, rows 0 and 1 shown)
        assert!(find_chat_lane(&pic.img(), &card, Some(3), rp as f32, VIEW, &bg).is_none());
        // an unknown slot ("Not received"): right under the last visible row
        assert!(find_chat_lane(&pic.img(), &card, None, rp as f32, VIEW, &bg).is_some());
        // no-plate mode: no lane
        let mut np = card.clone();
        np.plate = false;
        assert!(find_chat_lane(&pic.img(), &np, Some(k), rp as f32, VIEW, &bg).is_none());
    }

    #[test]
    fn glyph_edges_and_shadows_are_told_from_covers() {
        let g = [33, 55, 30];
        // a gold edge over its own shadow, the shadow alone, a faint link edge
        assert!(is_glyph_mix([86, 78, 9], g, LANE_GOLD));
        assert!(is_glyph_mix([13, 22, 12], g, LANE_GOLD));
        assert!(is_glyph_mix([48, 77, 76], g, LANE_LINK));
        // a tooltip's border, a parchment frame: neither; a tooltip's navy body is no gold mix,
        // but it is near a mix of 17% link blue over black (residual 9): such a pixel joins lane
        // ink only within NEAR_INK px of a link glyph, so at most a cover's outer 2 columns
        for c in [[160, 160, 160], [200, 170, 120]] {
            for g in [[59, 44, 23], [12, 12, 14]] {
                assert!(!is_glyph_mix(c, g, LANE_GOLD) && !is_glyph_mix(c, g, LANE_LINK), "{c:?} over {g:?}");
            }
        }
        assert!(!is_glyph_mix([23, 23, 48], [59, 44, 23], LANE_GOLD) && !is_glyph_mix([23, 23, 48], [12, 12, 14], LANE_GOLD));
    }

    #[test]
    fn settled_lanes_keep_their_fill() {
        let (mut pic, card, bg) = chat_pic(2, 17);
        hint(&mut pic, 42, 84, 17);
        let a = find_chat_lane(&pic.img(), &card, Some(2), 17.0, VIEW, &bg).expect("lane");
        let mut b = a.clone();
        b.map_colors(|c| c.map(|v| v.saturating_add(3)));
        assert_ne!(a.hash_words(), b.hash_words());
        settle(&mut b, &a);
        assert_eq!(b, a, "3 levels: the previous fill stays");
        let mut c = a.clone();
        c.map_colors(|c| c.map(|v| v.saturating_add(9)));
        settle(&mut c, &a);
        assert_ne!(c, a, "9 levels: the new fill");
    }

    // ---------------------------------------------------------------- integration review (2026-10-19)

    #[test]
    fn no_window_lane_under_a_bubble_whose_last_row_is_hidden() {
        // a 4-row bubble cut by the thread's bottom (rows 0 and 1 shown): the addon draws no lane,
        // and below the thread lies the reply card (bubble fill and its draft line)
        let rp = 16;
        let mut pic = Pic::new(400, 300, [12, 12, 12]);
        pic.fill(Rect { x: 30, y: 40, w: 330, h: 42 }, BUBBLE);
        draw_rows(&mut pic, 2, rp);
        pic.fill(Rect { x: 0, y: 82, w: 400, h: 3 }, [90, 80, 60]);
        pic.fill(Rect { x: 0, y: 85, w: 400, h: 10 }, [22, 20, 18]);
        pic.fill(Rect { x: 30, y: 95, w: 330, h: 120 }, BUBBLE);
        pic.glyphs(104, 300, 50 + 4 * rp + 4, 50 + 5 * rp - 4, [255, 255, 255]);
        let card = card_at(101, 200, 2, rp);
        assert!(find_window_lane(&pic.img(), &card, Some(4), rp as f32, VIEW, &bubble_bg()).is_none(), "rows 2 and 3 hidden");
        // a tooltip over row 2 and the lane of a 3-row bubble: rows 0 and 1 shown
        let mut pic = Pic::new(400, 300, [90, 120, 60]);
        pic.fill(Rect { x: 30, y: 40, w: 330, h: 4 * rp + 20 }, BUBBLE);
        draw_rows(&mut pic, 3, rp);
        let tip = Rect { x: 25, y: 82, w: 370, h: 100 };
        pic.fill(tip, [160, 160, 160]);
        pic.fill(Rect { x: tip.x + 1, y: tip.y + 1, w: tip.w - 2, h: tip.h - 2 }, [23, 23, 48]);
        pic.glyphs(40, 330, 102, 111, [255, 255, 255]);
        let card = card_at(101, 200, 2, rp);
        assert!(find_window_lane(&pic.img(), &card, Some(3), rp as f32, VIEW, &bubble_bg()).is_none(), "row 2 hidden");
        // a slot the Helper does not know: the lane would be under row 1, on the tooltip's body,
        // which is not the bubble's fill
        assert!(find_window_lane(&pic.img(), &card, None, rp as f32, VIEW, &bubble_bg()).is_none(), "a tooltip's body is not the fill");
    }

    #[test]
    fn a_window_hint_reaching_the_lane_edges_still_passes() {
        // a small UI scale: the hint's glyphs and their shadow reach the lane's outer rows
        let rp = 13;
        let (mut pic, card) = reply_pic(2, rp);
        let ly = 50 + 2 * rp;
        pic.fill(Rect { x: 40, y: ly, w: 300, h: rp }, BUBBLE);
        pic.glyphs(92, 260, ly, ly + rp, GOLD);
        pic.glyphs(266, 330, ly, ly + rp, LINK);
        let lane = find_window_lane(&pic.img(), &card, Some(2), rp as f32, VIEW, &bubble_bg()).expect("lane");
        assert_eq!(lane.passing(), lane.cols.len());
        // a tooltip's grey border on the lane's bottom rows is no glyph
        pic.fill(Rect { x: 200, y: ly + rp - 2, w: 120, h: 30 }, [160, 160, 160]);
        let lane = find_window_lane(&pic.img(), &card, Some(2), rp as f32, VIEW, &bubble_bg()).expect("most of it");
        for (i, ok) in lane.cols.iter().enumerate() {
            let x = lane.rect.x + i as i32;
            if (200..320).contains(&x) {
                assert!(!ok, "column {x} under the border passes");
            }
        }
    }

    #[test]
    fn a_window_lane_is_the_cards_own_fill() {
        // a uniform panel over the whole lane row, clean edges: not the card's fill
        let (mut pic, card) = reply_pic(2, 16);
        pic.fill(Rect { x: 30, y: 82, w: 330, h: 16 }, [60, 52, 40]);
        assert!(find_window_lane(&pic.img(), &card, Some(2), 16.0, VIEW, &bubble_bg()).is_none());
        // within a few levels of the sample: the fill (a sample is a median, the fill too)
        let (mut pic, card) = reply_pic(2, 16);
        pic.fill(Rect { x: 30, y: 82, w: 330, h: 16 }, [40, 33, 27]);
        assert!(find_window_lane(&pic.img(), &card, Some(2), 16.0, VIEW, &bubble_bg()).is_some());
        // no sample at all: the fill is not checked (as before the review)
        let (pic, card) = reply_pic(2, 16);
        assert!(find_window_lane(&pic.img(), &card, Some(2), 16.0, VIEW, &CardBg::default()).is_some());
    }

    /// A chat ground with per-pixel texture of +-`amp` levels over a slow gradient.
    fn noisy(amp: i32) -> impl Fn(i32, i32) -> [u8; 3] {
        move |x: i32, y: i32| {
            let mut h = (x as u32).wrapping_mul(374_761_393) ^ (y as u32).wrapping_mul(668_265_263);
            h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
            let n = (h % (2 * amp as u32 + 1)) as i32 - amp;
            let base = [70 + x / 10, 80, 50];
            base.map(|b| (b + n).clamp(0, 255) as u8)
        }
    }

    /// A chat card of `k` rows over `ground`, its lane holding the addon's hint from the tag's x0.
    fn chat_pic_on(k: u8, rp: i32, ground: impl Fn(i32, i32) -> [u8; 3]) -> (Pic, Card, CardBg) {
        let mut pic = Pic::new(400, 300, [0, 0, 0]);
        for y in 0..300 {
            for x in 0..400 {
                pic.set(x, y, ground(x, y));
            }
        }
        draw_rows(&mut pic, k, rp);
        hint(&mut pic, 42, lane_y(k, rp), rp);
        let card = card_at(7, 1, k, rp);
        let bg = backdrop::sample_card(&pic.img(), &card, VIEW, false);
        (pic, card, bg)
    }

    /// Hint pixels (gold or link blue within 40 levels) of the lane band left visible: in a
    /// column the lane does not paint, or all of them when no lane is found.
    fn hint_left(pic: &Pic, y0: i32, h: i32, l: Option<&Lane>) -> usize {
        let mut n = 0;
        for x in 40..340 {
            if l.is_some_and(|l| x >= l.rect.x && x < l.rect.x + l.rect.w && l.cols[(x - l.rect.x) as usize]) {
                continue;
            }
            for y in y0..y0 + h {
                let p = pic.get(x, y);
                if [GOLD, LINK].iter().any(|c| dist(p, *c) <= 40) {
                    n += 1;
                }
            }
        }
        n
    }

    #[test]
    fn a_chat_lane_over_a_textured_world_is_covered() {
        let (k, rp) = (2u8, 17);
        let ly = lane_y(k, rp);
        for amp in [6, 12, 24, 30] {
            let (pic, card, bg) = chat_pic_on(k, rp, noisy(amp));
            let lane = find_chat_lane(&pic.img(), &card, Some(k), rp as f32, VIEW, &bg);
            let left = hint_left(&pic, ly, rp, lane.as_ref());
            assert!(lane.is_some(), "texture +-{amp}: no lane");
            assert_eq!(left, 0, "texture +-{amp}: {left} hint pixels left visible ({:?} of 300 columns)", lane.map(|l| l.passing()));
        }
        // +-40 (80 levels from pixel to pixel) is past the largest tolerance: most of it is still
        // covered, a few columns are left alone
        let (pic, card, bg) = chat_pic_on(k, rp, noisy(40));
        let lane = find_chat_lane(&pic.img(), &card, Some(k), rp as f32, VIEW, &bg).expect("texture +-40");
        assert!(lane.passing() >= 270, "{} of 300 columns", lane.passing());
    }

    #[test]
    fn a_chat_lane_over_a_feature_the_samples_miss_is_covered() {
        // a smooth chat ground with one darker (or lighter) feature behind the lane, 40 px wide,
        // that neither background sample sees (a tree trunk, a doorway, a lit wall)
        let (k, rp) = (2u8, 17);
        let ly = lane_y(k, rp);
        for delta in [-45i32, -30, -20, 20, 30] {
            let g = move |x: i32, y: i32| {
                let base = chat_ground(x, y);
                if (150..190).contains(&x) {
                    base.map(|v| (i32::from(v) + delta).clamp(0, 255) as u8)
                } else {
                    base
                }
            };
            let (pic, card, bg) = chat_pic_on(k, rp, g);
            let lane = find_chat_lane(&pic.img(), &card, Some(k), rp as f32, VIEW, &bg);
            let left = hint_left(&pic, ly, rp, lane.as_ref());
            assert!(lane.is_some(), "feature {delta}: no lane");
            assert_eq!(left, 0, "feature {delta}: {left} hint pixels left visible");
        }
    }

    #[test]
    fn a_cover_edge_inside_a_chat_lane_is_never_painted() {
        // a tooltip whose top border lies in the lane's middle rows, over a dark chat whose ground
        // is close to the tooltip's body: its columns fail (border, body and text kept)
        let (k, rp) = (2u8, 17);
        let ly = lane_y(k, rp);
        let dark = |x: i32, y: i32| {
            let t = ((x * 7 + y * 13) % 5) as u8;
            [14 + t, 14 + t, 16 + t]
        };
        for (top, border) in [(6, [128, 128, 128]), (9, [128, 128, 128]), (12, [90, 70, 40])] {
            let (mut pic, card, _) = chat_pic_on(k, rp, dark);
            let tip = Rect { x: 240, y: ly + top, w: 150, h: 80 };
            pic.fill(tip, border);
            pic.fill(Rect { x: tip.x + 1, y: tip.y + 1, w: tip.w - 2, h: tip.h - 2 }, [24, 24, 26]);
            pic.glyphs(252, 380, tip.y + 12, tip.y + 24, [255, 255, 255]);
            let bg = backdrop::sample_card(&pic.img(), &card, VIEW, false);
            let lane = find_chat_lane(&pic.img(), &card, Some(k), rp as f32, VIEW, &bg).expect("the left part is clean");
            for (i, ok) in lane.cols.iter().enumerate() {
                let x = lane.rect.x + i as i32;
                if x >= tip.x - 1 {
                    assert!(!ok, "top {top}: column {x} over the tooltip passes");
                } else if x < tip.x - 1 - RUN_PAD as i32 {
                    assert!(ok, "top {top}: clean column {x} fails");
                }
            }
        }
    }

    #[test]
    fn the_addons_print_under_the_test_rows_is_not_a_lane() {
        // `/tpr test rows`: 3 rows and no lane; the next line is the addon's own print, whose
        // "TranslateProbe:" prefix (66bbff, the link blue) starts at the chat's left edge
        let rp = 17;
        let mut pic = Pic::new(400, 300, [0, 0, 0]);
        for y in 0..300 {
            for x in 0..400 {
                pic.set(x, y, chat_ground(x, y));
            }
        }
        draw_rows(&mut pic, 3, rp);
        let ly = lane_y(3, rp);
        pic.glyphs(2, 104, ly, ly + rp, LINK);
        pic.glyphs(108, 398, ly, ly + rp, [255, 255, 255]);
        let card = card_at(127, 1, 3, rp);
        let bg = backdrop::sample_card(&pic.img(), &card, VIEW, false);
        assert!(find_chat_lane(&pic.img(), &card, None, rp as f32, VIEW, &bg).is_none());
    }

    #[test]
    fn a_last_row_cut_by_the_top_of_the_chat_still_finds_its_lane() {
        // rows 0 and 1 at y 50 and 67 (17 px apart); the chat's top at y 70 cuts row 1 (3 px of
        // it are above), row 0 is gone: the band seen is row 1's visible part
        let (k, rp) = (2u8, 17);
        let (pic, mut card, _) = chat_pic_on(k, rp, chat_ground);
        let view = Rect { x: 0, y: 70, w: 400, h: 230 };
        card.bands.remove(0);
        let b = &mut card.bands[0];
        b.text_rect = Rect { x: b.text_rect.x, y: 70, w: b.text_rect.w, h: 14 };
        b.tag_rect = Rect { x: b.tag_rect.x, y: 70, w: b.tag_rect.w, h: 14 };
        let m = b.mask.as_mut().unwrap();
        *m = Mask { rect: Rect { x: m.rect.x, y: 70, w: m.rect.w, h: 14 }, bits: vec![255; (m.rect.w * 14) as usize] };
        let bg = backdrop::sample_card(&pic.img(), &card, view, false);
        let lane = find_chat_lane(&pic.img(), &card, Some(k), rp as f32, view, &bg).expect("lane under a cut row");
        assert_eq!(lane.rect.y, lane_y(k, rp), "measured from the row's bottom");
        assert_eq!(lane.passing(), lane.cols.len());
    }

    /// The memo of the lane of `chat_pic_on(2, rp, ..)` (found whole under its card), as if the
    /// chat had moved `y - lane_y(2, rp)` px since.
    fn memo_at(y: i32, rp: i32) -> ChatLaneMemo {
        let (pic, card, bg) = chat_pic_on(2, rp, chat_ground);
        let (lane, ink) = find_chat_lane_ink(&pic.img(), &card, Some(2), rp as f32, VIEW, &bg).expect("lane under its card");
        let mut m = ChatLaneMemo::of(&card, &lane, ink, rp as f32, VIEW, 0).expect("memo");
        m.rect.y = y;
        m
    }

    #[test]
    fn an_orphan_lane_at_the_top_of_the_chat_is_followed() {
        let (k, rp) = (2u8, 17);
        let ly = lane_y(k, rp);
        let (pic, _, _) = chat_pic_on(k, rp, chat_ground);
        // the chat moved up one line since the lane was found (memo one line lower); its rows are
        // now above the chat's top (y = ly): the lane is the top line
        let view = Rect { x: 0, y: ly, w: 400, h: 300 - ly };
        let (lane, whole) = find_orphan_chat_lane(&pic.img(), &memo_at(ly + rp, rp), view).expect("orphan lane");
        assert_eq!(rect4(&lane.rect), [40, ly, 300, rp]);
        assert_eq!(rect4(&whole), [40, ly, 300, rp]);
        assert_eq!(lane.passing(), lane.cols.len());
        assert_eq!(hint_left(&pic, ly, rp, Some(&lane)), 0);
        // found again where it is (the memo follows it), and cut by the top: its visible rows
        assert!(find_orphan_chat_lane(&pic.img(), &memo_at(ly, rp), view).is_some());
        let cut = Rect { x: 0, y: ly + 6, w: 400, h: 300 - ly - 6 };
        let (lane, whole) = find_orphan_chat_lane(&pic.img(), &memo_at(ly + 2 * rp, rp), cut).expect("cut by the top");
        assert_eq!(rect4(&lane.rect), [40, ly + 6, 300, rp - 6]);
        assert_eq!(whole.y, ly);
        assert_eq!(lane.passing(), lane.cols.len());
        // over a textured world too (the samples are taken on the lane's own line)
        let (pic, _, _) = chat_pic_on(k, rp, noisy(12));
        assert!(find_orphan_chat_lane(&pic.img(), &memo_at(ly + rp, rp), view).is_some(), "texture +-12");
    }

    #[test]
    fn no_orphan_lane_where_its_card_could_be_seen() {
        let (k, rp) = (2u8, 17);
        let ly = lane_y(k, rp);
        let (pic, _, _) = chat_pic_on(k, rp, chat_ground);
        // the row above the lane lies inside the view: a missing card there is covered, not gone
        assert!(find_orphan_chat_lane(&pic.img(), &memo_at(ly, rp), VIEW).is_none());
        // the top line is another message (from the chat's left edge), or the glossary row
        for (x0, rgb) in [(2, [255, 255, 255]), (2, [255, 192, 192]), (16, [230, 204, 128])] {
            let mut pic = Pic::new(400, 300, [0, 0, 0]);
            for y in 0..300 {
                for x in 0..400 {
                    pic.set(x, y, chat_ground(x, y));
                }
            }
            pic.glyphs(x0, 330, ly, ly + rp, rgb);
            let view = Rect { x: 0, y: ly, w: 400, h: 300 - ly };
            assert!(find_orphan_chat_lane(&pic.img(), &memo_at(ly + rp, rp), view).is_none(), "{rgb:?}");
        }
        // the chat moved up by more lines than a lane can follow (or another line is there)
        let view = Rect { x: 0, y: ly, w: 400, h: 300 - ly };
        assert!(find_orphan_chat_lane(&pic.img(), &memo_at(ly + (ORPHAN_MAX_LINES + 1) * rp, rp), view).is_none());
        // the lane went above the top: where it was, the next message's top rows (white, with
        // colour fringes) are no lane, nor a blank line
        let (mut pic, _, _) = chat_pic_on(k, rp, chat_ground);
        pic.glyphs(2, 330, ly + rp, ly + 2 * rp, [255, 255, 255]);
        for x in (6..330).step_by(9) {
            pic.set(x, ly + rp + 3, [255, 200, 40]);
            pic.set(x + 1, ly + rp + 3, [110, 190, 255]);
        }
        let view = Rect { x: 0, y: ly + rp + 2, w: 400, h: 300 - ly - rp - 2 };
        assert!(find_orphan_chat_lane(&pic.img(), &memo_at(ly + rp, rp), view).is_none(), "the next line");
        let (pic, _, _) = chat_pic_on(k, rp, chat_ground);
        assert!(find_orphan_chat_lane(&pic.img(), &memo_at(ly + rp, rp), view).is_none(), "a blank line");
    }

    #[test]
    fn an_orphan_lane_cut_to_its_last_rows_is_painted_where_its_ink_is() {
        // the chat moved up once more: only the lane's last 4 rows show; the candidate taken is
        // the one over its ink, not the blank line under it
        let (k, rp) = (2u8, 17);
        let ly = lane_y(k, rp);
        let (pic, _, _) = chat_pic_on(k, rp, chat_ground);
        let view = Rect { x: 0, y: ly + rp - 4, w: 400, h: 300 - ly - rp + 4 };
        let (lane, whole) = find_orphan_chat_lane(&pic.img(), &memo_at(ly + rp, rp), view).expect("its last rows");
        assert_eq!(rect4(&lane.rect), [40, ly + rp - 4, 300, 4]);
        assert_eq!(whole.y, ly);
        assert_eq!(hint_left(&pic, ly + rp - 4, 4, Some(&lane)), 0);
    }
}
