//! Pixel composition of cards into a premultiplied BGRA canvas (design D.3 "Overlay drawing").
//!
//! Per card: the tags are painted over with the observed plate colour (amendment A2), the
//! no-plate fill and the small gaps between rows are filled, then each text line is drawn with a
//! 1 px black shadow at 0.7. Final alpha = coverage x mask x card alpha: text only lands on plate
//! pixels that are visible in this capture, so a tooltip or menu over a row is never painted over.
//!
//! CPU (P4b): a canvas is reused from one render to the next and only its dirty rectangle (the
//! bounding box of what was painted) is cleared, counted and copied; the text loop skips pixels
//! without coverage before any floating-point work.

use tp_bridge::cards::{Card, Mask, RowBand};
use tp_bridge::image::Rect;

pub const SHADOW_ALPHA: f32 = 0.7;
/// `SHADOW_ALPHA` on the 0..255 scale.
const SHADOW_ALPHA_8: u32 = 179;
/// No-plate mode fills the text rectangle at 0.92 x alpha (design D.2 step 9).
pub const NO_PLATE_FILL: f32 = 0.92;

/// Premultiplied BGRA pixels covering `rect` (client coordinates).
#[derive(Clone, Debug)]
pub struct Canvas {
    pub rect: Rect,
    pub px: Vec<u8>,
    /// Bounding box `[x0, y0, x1, y1)` (client coordinates) of every pixel painted since the last
    /// clear; everything outside it is transparent black.
    dirty: Option<[i32; 4]>,
}

/// 8-bit text coverage (0 = none, 255 = full), `w` x `h`, row-major.
#[derive(Clone, Debug, PartialEq)]
pub struct Coverage {
    pub w: i32,
    pub h: i32,
    pub a: Vec<u8>,
}

impl Coverage {
    pub fn at(&self, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            0
        } else {
            self.a[(y * self.w + x) as usize]
        }
    }
}

/// One text line to draw into a band.
pub struct TextDraw<'a> {
    /// Index into `card.bands`.
    pub band: usize,
    pub cov: &'a Coverage,
    pub rgb: [u8; 3],
    /// Extra opacity (the status pulse); 1.0 normally.
    pub alpha_mul: f32,
}

/// Pixel counts painted by one card (for `--diag-json`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PaintStats {
    pub tag_px: usize,
    pub fill_px: usize,
    pub text_px: usize,
}

fn mask_bit(m: &Mask, x: i32, y: i32) -> bool {
    let (lx, ly) = (x - m.rect.x, y - m.rect.y);
    if lx < 0 || ly < 0 || lx >= m.rect.w || ly >= m.rect.h {
        return false;
    }
    m.bits.get((ly * m.rect.w + lx) as usize).is_some_and(|b| *b != 0)
}

impl Canvas {
    pub fn new(rect: Rect) -> Canvas {
        let n = (rect.w.max(0) * rect.h.max(0)) as usize * 4;
        Canvas { rect, px: vec![0; n], dirty: None }
    }

    /// Makes the canvas transparent over `rect`, reusing its buffer: only the dirty rectangle is
    /// cleared when the size is unchanged.
    pub fn reset(&mut self, rect: Rect) {
        let n = (rect.w.max(0) * rect.h.max(0)) as usize * 4;
        if rect.w != self.rect.w || rect.h != self.rect.h || self.px.len() != n {
            self.px.clear();
            self.px.resize(n, 0);
        } else if let Some(d) = self.dirty_local() {
            let row = self.rect.w as usize * 4;
            for y in d[1]..d[3] {
                let start = y as usize * row + d[0] as usize * 4;
                self.px[start..start + (d[2] - d[0]) as usize * 4].fill(0);
            }
        }
        self.rect = rect;
        self.dirty = None;
    }

    /// The dirty rectangle in canvas coordinates, clipped to the canvas.
    fn dirty_local(&self) -> Option<[i32; 4]> {
        let d = self.dirty?;
        let x0 = (d[0] - self.rect.x).max(0);
        let y0 = (d[1] - self.rect.y).max(0);
        let x1 = (d[2] - self.rect.x).min(self.rect.w);
        let y1 = (d[3] - self.rect.y).min(self.rect.h);
        (x1 > x0 && y1 > y0).then_some([x0, y0, x1, y1])
    }

