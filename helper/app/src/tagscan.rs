//! The tag search of one capture tick, around `tp_bridge::tags::scan` (P4b CPU cut).
//!
//! 1. **Candidate rows.** `tags::scan` only starts reading a tag where a pixel and one of the
//!    `SYNC_REACH` (4) pixels to its right form the sync pair: B0 above B1 by at least `LIKE_MIN`
//!    (40) on red and blue and below it by as much on green. That relation holds on every inner row
//!    of a tag, whatever the background behind a faded tag. A cheap integer pass checks it (with a
//!    looser threshold, 3/4 of `LIKE_MIN`, and one more pixel of reach) on every second row of the
//!    same x range `tags::scan` searches, and `tags::scan` then runs only on the row bands that
//!    passed, widened by `MARGIN` rows and with the area's own x and width (so its search range
//!    is unchanged). A tag-free area costs a small fraction of a full scan.
//! 2. **Unchanged tags.** A band whose tags all show the same inner pixels as in the previous
//!    scan (and the same calibration) keeps their readings instead of reading them again. A new tag
//!    changes the band's candidate rows, and any change of a tag's pixels (scroll, fade, cover)
//!    makes the band read again.
//! 3. **Unchanged cards** ([`CardCache`]). `cards::build` is a function of its inputs: the image,
//!    the tags, the views, the plate flags and the session bit. When all of them are the same as
//!    for the previous result (the image compared pixel for pixel with the grabber's previous
//!    capture of the same rectangle, which it keeps in its second buffer), the previous cards are
//!    kept. Any pixel change anywhere in the scan area (a tooltip, a scroll, a fade, the game
//!    world behind a translucent chat) builds them again; the comparison stops at the first
//!    difference, so a changing picture costs almost nothing.

use tp_bridge::cards::{self, Card};
use tp_bridge::image::{Bgra, Rect};
use tp_bridge::msg::View;
use tp_bridge::tags::{self, TagHit};

/// Threshold of the candidate test: 3/4 of the scan's own sync threshold.
pub const SYNC_MIN: i32 = (tags::LIKE_MIN * 0.75) as i32;
/// Pixels right of a candidate B0 pixel that may hold B1 (the scan looks 4 px; one more for slack).
pub const REACH: i32 = 5;
/// Rows added above and below each band of candidate rows (a tag's blended edge rows and the rows
/// the every-second-row test skipped).
pub const MARGIN: i32 = 6;

/// What one tick's search did (for the `perf` record).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScanStats {
    /// Rows of the search areas.
    pub area_rows: u32,
    /// Rows handed to `tags::scan`.
    pub scanned_rows: u32,
    pub bands: u32,
    /// Bands whose tags were reused.
    pub reused_bands: u32,
}

struct Memo {
    area_x: i32,
    area_w: i32,
    band: (i32, i32),
    /// The band's tags with the signature of their inner pixels (None: not reusable).
    hits: Vec<(TagHit, Option<u64>)>,
}

/// Remembers the last tick's bands and tags.
#[derive(Default)]
pub struct TagScanner {
    memo: Vec<Memo>,
    white_black: Option<([f32; 3], [f32; 3])>,
}

/// The x range `tags::scan` searches for B0 in `area` (its step 1): `[x_lo, xs_end)`, and the
/// image's right edge.
fn search_x(img: &Bgra, area: Rect) -> (i32, i32, i32) {
    let x_lo = area.x.max(img.origin_x);
    let x_hi = area.x.saturating_add(area.w).min(img.origin_x + img.width as i32);
    let reach = area.w.min(200.max((0.35 * area.w as f32) as i32));
    let xs_end = x_hi.min(area.x.saturating_add(reach));
    (x_lo, xs_end, img.origin_x + img.width as i32)
}

/// True when pixel `k` and pixel `k + d` of a BGRA row form a sync-pair candidate: the first
/// above the second by `SYNC_MIN` on red and blue and below it by as much on green.
#[inline]
fn pair_at(row: &[u8], k: usize, d: usize) -> bool {
    let (p, q) = (&row[k * 4..k * 4 + 3], &row[(k + d) * 4..(k + d) * 4 + 3]);
    let t = SYNC_MIN as u8;
    (p[0].saturating_sub(q[0]) >= t) & (q[1].saturating_sub(p[1]) >= t) & (p[2].saturating_sub(q[2]) >= t)
}

/// Pixels `[from, lim)` of a BGRA row, each against the pixel `d` to its right.
fn pairs_scalar(row: &[u8], from: usize, lim: usize, d: usize) -> bool {
    (from..lim).any(|k| pair_at(row, k, d))
}

