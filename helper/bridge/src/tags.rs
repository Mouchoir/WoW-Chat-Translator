//! Row tags: how the helper finds the reserved chat rows the addon printed.
//!
//! A tag is 8 inline blocks in one chat row: B0 magenta (255,0,255) and B1 green (0,255,0) form the
//! sync pair, then B2..B7 are 6 base-6 digits (0 black, 1 red, 2 blue, 3 yellow, 4 cyan, 5 white)
//! of `v = payload10 << 5 | crc5`, most significant first, with
//! `payload10 = slot << 3 | row << 1 | sessBit`. The plate (RGB 32,40,64) follows B7.
//!
//! Scan (normative in docs/BRIDGE.md):
//! 1. rows y step 2 over the area; B0 must start in the left `min(w, max(200, 0.35 w))` px;
//! 2. sync pair: a pixel p with a pixel q at most 4 px to its right such that p is above q by 40
//!    or more on red and blue and below it by 40 or more on green (B0 against B1: this holds for a
//!    faded tag over any ground, where "magenta-like" does not). Every candidate in the row is
//!    tried, left to right; B0 and B1 are bounded by colour similarity (a magenta-like ground next
//!    to B0 does not stretch it); B1 must start within `max(2, bw/2)` px (+2 for blended edges);
//! 3. vertical extent of B0 and B1 (similarity walks), cut where the tag's span changes (two rows
//!    of a card can touch), h >= 4;
//! 4. block edges measured to a fraction of a pixel on the row profile averaged over the tag's
//!    rows; the pitch searched +-1.5 px around the sync pair's for the most uniform block
//!    interiors, each range of equally good pitches then fitted to the blocks' full extents (B0's
//!    edge free by +-0.5 px; the block width free by +-0.5 px too when no range decodes without it:
//!    hard-edged blocks under 5 px alternate between two widths); each block sampled in a box of
//!    half-size `(max(1, bw/4), max(1, h/4))` inside its fully covered pixels: per-channel
//!    max-min <= 28 (uniformity); each channel of a data block near the sync pair's low or high
//!    reference (alphabet fit); no third block colour between two blocks; the tag ends after B7;
//!    digits from the mid thresholds; magenta or green digits, v >= 32768 or a bad CRC-5 reject
//!    the tag; every range that decodes must agree;
//! 5. the scan continues right of the tag and skips the rows it covers;
//! 6. alpha = mean over channels of (hi - lo) / (white - black) of the last strip calibration.

use crate::image;

/// Slot used by `/tpr test rows`; it never carries a TRANSLATE.
pub const TEST_SLOT: u8 = 127;
/// Colour of the plate texture drawn right of each tag.
pub const PLATE_RGB: [u8; 3] = [32, 40, 64];

/// One tag found in a capture. Coordinates are client px; `x0` is the left edge of B0 and
/// `x1 = x0 + 7 * pitch + block` the right edge of B7.
#[derive(Clone, Debug)]
pub struct TagHit {
    pub slot: u8,
    pub row: u8,
    pub sess_bit: u8,
    pub x0: i32,
    pub x1: i32,
    pub y_top: i32,
    pub y_bot: i32,
    pub block_px: f32,
    pub pitch_px: f32,
    /// Observed contrast of the sync pair relative to the strip calibration (0..1, chat fade).
    pub alpha: f32,
}

const SYNC: [[u8; 3]; 2] = [[255, 0, 255], [0, 255, 0]];
const DIGITS: [[u8; 3]; 6] = [[0, 0, 0], [255, 0, 0], [0, 0, 255], [255, 255, 0], [0, 255, 255], [255, 255, 255]];

/// Smallest channel difference between the sync blocks (B0 above B1 on red and blue, below on green).
pub const LIKE_MIN: f32 = 40.0;
/// B1 is looked for within this many px right of a B0 pixel (blended edges plus a 2 px gap).
const SYNC_REACH: i32 = 4;
/// Block width range (px).
pub const BLOCK_MIN: f32 = 4.0;
pub const BLOCK_MAX: f32 = 40.0;
/// Uniformity: largest per-channel max-min inside a block's sample box.
pub const UNIFORM_MAX: f32 = 28.0;
/// Colour similarity used to bound a block (max channel difference from its interior colour).
const SIMILAR: f32 = 24.0;
/// Alphabet fit: each channel of a data block within this fraction of (hi - lo) of lo or hi.
pub const FIT: f32 = 0.25;
/// Tolerance on the measured block width (sub-pixel edge noise).
const WIDTH_SLACK: f32 = 0.5;
/// A block's interior: whole pixels at least this far inside its measured edges.
const INNER: f32 = 0.25;
/// Pitch search around the sync pair's measurement: +-1.5 px in 0.05 px steps.
const PITCH_SEARCH: f32 = 1.5;
const PITCH_STEP: f32 = 0.05;
/// Pitch fit: scores within this of the best (sum of squared deviations) count as uniform.
const PITCH_TOL: f64 = 400.0;
/// Second-stage pitch filter: ranges whose full-extent fit (mean squared deviation) is above
/// best * FULL_RATIO + FULL_ABS are not read.
const FULL_RATIO: f64 = 1.5;
const FULL_ABS: f64 = 30.0;
/// Largest residual of a pixel between two block interiors explained as a two-colour blend.
const BETWEEN_MAX: f32 = 24.0;
/// The end-of-tag check runs only when B7 differs this much from the plate and the background.
const END_MIN: f32 = 6.0;