    /// The dirty rectangle (client coordinates, clipped to the canvas), None when nothing was
    /// painted since the last clear.
    pub fn dirty(&self) -> Option<Rect> {
        self.dirty_local().map(|d| Rect { x: d[0] + self.rect.x, y: d[1] + self.rect.y, w: d[2] - d[0], h: d[3] - d[1] })
    }

    /// Adds `r` to the dirty rectangle.
    fn mark(&mut self, r: Rect) {
        if r.w <= 0 || r.h <= 0 {
            return;
        }
        let n = [r.x, r.y, r.x + r.w, r.y + r.h];
        self.dirty = Some(match self.dirty {
            None => n,
            Some(d) => [d[0].min(n[0]), d[1].min(n[1]), d[2].max(n[2]), d[3].max(n[3])],
        });
    }

    fn index(&self, x: i32, y: i32) -> Option<usize> {
        let (lx, ly) = (x - self.rect.x, y - self.rect.y);
        if lx < 0 || ly < 0 || lx >= self.rect.w || ly >= self.rect.h {
            return None;
        }
        Some(((ly * self.rect.w + lx) * 4) as usize)
    }

    /// Source-over blend of `rgb` at opacity `a` (0..1) onto the pixel at client `(x, y)`.
    /// Returns false when the pixel is outside the canvas or `a` rounds to 0.
    pub fn blend(&mut self, x: i32, y: i32, rgb: [u8; 3], a: f32) -> bool {
        let ok = self.blend_unmarked(x, y, rgb, a);
        if ok {
            self.mark(Rect { x, y, w: 1, h: 1 });
        }
        ok
    }

    /// [`Canvas::blend`] without growing the dirty rectangle (the caller marks a whole area).
    fn blend_unmarked(&mut self, x: i32, y: i32, rgb: [u8; 3], a: f32) -> bool {
        let Some(i) = self.index(x, y) else { return false };
        blend_px(&mut self.px[i..i + 4], rgb, a)
    }

    /// Fills `r` (client coordinates) at opacity `a`; returns the pixels painted.
    pub fn fill(&mut self, r: Rect, rgb: [u8; 3], a: f32) -> usize {
        let (x0, x1) = (r.x.max(self.rect.x), (r.x + r.w).min(self.rect.x + self.rect.w));
        let (y0, y1) = (r.y.max(self.rect.y), (r.y + r.h).min(self.rect.y + self.rect.h));
        let mut n = 0;
        for y in y0..y1 {
            let row = ((y - self.rect.y) * self.rect.w) as usize * 4;
            for x in x0..x1 {
                let i = row + (x - self.rect.x) as usize * 4;
                n += usize::from(blend_px(&mut self.px[i..i + 4], rgb, a));
            }
        }
        if n > 0 {
            self.mark(r);
        }
        n
    }

    /// 1 px outline of `r` (the `--diag` frames).
    pub fn outline(&mut self, r: Rect, rgb: [u8; 3]) {
        if r.w <= 0 || r.h <= 0 {
            return;
        }
        for x in r.x..r.x + r.w {
            self.blend(x, r.y, rgb, 1.0);
            self.blend(x, r.y + r.h - 1, rgb, 1.0);
        }
        for y in r.y..r.y + r.h {
            self.blend(r.x, y, rgb, 1.0);
            self.blend(r.x + r.w - 1, y, rgb, 1.0);
        }
    }

    /// Pixels with a non-zero alpha (only the dirty rectangle can hold any).
    pub fn painted(&self) -> usize {
        let Some(d) = self.dirty_local() else { return 0 };
        let row = self.rect.w as usize * 4;
        (d[1]..d[3])
            .map(|y| {
                let start = y as usize * row + d[0] as usize * 4;
                self.px[start..start + (d[2] - d[0]) as usize * 4].chunks_exact(4).filter(|p| p[3] != 0).count()
            })
            .sum()
    }

    /// (premultiplied [R, G, B], alpha) at client `(x, y)`.
    pub fn pixel(&self, x: i32, y: i32) -> Option<([u8; 3], u8)> {
        let i = self.index(x, y)?;
        let p = &self.px[i..i + 4];
        Some(([p[2], p[1], p[0]], p[3]))
    }
}