/// Same as [`pairs_scalar`] from pixel 0, 4 pixels per step with SSE2 (part of the x86-64
/// baseline): saturating differences per byte, a per-lane threshold test, then a pixel passes when
/// none of its 4 lanes failed (alpha is forced to pass).
#[cfg(target_arch = "x86_64")]
fn pairs(row: &[u8], lim: usize, d: usize) -> bool {
    use std::arch::x86_64::*;
    let n = row.len() / 4;
    let mut k = 0usize;
    // SAFETY: every load reads 16 bytes starting at pixel k or k + d with k + d + 4 <= n.
    unsafe {
        let below = _mm_set1_epi8((SYNC_MIN - 1) as i8);
        let br = _mm_set1_epi32(0x00FF_00FF);
        let g = _mm_set1_epi32(0x0000_FF00);
        let a = _mm_set1_epi32(0xFF00_0000u32 as i32);
        let zero = _mm_setzero_si128();
        while k + 4 <= lim && k + d + 4 <= n {
            let p = _mm_loadu_si128(row.as_ptr().add(k * 4) as *const __m128i);
            let q = _mm_loadu_si128(row.as_ptr().add((k + d) * 4) as *const __m128i);
            let v = _mm_or_si128(
                _mm_or_si128(_mm_and_si128(_mm_subs_epu8(p, q), br), _mm_and_si128(_mm_subs_epu8(q, p), g)),
                a,
            );
            // 0xFF in every lane below the threshold
            let fail = _mm_movemask_epi8(_mm_cmpeq_epi8(_mm_subs_epu8(v, below), zero)) as u32;
            if (fail | fail >> 1 | fail >> 2 | fail >> 3) & 0x1111 != 0x1111 {
                return true;
            }
            k += 4;
        }
    }
    pairs_scalar(row, k, lim, d)
}

#[cfg(not(target_arch = "x86_64"))]
fn pairs(row: &[u8], lim: usize, d: usize) -> bool {
    pairs_scalar(row, 0, lim, d)
}

/// True when row `y` holds a sync-pair candidate with B0 in `[x0, x1)` and B1 1..=REACH px to its
/// right (within the image).
fn row_has_candidate(img: &Bgra, y: i32, x0: i32, x1: i32, img_right: i32) -> bool {
    let ly = (y - img.origin_y) as usize;
    let start = ly * img.stride as usize;
    let end = (x1 + REACH).min(img_right);
    let (a, b) = ((x0 - img.origin_x) as usize * 4, (end - img.origin_x) as usize * 4);
    let Some(row) = img.px.get(start + a..start + b) else { return false };
    let n = row.len() / 4;
    let m = ((x1 - x0) as usize).min(n);
    (1..=REACH as usize).any(|d| pairs(row, m.min(n.saturating_sub(d)), d))
}

/// Row bands `[y0, y1)` of `area` that may hold a tag (see the module notes).
pub fn candidate_bands(img: &Bgra, area: Rect, out: &mut Vec<(i32, i32)>) {
    out.clear();
    let y_lo = area.y.max(img.origin_y);
    let y_hi = area.y.saturating_add(area.h).min(img.origin_y + img.height as i32);
    let (x0, x1, right) = search_x(img, area);
    if x1 <= x0 || y_hi <= y_lo || area.w <= 0 {
        return;
    }
    let mut cur: Option<(i32, i32)> = None;
    let mut y = y_lo;
    while y < y_hi {
        if row_has_candidate(img, y, x0, x1, right) {
            cur = match cur {
                Some((a, b)) if y - b <= 2 * MARGIN => Some((a, y)),
                Some(done) => {
                    out.push(done);
                    Some((y, y))
                }
                None => Some((y, y)),
            };
        }
        y += 2;
    }
    if let Some(done) = cur {
        out.push(done);
    }
    for b in out.iter_mut() {
        *b = ((b.0 - MARGIN).max(y_lo), (b.1 + 1 + MARGIN).min(y_hi));
    }
    // Widening can make neighbours overlap: merge them.
    let mut merged: Vec<(i32, i32)> = Vec::with_capacity(out.len());
    for &b in out.iter() {
        match merged.last_mut() {
            Some(last) if b.0 <= last.1 => last.1 = last.1.max(b.1),
            _ => merged.push(b),
        }
    }
    *out = merged;
}