type Rgb = [f32; 3];

/// `v.floor() as i32` without a libm call (the pitch search runs it thousands of times per tag).
fn floor_i(v: f32) -> i32 {
    let t = v as i32;
    if (t as f32) > v {
        t - 1
    } else {
        t
    }
}

/// `v.ceil() as i32` without a libm call.
fn ceil_i(v: f32) -> i32 {
    let t = v as i32;
    if (t as f32) < v {
        t + 1
    } else {
        t
    }
}

fn get(img: &image::Bgra, x: i32, y: i32) -> Option<Rgb> {
    img.rgb(x, y).map(|p| [f32::from(p[0]), f32::from(p[1]), f32::from(p[2])])
}

fn dist(a: Rgb, b: Rgb) -> f32 {
    (a[0] - b[0]).abs().max((a[1] - b[1]).abs()).max((a[2] - b[2]).abs())
}

fn mean(v: &[Rgb]) -> Rgb {
    let n = v.len().max(1) as f32;
    let mut s = [0f32; 3];
    for p in v {
        for c in 0..3 {
            s[c] += p[c];
        }
    }
    [s[0] / n, s[1] / n, s[2] / n]
}

/// Least-squares fraction f of `a` in `p = f a + (1 - f) b` (clamped 0..1) and the residual.
fn blend(p: Rgb, a: Rgb, b: Rgb) -> (f32, f32) {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let n2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
    if n2 < 1.0 {
        return (0.5, dist(p, a)); // a and b are the same colour
    }
    let f = (((p[0] - b[0]) * d[0] + (p[1] - b[1]) * d[1] + (p[2] - b[2]) * d[2]) / n2).clamp(0.0, 1.0);
    let q = [b[0] + f * d[0], b[1] + f * d[1], b[2] + f * d[2]];
    (f, dist(p, q))
}

/// The sync pair's colour relation, independent of the background behind a faded tag: B0 is
/// above B1 on red and blue and below it on green. Returns the smallest of the three margins.
fn sync_margin(b0: Rgb, b1: Rgb) -> f32 {
    (b0[0] - b1[0]).min(b1[1] - b0[1]).min(b0[2] - b1[2])
}

/// True when every channel of `p` sits near the sync pair's low or high reference (a block colour).
fn alphabet_like(p: Rgb, lo: [f32; 3], hi: [f32; 3]) -> bool {
    (0..3).all(|c| {
        let t = (p[c] - lo[c]) / (hi[c] - lo[c]);
        t <= FIT || t >= 1.0 - FIT
    })
}

/// Residual of `p` explained as a mix of three colours (non-negative weights summing to 1):
/// a pixel at a block boundary can hold both blocks and a sub-pixel gap.
fn blend3(p: Rgb, a: Rgb, b: Rgb, c: Rgb) -> f32 {
    let edges = blend(p, a, b).1.min(blend(p, a, c).1).min(blend(p, b, c).1);
    let f = |x: Rgb, y: Rgb| [f64::from(x[0] - y[0]), f64::from(x[1] - y[1]), f64::from(x[2] - y[2])];
    let (u, v, w) = (f(a, c), f(b, c), f(p, c));
    let dot = |x: [f64; 3], y: [f64; 3]| x[0] * y[0] + x[1] * y[1] + x[2] * y[2];
    let (uu, uv, vv, uw, vw) = (dot(u, u), dot(u, v), dot(v, v), dot(u, w), dot(v, w));
    let det = uu * vv - uv * uv;
    // nearly collinear colours (two blocks of the same digit): the edge blends say it all
    if det <= 1e-4 * uu * vv || det <= 1e-9 {
        return edges;
    }
    let al = (uw * vv - vw * uv) / det;
    let be = (vw * uu - uw * uv) / det;
    if al >= 0.0 && be >= 0.0 && al + be <= 1.0 {
        let q = [c[0] as f64 + al * u[0] + be * v[0], c[1] as f64 + al * u[1] + be * v[1], c[2] as f64 + al * u[2] + be * v[2]];
        let r = (0..3).map(|k| (f64::from(p[k]) - q[k]).abs()).fold(0.0, f64::max) as f32;
        return r.min(edges);
    }
    edges
}