/// Source-over blend of `rgb` at opacity `a` onto one premultiplied BGRA pixel; false when `a`
/// rounds to 0.
#[inline]
fn blend_px(p: &mut [u8], rgb: [u8; 3], a: f32) -> bool {
    // `+ 0.5` then truncation: `f32::round` is a C runtime call on the baseline x86-64 target.
    let sa = (a.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    if sa == 0 {
        return false;
    }
    let inv = 255 - sa;
    // BGRA order; colour channels premultiplied.
    for (c, src) in [(0usize, rgb[2]), (1, rgb[1]), (2, rgb[0])] {
        let s = (u32::from(src) * sa + 127) / 255;
        p[c] = (s + (u32::from(p[c]) * inv + 127) / 255).min(255) as u8;
    }
    p[3] = (sa + (u32::from(p[3]) * inv + 127) / 255).min(255) as u8;
    true
}

fn band_bottom_row(b: &RowBand) -> i32 {
    match &b.mask {
        Some(m) => m.rect.y + m.rect.h - 1,
        None => b.text_rect.y + b.text_rect.h - 1,
    }
}

fn band_top_row(b: &RowBand) -> i32 {
    match &b.mask {
        Some(m) => m.rect.y,
        None => b.text_rect.y,
    }
}

/// Paints one card. `texts` are the lines to draw, already fitted and rasterized.
pub fn paint_card(c: &mut Canvas, card: &Card, texts: &[TextDraw], diag: bool) -> PaintStats {
    let mut st = PaintStats::default();
    let alpha = card.alpha.clamp(0.0, 1.0);
    let fill_alpha = if card.plate { alpha } else { alpha * NO_PLATE_FILL };

    // A2: hide the tags (every hit passed the per-block uniformity check of the scan). Opaque:
    // the observed plate colour already is what the faded plate looks like on screen, so a
    // translucent cover would let a faded tag show through.
    for b in &card.bands {
        if b.tag_rect.w > 0 && b.tag_rect.h > 0 {
            st.tag_px += c.fill(b.tag_rect, card.plate_rgb, 1.0);
        }
    }

    // No-plate mode: the text rectangle gets a fill sampled at the view's right edge.
    if !card.plate {
        for b in &card.bands {
            st.fill_px += c.fill(b.text_rect, card.plate_rgb, fill_alpha);
        }
    }

    // Gap fill between consecutive rows, only where both neighbouring rows' masks are set.
    for g in &card.gaps {
        let above = card.bands.iter().filter(|b| band_bottom_row(b) < g.y + g.h).max_by_key(|b| band_bottom_row(b));
        let below = card.bands.iter().filter(|b| band_top_row(b) >= g.y).min_by_key(|b| band_top_row(b));
        let (Some(above), Some(below)) = (above, below) else { continue };
        for x in g.x..g.x + g.w {
            let ok = match (&above.mask, &below.mask) {
                (Some(ma), Some(mb)) => mask_bit(ma, x, band_bottom_row(above)) && mask_bit(mb, x, band_top_row(below)),
                (None, None) => !card.plate,
                _ => false,
            };
            if ok {
                for y in g.y..g.y + g.h {
                    st.fill_px += usize::from(c.blend(x, y, card.plate_rgb, fill_alpha));
                }
            }
        }
    }

    // Text over its shadow, composed at full opacity first and then faded as one layer by the
    // card alpha (like the chat line it belongs to), limited to the text rectangle and the mask.
    for t in texts {
        let Some(b) = card.bands.get(t.band) else { continue };
        let tr = b.text_rect;
        let ox = tr.x;
        let oy = tr.y + (tr.h - t.cov.h) / 2;
        let a = alpha * t.alpha_mul.clamp(0.0, 1.0);
        let a8 = (a.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
        // The text rectangle inside the canvas; rows of glyph coverage, of the shadow (the glyph
        // one pixel up-left) and of the mask are taken once per row.
        let (x0, x1) = (tr.x.max(c.rect.x), (tr.x + tr.w).min(c.rect.x + c.rect.w));
        let (y0, y1) = (tr.y.max(c.rect.y), (tr.y + tr.h).min(c.rect.y + c.rect.h));
        let cov = t.cov;
        let cov_row = |r: i32| -> Option<&[u8]> {
            (r >= 0 && r < cov.h).then(|| &cov.a[(r * cov.w) as usize..((r + 1) * cov.w) as usize])
        };
        let at = |row: Option<&[u8]>, i: i32| -> u8 {
            match row {
                Some(r) if i >= 0 => r.get(i as usize).copied().unwrap_or(0),
                _ => 0,
            }
        };
        let mut any = false;
        for y in y0..y1 {
            let (glyph, shadow) = (cov_row(y - oy), cov_row(y - oy - 1));
            if glyph.is_none() && shadow.is_none() {
                continue;
            }
            // Text lands only on mask pixels (outside the mask rectangle nothing is allowed).
            let mask_row = match &b.mask {
                None => None,
                Some(m) => {
                    let ly = y - m.rect.y;
                    if ly < 0 || ly >= m.rect.h {
                        continue;
                    }
                    Some((m.rect.x, &m.bits[(ly * m.rect.w) as usize..((ly + 1) * m.rect.w) as usize]))
                }
            };
            let crow = ((y - c.rect.y) * c.rect.w) as usize * 4;
            for x in x0..x1 {
                let tc8 = at(glyph, x - ox);
                let sc8 = at(shadow, x - ox - 1);
                // Most of the rectangle has neither glyph nor shadow.
                if tc8 == 0 && sc8 == 0 {
                    continue;
                }
                if let Some((mx0, bits)) = mask_row {
                    if at(Some(bits), x - mx0) == 0 {
                        continue;
                    }
                }
                // Integer form of: layer = tc + 0.7 sc (1 - tc); source alpha = layer x a; the
                // premultiplied source colour is rgb x tc x a (the shadow is black).
                let (tc, sc) = (u32::from(tc8), (u32::from(sc8) * SHADOW_ALPHA_8 + 127) / 255);
                let layer = tc + (sc * (255 - tc) + 127) / 255;
                let sa = (layer * a8 + 127) / 255;
                if sa == 0 {
                    continue;
                }
                let inv = 255 - sa;
                let i = crow + (x - c.rect.x) as usize * 4;
                let p = &mut c.px[i..i + 4];
                for (ch, src) in [(0usize, t.rgb[2]), (1, t.rgb[1]), (2, t.rgb[0])] {
                    let s = (u32::from(src) * tc * a8 + 32512) / 65025;
                    p[ch] = (s + (u32::from(p[ch]) * inv + 127) / 255).min(255) as u8;
                }
                p[3] = (sa + (u32::from(p[3]) * inv + 127) / 255).min(255) as u8;
                any = true;
                if tc8 > 0 {
                    st.text_px += 1;
                }
            }
        }
        if any {
            c.mark(tr);
        }
    }

    if diag {
        for b in &card.bands {
            c.outline(b.tag_rect, [255, 255, 0]);
            c.outline(b.text_rect, [0, 255, 255]);
        }
    }
    st
}

#[cfg(test)]
mod tests {
    use super::*;
    use tp_bridge::cards::Mask;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    fn band(row: u8, y: i32, mask: Option<Mask>) -> RowBand {
        RowBand { row, text_rect: rect(60, y, 40, 10), mask, tag_rect: rect(10, y, 47, 10) }
    }

    fn full_mask(y: i32) -> Mask {
        Mask { rect: rect(57, y, 50, 10), bits: vec![255; 500] }
    }

    fn card(bands: Vec<RowBand>, alpha: f32, plate: bool) -> Card {
        Card { slot: 1, view_id: 0, bands, gaps: Vec::new(), alpha, plate, plate_rgb: [32, 40, 64], text_width_px: 40 }
    }

    fn solid(w: i32, h: i32) -> Coverage {
        Coverage { w, h, a: vec![255; (w * h) as usize] }
    }

    #[test]
    fn reset_clears_only_what_was_painted_and_keeps_the_buffer() {
        let mut c = Canvas::new(rect(100, 50, 40, 20));
        assert!(c.dirty().is_none());
        assert_eq!(c.painted(), 0);
        c.fill(rect(110, 55, 5, 3), [255, 0, 0], 1.0);
        c.blend(139, 69, [0, 0, 255], 1.0);
        let d = c.dirty().unwrap();
        assert_eq!((d.x, d.y, d.w, d.h), (110, 55, 30, 15));
        assert_eq!(c.painted(), 16);
        let ptr = c.px.as_ptr();
        c.reset(rect(100, 50, 40, 20));
        assert_eq!(c.px.as_ptr(), ptr, "same buffer");
        assert!(c.px.iter().all(|v| *v == 0), "cleared");
        assert!(c.dirty().is_none());
        // Moved but same size: still reused; a new size reallocates.
        c.fill(rect(130, 80, 2, 2), [1, 2, 3], 1.0);
        c.reset(rect(0, 0, 40, 20));
        assert!(c.px.iter().all(|v| *v == 0));
        c.reset(rect(0, 0, 10, 10));
        assert_eq!(c.px.len(), 400);
        // Painting outside the canvas marks nothing.
        c.fill(rect(50, 50, 3, 3), [1, 2, 3], 1.0);
        assert!(c.dirty().is_none());
    }

    #[test]
    fn blend_is_premultiplied_source_over() {
        let mut c = Canvas::new(rect(0, 0, 2, 1));
        assert!(c.blend(0, 0, [255, 255, 255], 0.5));
        assert_eq!(c.pixel(0, 0), Some(([128, 128, 128], 128)));
        c.blend(0, 0, [0, 0, 0], 1.0);
        assert_eq!(c.pixel(0, 0), Some(([0, 0, 0], 255)));
        assert!(!c.blend(5, 0, [1, 2, 3], 1.0), "outside");
        assert!(!c.blend(1, 0, [1, 2, 3], 0.001), "rounds to zero");
        assert_eq!(c.painted(), 1);
    }

    #[test]
    fn tags_are_painted_over_with_the_plate_colour() {
        let mut c = Canvas::new(rect(0, 0, 200, 40));
        let k = card(vec![band(0, 5, Some(full_mask(5)))], 1.0, true);
        let st = paint_card(&mut c, &k, &[], false);
        assert_eq!(st.tag_px, 47 * 10);
        assert_eq!(c.pixel(10, 5), Some(([32, 40, 64], 255)));
        assert_eq!(c.pixel(56, 14), Some(([32, 40, 64], 255)));
        assert_eq!(c.pixel(57, 5), Some(([0, 0, 0], 0)), "nothing else is painted without text");
        // At alpha 0.5 the cover stays opaque: plate_rgb is the observed (already faded) plate
        // colour, and a half-transparent cover would let the faded tag show through.
        let mut c2 = Canvas::new(rect(0, 0, 200, 40));
        paint_card(&mut c2, &card(vec![band(0, 5, None)], 0.5, true), &[], false);
        assert_eq!(c2.pixel(20, 8), Some(([32, 40, 64], 255)));
        // An empty tag rectangle paints nothing.
        let mut b = band(0, 5, None);
        b.tag_rect = rect(10, 5, 0, 10);
        let mut c3 = Canvas::new(rect(0, 0, 200, 40));
        assert_eq!(paint_card(&mut c3, &card(vec![b], 1.0, true), &[], false).tag_px, 0);
    }

    #[test]
    fn mask_is_respected() {
        // Left half of the plate visible, right half covered (tooltip).
        let mut bits = vec![0u8; 500];
        for y in 0..10 {
            for x in 0..23 {
                bits[y * 50 + x] = 255; // client x 57..79
            }
        }
        let k = card(vec![band(0, 5, Some(Mask { rect: rect(57, 5, 50, 10), bits }))], 1.0, true);
        let cov = solid(40, 10);
        let mut c = Canvas::new(rect(0, 0, 200, 40));
        let st = paint_card(&mut c, &k, &[TextDraw { band: 0, cov: &cov, rgb: [255, 255, 255], alpha_mul: 1.0 }], false);
        assert_eq!(st.text_px, 20 * 10, "text only on x 60..79");
        assert_eq!(c.pixel(70, 9).unwrap().1, 255);
        assert_eq!(c.pixel(85, 9).unwrap().1, 0, "covered part untouched");
        assert_eq!(c.pixel(99, 14).unwrap().1, 0);
    }

    #[test]
    fn card_alpha_scales_text() {
        let k = card(vec![band(0, 5, Some(full_mask(5)))], 0.5, true);
        let cov = Coverage { w: 40, h: 10, a: { let mut a = vec![0; 400]; a[5 * 40 + 5] = 255; a } };
        let mut c = Canvas::new(rect(0, 0, 200, 40));
        paint_card(&mut c, &k, &[TextDraw { band: 0, cov: &cov, rgb: [200, 100, 50], alpha_mul: 1.0 }], false);
        // Glyph at text-rect (5,5) = client (65,10); band text_rect.h == cov.h so no vertical offset.
        let (rgb, a) = c.pixel(65, 10).unwrap();
        assert_eq!(a, 128);
        assert_eq!(rgb, [100, 50, 25]);
    }

    #[test]
    fn shadow_is_one_pixel_down_right_at_0_7() {
        let k = card(vec![band(0, 5, Some(full_mask(5)))], 1.0, true);
        let cov = Coverage { w: 40, h: 10, a: { let mut a = vec![0; 400]; a[2 * 40 + 3] = 255; a } };
        let mut c = Canvas::new(rect(0, 0, 200, 40));
        paint_card(&mut c, &k, &[TextDraw { band: 0, cov: &cov, rgb: [255, 255, 255], alpha_mul: 1.0 }], false);
        assert_eq!(c.pixel(63, 7), Some(([255, 255, 255], 255)), "glyph");
        let (rgb, a) = c.pixel(64, 8).unwrap();
        assert_eq!(rgb, [0, 0, 0], "black shadow");
        assert!((178..=179).contains(&a), "shadow at 0.7: {a}");
        assert_eq!(c.pixel(62, 6).unwrap().1, 0);
    }

    #[test]
    fn gap_fill_needs_both_masks() {
        // Row 0 at y 5..14, row 1 at y 17..26, gap y 15..16.
        let mut above = vec![0u8; 500];
        let mut below = vec![0u8; 500];
        for y in 0..10 {
            for x in 0..20 {
                above[y * 50 + x] = 255; // x 57..76
            }
            for x in 10..30 {
                below[y * 50 + x] = 255; // x 67..86
            }
        }
        let mut k = card(
            vec![band(0, 5, Some(Mask { rect: rect(57, 5, 50, 10), bits: above })), band(1, 17, Some(Mask { rect: rect(57, 17, 50, 10), bits: below }))],
            1.0,
            true,
        );
        k.gaps = vec![rect(57, 15, 50, 2)];
        let mut c = Canvas::new(rect(0, 0, 200, 40));
        let st = paint_card(&mut c, &k, &[], false);
        assert_eq!(st.fill_px, 10 * 2, "x 67..76 only");
        assert_eq!(c.pixel(70, 15), Some(([32, 40, 64], 255)));
        assert_eq!(c.pixel(70, 16), Some(([32, 40, 64], 255)));
        assert_eq!(c.pixel(60, 15).unwrap().1, 0);
        assert_eq!(c.pixel(80, 16).unwrap().1, 0);
    }

    #[test]
    fn no_plate_mode_fills_the_text_rect() {
        let mut k = card(vec![band(0, 5, None), band(1, 17, None)], 1.0, false);
        k.plate_rgb = [10, 20, 30];
        k.gaps = vec![rect(60, 15, 40, 2)];
        let mut c = Canvas::new(rect(0, 0, 200, 40));
        let st = paint_card(&mut c, &k, &[], false);
        assert_eq!(st.fill_px, 40 * 10 * 2 + 40 * 2);
        let (_, a) = c.pixel(80, 10).unwrap();
        assert_eq!(a, 235, "0.92 x 255");
        assert_eq!(c.pixel(80, 16).unwrap().1, 235, "gap filled too");
    }

    #[test]
    fn diag_outlines() {
        let k = card(vec![band(0, 5, Some(full_mask(5)))], 1.0, true);
        let mut c = Canvas::new(rect(0, 0, 200, 40));
        paint_card(&mut c, &k, &[], true);
        assert_eq!(c.pixel(60, 5), Some(([0, 255, 255], 255)));
        assert_eq!(c.pixel(70, 9).unwrap().1, 0, "inside the text rect stays clear");
    }

    #[test]
    fn text_is_vertically_centred() {
        let k = card(vec![band(0, 5, Some(full_mask(5)))], 1.0, true);
        let cov = solid(4, 4); // 4 px tall in a 10 px band: rows 8..11
        let mut c = Canvas::new(rect(0, 0, 200, 40));
        paint_card(&mut c, &k, &[TextDraw { band: 0, cov: &cov, rgb: [255, 255, 255], alpha_mul: 1.0 }], false);
        assert_eq!(c.pixel(61, 7).unwrap().1, 0);
        assert_eq!(c.pixel(61, 8).unwrap().1, 255);
        assert_eq!(c.pixel(61, 11).unwrap().1, 255);
    }
}