/// FNV-1a over the inner pixels of a tag (its blended edge pixels left out), or None when they
/// are not all inside the image.
fn tag_signature(img: &Bgra, h: &TagHit) -> Option<u64> {
    let (x0, x1) = (h.x0 + 1, h.x1 - 1);
    let (y0, y1) = (h.y_top + 1, h.y_bot - 1);
    if x1 <= x0 || y1 < y0 {
        return None;
    }
    let lx0 = x0 - img.origin_x;
    let lx1 = x1 - img.origin_x;
    if lx0 < 0 || lx1 > img.width as i32 || y0 < img.origin_y || y1 >= img.origin_y + img.height as i32 {
        return None;
    }
    let mut s = 0xcbf2_9ce4_8422_2325u64;
    for y in y0..=y1 {
        let start = (y - img.origin_y) as usize * img.stride as usize;
        let row = img.px.get(start + lx0 as usize * 4..start + lx1 as usize * 4)?;
        for c in row.chunks_exact(8) {
            s ^= u64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]);
            s = s.wrapping_mul(0x100_0000_01b3);
        }
        for &b in row.chunks_exact(8).remainder() {
            s ^= u64::from(b);
            s = s.wrapping_mul(0x100_0000_01b3);
        }
    }
    Some(s)
}

impl TagScanner {
    pub fn new() -> TagScanner {
        TagScanner::default()
    }

    /// Forgets the remembered tags (new window, new session).
    pub fn reset(&mut self) {
        self.memo.clear();
        self.white_black = None;
    }

    /// Finds the tags of every area (in order) and appends them to `out`, like `tags::scan` on
    /// each area.
    pub fn scan(&mut self, img: &Bgra, areas: &[Rect], white_black: Option<([f32; 3], [f32; 3])>, out: &mut Vec<TagHit>) -> ScanStats {
        let mut st = ScanStats::default();
        let same_calib = self.white_black == white_black;
        let old = std::mem::take(&mut self.memo);
        let mut bands: Vec<(i32, i32)> = Vec::new();
        for &area in areas {
            let y_lo = area.y.max(img.origin_y);
            let y_hi = area.y.saturating_add(area.h).min(img.origin_y + img.height as i32);
            st.area_rows += (y_hi - y_lo).max(0) as u32;
            candidate_bands(img, area, &mut bands);
            for &band in &bands {
                st.bands += 1;
                let prev = old.iter().find(|m| m.area_x == area.x && m.area_w == area.w && m.band == band);
                let reuse = same_calib
                    && prev.is_some_and(|m| !m.hits.is_empty() && m.hits.iter().all(|(h, sig)| sig.is_some() && tag_signature(img, h) == *sig));
                let hits: Vec<(TagHit, Option<u64>)> = if let (true, Some(m)) = (reuse, prev) {
                    st.reused_bands += 1;
                    m.hits.clone()
                } else {
                    st.scanned_rows += (band.1 - band.0) as u32;
                    let mut found = Vec::new();
                    tags::scan(img, Rect { x: area.x, y: band.0, w: area.w, h: band.1 - band.0 }, white_black, &mut found);
                    found.into_iter().map(|h| (tag_signature(img, &h), h)).map(|(sig, h)| (h, sig)).collect()
                };
                out.extend(hits.iter().map(|(h, _)| h.clone()));
                self.memo.push(Memo { area_x: area.x, area_w: area.w, band, hits });
            }
        }
        self.white_black = white_black;
        st
    }
}

/// The inputs of the last `cards::build` and its result (see the module notes, step 3).
#[derive(Default)]
pub struct CardCache {
    hits: Vec<TagHit>,
    views: Vec<View>,
    plates: Vec<(u8, Option<bool>)>,
    sess_bit: Option<u8>,
    /// Origin and size of the image the cards come from.
    image: [i64; 4],
    /// Number of the grab whose image the cards were built or checked against.
    frame: u64,
    cards: Vec<Card>,
    valid: bool,
}

fn same_hits(a: &[TagHit], b: &[TagHit]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            (x.slot, x.row, x.sess_bit, x.x0, x.x1, x.y_top, x.y_bot) == (y.slot, y.row, y.sess_bit, y.x0, y.x1, y.y_top, y.y_bot)
                && x.block_px == y.block_px
                && x.pitch_px == y.pitch_px
                && x.alpha == y.alpha
        })
}

fn image_key(img: &Bgra) -> [i64; 4] {
    [img.origin_x.into(), img.origin_y.into(), img.width.into(), img.height.into()]
}