/// Scans `area` for tags and appends them to `out`. `white_black` is the latest strip
/// calibration (white, black) per channel; None means 255 / 0.
pub fn scan(img: &image::Bgra, area: image::Rect, white_black: Option<([f32; 3], [f32; 3])>, out: &mut Vec<TagHit>) {
    let x_lo = area.x.max(img.origin_x);
    let y_lo = area.y.max(img.origin_y);
    let x_hi = (area.x.saturating_add(area.w)).min(img.origin_x + img.width as i32);
    let y_hi = (area.y.saturating_add(area.h)).min(img.origin_y + img.height as i32);
    if x_hi <= x_lo || y_hi <= y_lo || area.w <= 0 {
        return;
    }
    let reach = area.w.min(200.max((0.35 * area.w as f32) as i32));
    let xs_end = x_hi.min(area.x.saturating_add(reach));
    let first = out.len();
    let lim = (x_lo, x_hi, y_lo, y_hi);
    let mut refused: Vec<Geometry> = Vec::new();
    let stride = img.stride as usize;
    let like_min = LIKE_MIN as i32;
    let mut y = y_lo;
    while y < y_hi {
        // the row's bytes (B, G, R, A from the image's left edge): the candidate test runs on
        // every pixel of the area, so it reads them directly
        let row = &img.px[(y - img.origin_y) as usize * stride..];
        let at = |x: i32| -> (i32, i32, i32) {
            let i = (x - img.origin_x) as usize * 4;
            (i32::from(row[i + 2]), i32::from(row[i + 1]), i32::from(row[i]))
        };
        let mut x = x_lo;
        while x < xs_end {
            let (pr, pg, pb) = at(x);
            // a B1 candidate within SYNC_REACH px to the right (the strongest one): B0 is above it
            // on red and blue by LIKE_MIN or more, so B0 itself is at least LIKE_MIN there
            let mut cand = None;
            if pr >= like_min && pb >= like_min {
                let mut best = like_min;
                for d in 1..=SYNC_REACH {
                    // B1 must lie inside the scan area: a narrower area inside a wider capture
                    // would otherwise hand read_tag a B1 past the profile's end
                    if x + d >= x_hi {
                        break;
                    }
                    let (qr, qg, qb) = at(x + d);
                    let m = (pr - qr).min(qg - pg).min(pb - qb);
                    if m >= best {
                        best = m;
                        cand = Some(x + d);
                    }
                }
            }
            let Some(xb1) = cand else {
                x += 1;
                continue;
            };
            let inside = out[first..]
                .iter()
                .any(|h| y >= h.y_top && y <= h.y_bot && x >= h.x0 - 1 && x < h.x0 + (8.0 * h.pitch_px).ceil() as i32);
            if inside {
                x += 1;
                continue;
            }
            let read = match interior2(img, x, y, (x_lo, x_hi)) {
                Some(b0) => read_tag(img, b0, xb1, y, lim, white_black, &refused),
                None => Err(("outside", None)),
            };
            match read {
                Ok(h) => {
                    x = (x + 1).max(h.x0 + (8.0 * h.pitch_px).ceil() as i32);
                    let same = |o: &TagHit| (o.slot, o.row, o.sess_bit) == (h.slot, h.row, h.sess_bit);
                    let near = |o: &TagHit, gap: i32| {
                        (o.x0 - h.x0).abs() <= 2 && h.y_top <= o.y_bot + gap && h.y_bot + gap >= o.y_top
                    };
                    if let Some(o) = out[first..].iter_mut().find(|o| same(o) && near(o, 2)) {
                        // the same tag seen again below a row where the vertical walk stopped
                        o.y_top = o.y_top.min(h.y_top);
                        o.y_bot = o.y_bot.max(h.y_bot);
                    } else if !out[first..].iter().any(|o| near(o, 0)) {
                        // (a different reading overlapping an earlier tag is dropped)
                        out.push(h);
                    }
                }
                Err((_, geom)) => {
                    // resume at the next pixel: this one may have been a blended edge of B0
                    if let Some(g) = geom {
                        refused.push(g);
                    }
                    x += 1;
                }
            }
        }
        y += 2;
    }
}

/// Interior of a block around `x` on row `y`: the contiguous pixels within SIMILAR of `refc`.
/// Returns (first, last).
fn interior(img: &image::Bgra, x: i32, y: i32, refc: Rgb, lim: (i32, i32)) -> (i32, i32) {
    let ok = |xx: i32| get(img, xx, y).is_some_and(|p| dist(p, refc) <= SIMILAR);
    let (mut a, mut b) = (x, x);
    while a - 1 >= lim.0 && ok(a - 1) {
        a -= 1;
    }
    while b + 1 < lim.1 && ok(b + 1) {
        b += 1;
    }
    (a, b)
}

/// Interior around `x`, re-walked from the interior's mean colour (more robust than one pixel).
fn interior2(img: &image::Bgra, x: i32, y: i32, lim: (i32, i32)) -> Option<(i32, i32, Rgb)> {
    let c = get(img, x, y)?;
    let (a, b) = interior(img, x, y, c, lim);
    let c = mean(&(a..=b).filter_map(|xx| get(img, xx, y)).collect::<Vec<_>>());
    let (a, b) = interior(img, (a + b) / 2, y, c, lim);
    Some((a, b, c))
}