/// True when two captures of the same rectangle hold the same pixels (stops at the first
/// difference, so a changing picture costs almost nothing).
pub fn same_pixels(a: &Bgra, b: &Bgra) -> bool {
    if image_key(a) != image_key(b) {
        return false;
    }
    let row = a.width as usize * 4;
    (0..a.height as usize).all(|y| {
        let (sa, sb) = (y * a.stride as usize, y * b.stride as usize);
        a.px.get(sa..sa + row) == b.px.get(sb..sb + row)
    })
}

impl CardCache {
    pub fn new() -> CardCache {
        CardCache::default()
    }

    pub fn reset(&mut self) {
        self.valid = false;
        self.cards.clear();
    }

    /// `cards::build(img, hits, views, plate_expected, sess_bit)` for the capture numbered
    /// `frame`, or the previous result when its inputs are the same and `prev` (the capture of
    /// the same rectangle the previous result came from, with its number) holds exactly the same
    /// pixels as `img`. The bool is true when the previous cards were kept.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        &mut self,
        img: &Bgra,
        frame: u64,
        prev: Option<(&Bgra, u64)>,
        hits: &[TagHit],
        views: &[View],
        plate_expected: &dyn Fn(u8) -> Option<bool>,
        sess_bit: Option<u8>,
    ) -> (Vec<Card>, bool) {
        let mut plates: Vec<(u8, Option<bool>)> = hits.iter().map(|h| (h.slot, plate_expected(h.slot))).collect();
        plates.sort_unstable_by_key(|p| p.0);
        plates.dedup();
        let image = image_key(img);
        let keep = self.valid
            && frame != 0
            && self.image == image
            && self.sess_bit == sess_bit
            && self.views == views
            && self.plates == plates
            && same_hits(&self.hits, hits)
            && prev.is_some_and(|(p, n)| n == self.frame && same_pixels(p, img));
        if keep {
            self.frame = frame;
            return (self.cards.clone(), true);
        }
        let cards = cards::build(img, hits, views, plate_expected, sess_bit);
        self.hits = hits.to_vec();
        self.views = views.to_vec();
        self.plates = plates;
        self.sess_bit = sess_bit;
        self.image = image;
        self.frame = frame;
        self.cards = cards.clone();
        self.valid = frame != 0;
        (cards, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dark BGRA image with one tag-like sync pair (magenta then green, 6 px each) at (x, y).
    fn image(w: u32, h: u32, tags_at: &[(i32, i32, i32)]) -> Vec<u8> {
        let mut px = vec![0u8; (w * h * 4) as usize];
        for p in px.chunks_exact_mut(4) {
            p.copy_from_slice(&[20, 18, 16, 255]);
        }
        for &(x, y, hgt) in tags_at {
            for yy in y..y + hgt {
                for xx in x..x + 12 {
                    let i = ((yy as u32 * w + xx as u32) * 4) as usize;
                    let c: [u8; 4] = if xx < x + 6 { [255, 0, 255, 255] } else { [0, 255, 0, 255] };
                    px[i..i + 4].copy_from_slice(&c);
                }
            }
        }
        px
    }

    #[test]
    fn bands_cover_candidate_rows_with_margin() {
        let px = image(400, 300, &[(30, 100, 14), (40, 120, 14), (10, 250, 10)]);
        let img = Bgra { px: &px, width: 400, height: 300, stride: 1600, origin_x: 0, origin_y: 0 };
        let mut out = Vec::new();
        candidate_bands(&img, Rect { x: 0, y: 0, w: 400, h: 300 }, &mut out);
        assert_eq!(out.len(), 2, "{out:?}");
        let (a, b) = out[0];
        assert!(a <= 100 - MARGIN + 1 && b >= 134 + MARGIN - 1, "{out:?}");
        let (c, d) = out[1];
        assert!(c <= 250 - MARGIN + 1 && d >= 260 + MARGIN - 1 && d <= 300, "{out:?}");
    }

    #[test]
    fn faded_pair_over_a_bright_background_is_a_candidate() {
        // alpha 0.3 over green ground: neither block is magenta-like, the pair relation holds.
        let mut px = image(300, 40, &[]);
        let mix = |c: [f32; 3], a: f32| -> [u8; 4] {
            let bg = [40.0, 150.0, 30.0];
            let v = |i: usize| (c[i] * a + bg[i] * (1.0 - a)).round() as u8;
            [v(2), v(1), v(0), 255]
        };
        for y in 10..24 {
            for x in 50..62 {
                let i = ((y * 300 + x) * 4) as usize;
                let p = if x < 56 { mix([255.0, 0.0, 255.0], 0.3) } else { mix([0.0, 255.0, 0.0], 0.3) };
                px[i..i + 4].copy_from_slice(&p);
            }
        }
        let img = Bgra { px: &px, width: 300, height: 40, stride: 1200, origin_x: 0, origin_y: 0 };
        let mut out = Vec::new();
        candidate_bands(&img, Rect { x: 0, y: 0, w: 300, h: 40 }, &mut out);
        assert_eq!(out.len(), 1, "{out:?}");
    }

    #[test]
    fn nothing_outside_the_search_range() {
        // The pair sits right of the scan's search range (max(200, 35%) of a 1000 px area).
        let px = image(1000, 60, &[(600, 20, 14)]);
        let img = Bgra { px: &px, width: 1000, height: 60, stride: 4000, origin_x: 0, origin_y: 0 };
        let mut out = Vec::new();
        candidate_bands(&img, Rect { x: 0, y: 0, w: 1000, h: 60 }, &mut out);
        assert!(out.is_empty(), "{out:?}");
        // With an image that starts lower (client coordinates), the rows follow the origin.
        let px2 = image(400, 60, &[(30, 20, 14)]);
        let img2 = Bgra { px: &px2, width: 400, height: 60, stride: 1600, origin_x: 0, origin_y: 500 };
        candidate_bands(&img2, Rect { x: 0, y: 500, w: 400, h: 60 }, &mut out);
        assert_eq!(out.len(), 1);
        assert!(out[0].0 >= 500 && out[0].0 <= 520 && out[0].1 >= 534);
    }

    /// Cost of the candidate pass over a 645 x 464 chat area of mid-tone pixels with +-6 noise (no
    /// tag).
    /// Run: `cargo test --release --lib candidate_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn candidate_cost() {
        let (w, h) = (645u32, 464u32);
        let mut px = vec![0u8; (w * h * 4) as usize];
        let mut s = 0x1234_5678u32;
        for v in px.iter_mut() {
            s = s.wrapping_mul(1_103_515_245).wrapping_add(12345);
            *v = 90 + ((s >> 16) % 13) as u8;
        }
        let img = Bgra { px: &px, width: w, height: h, stride: w * 4, origin_x: 0, origin_y: 0 };
        let mut out = Vec::new();
        let n = 200;
        let t0 = std::time::Instant::now();
        for _ in 0..n {
            candidate_bands(&img, Rect { x: 0, y: 0, w: w as i32, h: h as i32 }, &mut out);
        }
        eprintln!("candidates: {:.3} ms per area, {} bands", t0.elapsed().as_secs_f64() * 1000.0 / f64::from(n), out.len());
    }

    #[test]
    fn vector_and_scalar_pair_tests_agree() {
        let mut s = 0x9E37_79B9u32;
        let mut next = || {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (s >> 24) as u8
        };
        for len in [1usize, 3, 4, 5, 7, 8, 9, 16, 33] {
            for _ in 0..400 {
                let mut row: Vec<u8> = (0..(len + REACH as usize) * 4).map(|_| next()).collect();
                // Sometimes plant one exact pair.
                if next() % 3 == 0 {
                    let k = next() as usize % len;
                    let d = 1 + next() as usize % REACH as usize;
                    row[k * 4..k * 4 + 3].copy_from_slice(&[200, 10, 220]);
                    row[(k + d) * 4..(k + d) * 4 + 3].copy_from_slice(&[150, 60, 170]);
                }
                let n = row.len() / 4;
                for d in 1..=REACH as usize {
                    let lim = len.min(n - d);
                    assert_eq!(pairs(&row, lim, d), pairs_scalar(&row, 0, lim, d), "len {len} d {d}");
                }
            }
        }
        // The threshold itself: a difference of exactly SYNC_MIN passes, one less does not.
        let t = SYNC_MIN as u8;
        let mut row = vec![0u8; 8 * 4];
        row[0..3].copy_from_slice(&[t, 0, t]);
        row[4..7].copy_from_slice(&[0, t, 0]);
        assert!(pairs(&row, 4, 1));
        row[4 + 1] = t - 1;
        assert!(!pairs(&row, 4, 1));
    }

    #[test]
    fn empty_area_scans_nothing() {
        let px = image(300, 200, &[]);
        let img = Bgra { px: &px, width: 300, height: 200, stride: 1200, origin_x: 0, origin_y: 0 };
        let mut s = TagScanner::new();
        let mut hits = Vec::new();
        let st = s.scan(&img, &[Rect { x: 0, y: 0, w: 300, h: 200 }], None, &mut hits);
        assert!(hits.is_empty());
        assert_eq!((st.area_rows, st.scanned_rows, st.bands), (200, 0, 0));
    }
}