/// Vertical extent of a block at column `x` from row `y`: rows within SIMILAR of `refc`.
fn vertical(img: &image::Bgra, x: i32, y: i32, refc: Rgb, lim: (i32, i32)) -> (i32, i32) {
    let ok = |yy: i32| get(img, x, yy).is_some_and(|p| dist(p, refc) <= SIMILAR);
    let (mut t, mut b) = (y, y);
    while t - 1 >= lim.0 && ok(t - 1) {
        t -= 1;
    }
    while b + 1 < lim.1 && ok(b + 1) {
        b += 1;
    }
    (t, b)
}

/// Why a candidate sync pair was not read as a tag (diagnostics and counters only).
pub type Reject = &'static str;

/// The measured geometry of a candidate (both sync interiors and the rows): the same geometry
/// always gives the same verdict, so a refused one is not read again on the next rows.
type Geometry = (i32, i32, i32, i32, i32, i32);

/// Rows around `y` (within `bound`) whose pixels over [x0, x1) all match row `y`.
fn same_rows(img: &image::Bgra, y: i32, x0: i32, x1: i32, bound: (i32, i32)) -> (i32, i32) {
    let same = |r: i32| {
        (x0..x1).all(|x| match (get(img, x, r), get(img, x, y)) {
            (Some(p), Some(q)) => dist(p, q) <= SIMILAR,
            _ => false,
        })
    };
    let (mut t, mut b) = (y, y);
    while t - 1 >= bound.0 && same(t - 1) {
        t -= 1;
    }
    while b + 1 <= bound.1 && same(b + 1) {
        b += 1;
    }
    (t, b)
}

/// Tries a tag whose B0 contains (xb0, y) and B1 contains (xb1, y).
fn try_tag(
    img: &image::Bgra,
    xb0: i32,
    xb1: i32,
    y: i32,
    lim: (i32, i32, i32, i32),
    white_black: Option<([f32; 3], [f32; 3])>,
) -> Result<TagHit, (Reject, Option<Geometry>)> {
    let Some((a0, b0, c0)) = interior2(img, xb0, y, (lim.0, lim.1)) else { return Err(("outside", None)) };
    read_tag(img, (a0, b0, c0), xb1, y, lim, white_black, &[])
}

/// Diagnoses the candidate whose B0 contains client pixel (x, y): the tag, or why it was
/// refused. For `--diag` tooling and tests.
pub fn diagnose(
    img: &image::Bgra,
    area: image::Rect,
    white_black: Option<([f32; 3], [f32; 3])>,
    x: i32,
    y: i32,
) -> Result<TagHit, Reject> {
    let lim = (
        area.x.max(img.origin_x),
        (area.x + area.w).min(img.origin_x + img.width as i32),
        area.y.max(img.origin_y),
        (area.y + area.h).min(img.origin_y + img.height as i32),
    );
    let p = get(img, x, y).ok_or("outside")?;
    let xb1 = (1..=SYNC_REACH)
        .take_while(|d| x + d < lim.1)
        .filter_map(|d| get(img, x + d, y).map(|q| (sync_margin(p, q), x + d)))
        .filter(|m| m.0 >= LIKE_MIN)
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .ok_or("no sync pair")?
        .1;
    try_tag(img, x, xb1, y, lim, white_black).map_err(|e| e.0)
}

fn read_tag(
    img: &image::Bgra,
    b0i: (i32, i32, Rgb),
    xb1: i32,
    y: i32,
    lim: (i32, i32, i32, i32),
    white_black: Option<([f32; 3], [f32; 3])>,
    refused: &[Geometry],
) -> Result<TagHit, (Reject, Option<Geometry>)> {
    let (x_lo, x_hi, y_lo, y_hi) = lim;
    let (a0, b0, c0) = b0i;
    let wi0 = b0 - a0 + 1;
    if wi0 < 2 || (wi0 as f32) > BLOCK_MAX {
        return Err(("B0 width", None));
    }
    // B1 must start within max(2, bw/2) px after B0 (plus the blended edge pixels)
    let (a1, b1, c1) = interior2(img, xb1, y, (x_lo, x_hi)).ok_or(("outside", None))?;
    let bw_est = wi0 as f32 + 1.0;
    if a1 <= b0 || a1 - b0 - 1 > 2.max((bw_est / 2.0) as i32) + 2 || sync_margin(c0, c1) < LIKE_MIN {
        return Err(("B1 position", None));
    }
    // 3. vertical extent of both sync blocks
    let (t0, d0) = vertical(img, (a0 + b0) / 2, y, c0, (y_lo, y_hi));
    let (t1, d1) = vertical(img, (a1 + b1) / 2, y, c1, (y_lo, y_hi));
    // consecutive rows of a card can touch: stop where any pixel of the tag's span changes
    // (two rows always differ in at least one data block)
    let span_end = (a0 + 8 * (a1 - a0 + 1)).min(x_hi);
    let (y_top, y_bot) = same_rows(img, y, a0, span_end, (t0.max(t1), d0.min(d1)));
    let h = y_bot - y_top + 1;
    if h < 4 {
        return Err(("height", None));
    }
    // from here on the verdict depends only on this geometry
    let geom = (a0, b0, a1, b1, y_top, y_bot);
    if refused.contains(&geom) {
        return Err(("seen", None));
    }
    let fail = |r: Reject| -> Result<TagHit, (Reject, Option<Geometry>)> { Err((r, Some(geom))) };
    // 4. row profile averaged over the tag's rows, from 3 px left of B0 to past B7
    let rough_pitch = (a1 - a0) as f32;
    let px0 = (a0 - 3).max(x_lo);
    let px1 = ((a0 as f32 + 8.0 * (rough_pitch + 2.5) + 6.0) as i32).min(x_hi);
    if px1 <= px0 {
        return fail("outside");
    }
    let n = (px1 - px0) as usize;
    let mut prof: Vec<Rgb> = vec![[0f32; 3]; n];
    for yy in y_top..=y_bot {
        for (i, acc) in prof.iter_mut().enumerate() {
            let Some(p) = get(img, px0 + i as i32, yy) else { return fail("outside") };
            for c in 0..3 {
                acc[c] += p[c];
            }
        }
    }
    let hf = h as f32;
    for acc in &mut prof {
        for v in acc.iter_mut() {
            *v /= hf;
        }
    }
    // prefix sums of the profile and its squares (block variances in O(1))
    let mut s1 = vec![[0f64; 3]; n + 1];
    let mut s2 = vec![[0f64; 3]; n + 1];
    for (i, p) in prof.iter().enumerate() {
        for c in 0..3 {
            s1[i + 1][c] = s1[i][c] + f64::from(p[c]);
            s2[i + 1][c] = s2[i][c] + f64::from(p[c]) * f64::from(p[c]);
        }
    }
    let at = |x: i32| -> Option<Rgb> {
        if x < px0 || x >= px1 {
            None
        } else {
            Some(prof[(x - px0) as usize])
        }
    };
    let span_mean = |a: i32, b: i32| -> Rgb {
        let i = (a.max(px0) - px0) as usize;
        let j = (((b + 1).min(px1) - px0) as usize).max(i + 1).min(n);
        let k = (j - i).max(1) as f64;
        let m = |c: usize| ((s1[j][c] - s1[i][c]) / k) as f32;
        [m(0), m(1), m(2)]
    };
    let pc0 = span_mean(a0, b0);
    let pc1 = span_mean(a1, b1);
    // background left of B0 (the indent); fall back to the nearest pixel
    let bg = at(a0 - 2).or_else(|| at(a0 - 1)).unwrap_or(pc0);
    // sub-pixel edges from two-colour blends: left of B0, right of B0 (towards B1 or the gap),
    // left of B1, right of B1 (towards B2 or the gap)
    let e_l0 = match at(a0 - 1) {
        Some(p) => a0 as f32 - blend(p, pc0, bg).0,
        None => a0 as f32,
    };
    let e_r0 = match at(b0 + 1) {
        Some(p) => {
            let (f1, r1) = blend(p, pc0, pc1);
            let (f2, r2) = blend(p, pc0, bg);
            (b0 + 1) as f32 + if r1 <= r2 { f1 } else { f2 }
        }
        None => (b0 + 1) as f32,
    };
    let e_l1 = match at(a1 - 1) {
        Some(p) if a1 - 1 > b0 => {
            let (f1, r1) = blend(p, pc1, pc0);
            let (f2, r2) = blend(p, pc1, bg);
            a1 as f32 - if r1 <= r2 { f1 } else { f2 }
        }
        _ => e_r0,
    };
    let e_r1 = match (at(b1 + 1), at(b1 + 2)) {
        (Some(p), Some(next)) => {
            let (f1, r1) = blend(p, pc1, next);
            let (f2, r2) = blend(p, pc1, bg);
            (b1 + 1) as f32 + if r1 <= r2 { f1 } else { f2 }
        }
        _ => (b1 + 1) as f32,
    };
    let bw0 = e_r0 - e_l0;
    let bw1 = e_r1 - e_l1;
    let bw = if (bw0 - bw1).abs() < 1.5 { (bw0 + bw1) / 2.0 } else { bw0 };
    if !(BLOCK_MIN - WIDTH_SLACK..=BLOCK_MAX).contains(&bw) {
        return fail("block width");
    }
    if ((b1 - a1 + 2) as f32) < 0.7 * bw {
        return fail("B1 width");
    }
    // pitch: the value near the sync pair's that makes all 8 block interiors most uniform
    // (hard-edged blocks are whole pixels wide, so the sync pair alone gives it +-1 px)
    let measured = e_l1 - e_l0;
    let inner = |l: f32, w: f32| -> (i32, i32) { (ceil_i(l + INNER), floor_i(l + w - INNER) - 1) };
    let score = |p: f32| -> f64 {
        let mut s = 0f64;
        for i in 0..8 {
            let (a, b) = inner(e_l0 + i as f32 * p, bw);
            if b < a || a < px0 || b >= px1 {
                return f64::MAX;
            }
            let (ia, ib) = ((a - px0) as usize, (b + 1 - px0) as usize);
            let k = (ib - ia) as f64;
            for c in 0..3 {
                let sum = s1[ib][c] - s1[ia][c];
                s += (s2[ib][c] - s2[ia][c]) - sum * sum / k;
            }
        }
        s
    };
    // Several pitches can leave every interior uniform when neighbouring digits share a colour
    // (a run of black blocks): each such range of pitches is decoded, and the tag is accepted
    // only when every range that decodes agrees.
    let steps = (PITCH_SEARCH / PITCH_STEP) as i32;
    let scores: Vec<(f32, f64)> = (-steps..=steps)
        .map(|k| {
            let p = measured + k as f32 * PITCH_STEP;
            (p, score(p))
        })
        .collect();
    let best = scores.iter().map(|e| e.1).fold(f64::MAX, f64::min);
    if best == f64::MAX {
        return fail("pitch");
    }
    let tol = best + (0.25 * best).max(PITCH_TOL);
    let mut spans_p: Vec<(f32, f32)> = Vec::new();
    let mut k = 0;
    while k < scores.len() {
        if scores[k].1 > tol {
            k += 1;
            continue;
        }
        let a = k;
        while k + 1 < scores.len() && scores[k + 1].1 <= tol {
            k += 1;
        }
        spans_p.push((scores[a].0, scores[k].0));
        k += 1;
    }
    // Second stage: each range is scored with the blocks' full extents (every pixel whose centre
    // falls in a block, B0's edge free by +-0.5 px, the block width free by +-0.5 px). A run of
    // equal digits, or black digits on a dark background, fits one block more or less at another
    // pitch inside the interiors, but the blocks around the run then land a pixel off. The width
    // is part of the fit because hard-edged blocks under 5 px alternate between two whole-pixel
    // widths, so the sync pair alone measures it up to half a pixel off.
    let full = |p: f32, x0: f32, w: f32| -> f64 {
        let (mut ssd, mut n) = (0f64, 0f64);
        for i in 0..8 {
            let l = x0 + i as f32 * p;
            let a = ceil_i(l - 0.5);
            let b = ceil_i(l + w - 0.5) - 1;
            if b < a || a < px0 || b >= px1 {
                return f64::MAX;
            }
            let (ia, ib) = ((a - px0) as usize, (b + 1 - px0) as usize);
            let k = (ib - ia) as f64;
            for c in 0..3 {
                let sum = s1[ib][c] - s1[ia][c];
                ssd += (s2[ib][c] - s2[ia][c]) - sum * sum / k;
            }
            n += 3.0 * k;
        }
        ssd / n
    };
    const OFFS: [f32; 9] = [-0.5, -0.375, -0.25, -0.125, 0.0, 0.125, 0.25, 0.375, 0.5];
    let make_ranges = |widths: &[f32]| -> Vec<(f64, f32, f32, f32, f32)> {
        let mut ranges: Vec<(f64, f32, f32, f32, f32)> = spans_p
            .iter()
            .map(|&(lo, hi)| {
                let mut best_r = (f64::MAX, (lo + hi) / 2.0, (lo + hi) / 2.0, e_l0, bw);
                let mut p = lo - 0.1;
                while p <= hi + 0.1 + 1e-4 {
                    for dx in OFFS {
                        for &dw in widths {
                            let w = bw + dw;
                            if w < BLOCK_MIN - WIDTH_SLACK || w > p + 0.25 {
                                continue;
                            }
                            let f = full(p, e_l0 + dx, w);
                            if f < best_r.0 {
                                best_r = (f, (lo + hi) / 2.0, p, e_l0 + dx, w);
                            }
                        }
                    }
                    p += PITCH_STEP;
                }
                best_r
            })
            .collect();
        let best_full = ranges.iter().map(|r| r.0).fold(f64::MAX, f64::min);
        ranges.retain(|r| r.0 <= best_full * FULL_RATIO + FULL_ABS);
        ranges.sort_by(|x, y| x.0.total_cmp(&y.0));
        ranges
    };
    // each range is read at its middle, then (hard-edged blocks) at its full-extent fit
    // block boxes inside each block's interior
    let cy = (y_top + y_bot) / 2;
    let hy = 1.max(h / 4);
    let (by0, by1) = ((cy - hy).max(y_top), (cy + hy).min(y_bot));
    let decode = |pitch: f32, e_l0: f32, bw: f32| -> Result<(u32, [f32; 3], [f32; 3]), Reject> {
        if pitch < 0.8 * bw || pitch > 1.6 * bw + 2.0 {
            return Err("pitch");
        }
        let hx_spec = 1.max((bw / 4.0) as i32);
        let mut obs = [[0f32; 3]; 8];
        let mut spans = [(0i32, 0i32); 8];
        for (i, o) in obs.iter_mut().enumerate() {
            let (p0, p1) = inner(e_l0 + i as f32 * pitch, bw);
            if p1 < p0 {
                return Err("block width");
            }
            spans[i] = (p0, p1);
            let cx = (p0 + p1) / 2;
            let hx = hx_spec.min((p1 - p0) / 2);
            let mut lo = [f32::MAX; 3];
            let mut hi = [f32::MIN; 3];
            let mut sum = [0f32; 3];
            let mut cnt = 0f32;
            for yy in by0..=by1 {
                for xx in cx - hx..=cx + hx {
                    let p = get(img, xx, yy).ok_or("outside")?;
                    for c in 0..3 {
                        lo[c] = lo[c].min(p[c]);
                        hi[c] = hi[c].max(p[c]);
                        sum[c] += p[c];
                    }
                    cnt += 1.0;
                }
            }
            if (0..3).any(|c| hi[c] - lo[c] > UNIFORM_MAX) {
                return Err("uniformity");
            }
            *o = [sum[0] / cnt, sum[1] / cnt, sum[2] / cnt];
        }
        // references from the sync pair: R hi B0, lo B1; G hi B1, lo B0; B hi B0, lo B1
        let hi = [obs[0][0], obs[1][1], obs[0][2]];
        let lo = [obs[1][0], obs[0][1], obs[1][2]];
        if (0..3).any(|c| hi[c] - lo[c] < LIKE_MIN) {
            return Err("sync contrast");
        }
        // every pixel between two consecutive interiors is a blend of those two blocks' colours
        // and the background (gap): a block skipped by a wrong pitch would show up here
        for i in 0..7 {
            let (ca, cb) = (span_mean(spans[i].0, spans[i].1), span_mean(spans[i + 1].0, spans[i + 1].1));
            for x in spans[i].1 + 1..spans[i + 1].0 {
                let p = at(x).ok_or("outside")?;
                if blend3(p, ca, cb, bg) > BETWEEN_MAX && alphabet_like(p, lo, hi) {
                    // a third block colour between two blocks (the background behind a gap is
                    // not compared: it may differ from the indent's along a gradient)
                    return Err("between blocks");
                }
            }
        }
        let mut v: u32 = 0;
        for o in &obs[2..] {
            let mut bits = [false; 3];
            let mut t = [0f32; 3];
            for c in 0..3 {
                t[c] = (o[c] - lo[c]) / (hi[c] - lo[c]);
                if t[c] > FIT && t[c] < 1.0 - FIT {
                    return Err("alphabet fit");
                }
                bits[c] = t[c] > 0.5;
            }
            // black must be nearer black than the plate, which fades with the tag:
            // plate - black = alpha * (32, 40, 64), alpha * 255 = hi - lo
            if bits == [false, false, false] && t[2] > 0.5 * f32::from(PLATE_RGB[2]) / 255.0 {
                return Err("plate read as black");
            }
            let d = match bits {
                [false, false, false] => 0,
                [true, false, false] => 1,
                [false, false, true] => 2,
                [true, true, false] => 3,
                [false, true, true] => 4,
                [true, true, true] => 5,
                _ => return Err("sync colour in data"), // magenta or green: never a data digit
            };
            v = v * 6 + d;
        }
        if v >= 32768 {
            return Err("range");
        }
        if crate::crc::crc5_tag((v >> 5) as u16) as u32 != v & 31 {
            return Err("crc");
        }
        // the tag ends here: just past B7 comes the plate (faded like the tag) or the background,
        // not more of B7's colour (a run of equal digits read at a wrong pitch ends too early)
        let end = e_l0 + 7.0 * pitch + bw + (pitch - bw).max(0.0);
        let q0 = end.ceil() as i32 + 1;
        if let (Some(q1), Some(q2)) = (at(q0), at(q0 + 1)) {
            let q = [(q1[0] + q2[0]) / 2.0, (q1[1] + q2[1]) / 2.0, (q1[2] + q2[2]) / 2.0];
            let c7 = span_mean(spans[7].0, spans[7].1);
            let mut plate = [0f32; 3];
            for c in 0..3 {
                plate[c] = lo[c] + (hi[c] - lo[c]) * f32::from(PLATE_RGB[c]) / 255.0;
            }
            let after = dist(c7, plate).min(dist(c7, bg));
            if after > END_MIN && dist(q, c7) < dist(q, plate).min(dist(q, bg)) {
                return Err("tag end");
            }
            // nor another non-black block (the plate fades to near black, the background is known)
            let digit_like = alphabet_like(q, lo, hi) && (0..3).any(|c| q[c] - lo[c] >= (1.0 - FIT) * (hi[c] - lo[c]));
            if digit_like && dist(q, bg) > SIMILAR && dist(q, plate) > SIMILAR {
                return Err("tag end");
            }
        }
        Ok((v, hi, lo))
    };
    type Read = (u32, [f32; 3], [f32; 3], f32, f32, f32);
    let read = |mid: f32, p: f32, x0: f32, w: f32| -> Result<Read, Reject> {
        match decode(mid, e_l0, bw) {
            Ok((v, hi, lo)) => Ok((v, hi, lo, mid, e_l0, bw)),
            Err(e) => decode(p, x0, w).map(|(v, hi, lo)| (v, hi, lo, p, x0, w)).map_err(|_| e),
        }
    };
    // every range that decodes must agree; Err only for disagreeing ranges
    let read_all = |ranges: &[(f64, f32, f32, f32, f32)], first_err: &mut Reject| -> Result<Option<Read>, Reject> {
        let mut found: Option<Read> = None;
        for (i, &(_, mid, p, x0, w)) in ranges.iter().enumerate() {
            match read(mid, p, x0, w) {
                Ok(r) => match found {
                    None => found = Some(r),
                    Some(f) if f.0 == r.0 => {}
                    Some(_) => return Err("ambiguous pitch"),
                },
                Err(e) if i == 0 => *first_err = e,
                Err(_) => {}
            }
        }
        Ok(found)
    };
    let mut first_err: Reject = "pitch";
    // the measured block width first; when no range decodes, the width joins the fit (hard-edged
    // blocks under 5 px, see above)
    let mut found = match read_all(&make_ranges(&[0.0]), &mut first_err) {
        Ok(f) => f,
        Err(e) => return fail(e),
    };
    if found.is_none() {
        found = match read_all(&make_ranges(&OFFS), &mut first_err) {
            Ok(f) => f,
            Err(e) => return fail(e),
        };
    }
    let Some((v, hi, lo, pitch, e_l0, bw)) = found else { return fail(first_err) };
    let payload = (v >> 5) as u16;
    let (white, black) = white_black.unwrap_or(([255.0; 3], [0.0; 3]));
    let alpha = ((0..3).map(|c| (hi[c] - lo[c]) / (white[c] - black[c]).max(1.0)).sum::<f32>() / 3.0).clamp(0.0, 1.0);
    Ok(TagHit {
        slot: (payload >> 3) as u8,
        row: ((payload >> 1) & 3) as u8,
        sess_bit: (payload & 1) as u8,
        x0: e_l0.round() as i32,
        x1: (e_l0 + 7.0 * pitch + bw).round() as i32,
        y_top,
        y_bot,
        block_px: bw,
        pitch_px: pitch,
        alpha,
    })
}

/// The 8 block colours of a tag, as the addon draws them (`Bridge.TagColors`).
/// Inputs are masked to their field widths (slot 7 bits, row 2 bits, sessBit 1 bit).
pub fn tag_colors(slot: u8, row: u8, sess_bit: u8) -> [[u8; 3]; 8] {
    let payload = (u16::from(slot & 0x7F) << 3) | (u16::from(row & 3) << 1) | u16::from(sess_bit & 1);
    let mut v = (payload << 5) | u16::from(crate::crc::crc5_tag(payload));
    let mut out = [[0u8; 3]; 8];
    out[0] = SYNC[0];
    out[1] = SYNC[1];
    for i in (2..8).rev() {
        out[i] = DIGITS[(v % 6) as usize];
        v /= 6;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digits(slot: u8, row: u8, sess: u8) -> Vec<usize> {
        tag_colors(slot, row, sess)[2..].iter().map(|c| DIGITS.iter().position(|d| d == c).unwrap()).collect()
    }

    #[test]
    fn check_values() {
        assert_eq!(digits(0, 0, 0), vec![0, 0, 0, 0, 4, 1]);
        assert_eq!(digits(5, 2, 0), vec![0, 1, 0, 3, 3, 3]);
        assert_eq!(digits(127, 0, 0), vec![4, 1, 0, 3, 4, 3]);
    }

    #[test]
    fn sync_colours_never_in_data() {
        for slot in 0..=127u8 {
            for row in 0..4u8 {
                for sess in 0..2u8 {
                    let c = tag_colors(slot, row, sess);
                    assert_eq!((c[0], c[1]), (SYNC[0], SYNC[1]));
                    assert!(c[2..].iter().all(|d| !SYNC.contains(d)));
                }
            }
        }
    }

    #[test]
    fn fast_floor_and_ceil() {
        for i in -4000..4000 {
            let v = i as f32 * 0.137;
            assert_eq!(floor_i(v), v.floor() as i32, "{v}");
            assert_eq!(ceil_i(v), v.ceil() as i32, "{v}");
        }
    }

    #[test]
    fn blend_fraction() {
        let (f, r) = blend([191.25, 63.75, 191.25], [255.0, 0.0, 255.0], [0.0, 255.0, 0.0]);
        assert!((f - 0.75).abs() < 1e-4 && r < 1e-3);
    }
}
