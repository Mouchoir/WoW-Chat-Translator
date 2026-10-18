//! Pixel helpers over a BGRA buffer (the window's DIB): rectangles with sub-pixel coverage, blends,
//! the world background, and the display transform.

use crate::display::Lut;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct IRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl IRect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> IRect {
        IRect { x, y, w, h }
    }

    /// From float edges, rounded to the nearest pixel boundary.
    pub fn from_edges(x0: f64, y0: f64, x1: f64, y1: f64) -> IRect {
        let (a, b, c, d) = (x0.round() as i32, y0.round() as i32, x1.round() as i32, y1.round() as i32);
        IRect { x: a, y: b, w: (c - a).max(0), h: (d - b).max(0) }
    }

    pub fn right(&self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    pub fn intersect(&self, o: &IRect) -> Option<IRect> {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = self.right().min(o.right());
        let y1 = self.bottom().min(o.bottom());
        if x1 > x0 && y1 > y0 {
            Some(IRect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
        } else {
            None
        }
    }

    pub fn union(&self, o: &IRect) -> IRect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        let x0 = self.x.min(o.x);
        let y0 = self.y.min(o.y);
        let x1 = self.right().max(o.right());
        let y1 = self.bottom().max(o.bottom());
        IRect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.right() && y < self.bottom()
    }

    pub fn inflate(&self, d: i32) -> IRect {
        IRect { x: self.x - d, y: self.y - d, w: self.w + 2 * d, h: self.h + 2 * d }
    }

    pub fn to_array(&self) -> [i32; 4] {
        [self.x, self.y, self.w, self.h]
    }
}

/// A BGRA image, rows top to bottom, stride = 4 * w.
pub struct Surface<'a> {
    pub px: &'a mut [u8],
    pub w: i32,
    pub h: i32,
}

impl<'a> Surface<'a> {
    pub fn new(px: &'a mut [u8], w: i32, h: i32) -> Surface<'a> {
        assert!(px.len() >= (w * h * 4) as usize);
        Surface { px, w, h }
    }

    pub fn bounds(&self) -> IRect {
        IRect::new(0, 0, self.w, self.h)
    }

    #[inline]
    fn idx(&self, x: i32, y: i32) -> usize {
        ((y * self.w + x) * 4) as usize
    }

    pub fn get(&self, x: i32, y: i32) -> [u8; 3] {
        let i = self.idx(x, y);
        [self.px[i + 2], self.px[i + 1], self.px[i]]
    }

    #[inline]
    pub fn put(&mut self, x: i32, y: i32, rgb: [u8; 3]) {
        let i = self.idx(x, y);
        self.px[i] = rgb[2];
        self.px[i + 1] = rgb[1];
        self.px[i + 2] = rgb[0];
        self.px[i + 3] = 255;
    }

    /// p = p * (1 - a) + rgb * a
    #[inline]
    pub fn blend(&mut self, x: i32, y: i32, rgb: [u8; 3], a: f64) {
        if a <= 0.0 {
            return;
        }
        if a >= 1.0 {
            self.put(x, y, rgb);
            return;
        }
        // 8.8 fixed point: exact at a = 0 and 1, within one level in between
        let i = self.idx(x, y);
        let k = (a * 256.0).round() as u32;
        for (c, v) in [(0usize, rgb[2]), (1, rgb[1]), (2, rgb[0])] {
            let p = u32::from(self.px[i + c]);
            self.px[i + c] = ((p * (256 - k) + u32::from(v) * k + 128) >> 8) as u8;
        }
        self.px[i + 3] = 255;
    }

    pub fn fill(&mut self, r: IRect, rgb: [u8; 3]) {
        if let Some(r) = r.intersect(&self.bounds()) {
            for y in r.y..r.bottom() {
                for x in r.x..r.right() {
                    self.put(x, y, rgb);
                }
            }
        }
    }

    pub fn blend_rect(&mut self, r: IRect, rgb: [u8; 3], a: f64) {
        if let Some(r) = r.intersect(&self.bounds()) {
            for y in r.y..r.bottom() {
                for x in r.x..r.right() {
                    self.blend(x, y, rgb, a);
                }
            }
        }
    }

    /// A rectangle with float edges: each pixel gets `alpha * covered area` of the colour (what a
    /// texture drawn at a sub-pixel position looks like after filtering). Only inside `clip`.
    pub fn fill_aa(&mut self, x0: f64, y0: f64, x1: f64, y1: f64, rgb: [u8; 3], alpha: f64, clip: IRect) {
        let Some(clip) = clip.intersect(&self.bounds()) else { return };
        if x1 <= x0 || y1 <= y0 || alpha <= 0.0 {
            return;
        }
        let px0 = (x0.floor() as i32).max(clip.x);
        let px1 = (x1.ceil() as i32).min(clip.right());
        let py0 = (y0.floor() as i32).max(clip.y);
        let py1 = (y1.ceil() as i32).min(clip.bottom());
        for y in py0..py1 {
            let cy = (y1.min(f64::from(y) + 1.0) - y0.max(f64::from(y))).clamp(0.0, 1.0);
            for x in px0..px1 {
                let cx = (x1.min(f64::from(x) + 1.0) - x0.max(f64::from(x))).clamp(0.0, 1.0);
                self.blend(x, y, rgb, alpha * cx * cy);
            }
        }
    }

    /// Copies `r` from another buffer of the same size.
    pub fn copy_from(&mut self, src: &[u8], r: IRect) {
        if let Some(r) = r.intersect(&self.bounds()) {
            for y in r.y..r.bottom() {
                let a = self.idx(r.x, y);
                let n = (r.w * 4) as usize;
                self.px[a..a + n].copy_from_slice(&src[a..a + n]);
            }
        }
    }

    /// Applies the display transform to `r` (and makes alpha opaque, GDI text leaves it at 0).
    pub fn apply_lut(&mut self, r: IRect, lut: &Lut) {
        if let Some(r) = r.intersect(&self.bounds()) {
            for y in r.y..r.bottom() {
                let a = self.idx(r.x, y);
                for p in self.px[a..a + (r.w * 4) as usize].chunks_exact_mut(4) {
                    if !lut.identity {
                        p[0] = lut.table[p[0] as usize];
                        p[1] = lut.table[p[1] as usize];
                        p[2] = lut.table[p[2] as usize];
                    }
                    p[3] = 255;
                }
            }
        }
    }
}

fn hash(x: i32, y: i32, seed: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA77) ^ seed.wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^ (h >> 15)
}

/// Smooth value noise in 0..1 with a cell size in px.
fn value_noise(x: i32, y: i32, cell: i32, seed: u32) -> f64 {
    let (cx, cy) = (x.div_euclid(cell), y.div_euclid(cell));
    let (fx, fy) = (f64::from(x.rem_euclid(cell)) / f64::from(cell), f64::from(y.rem_euclid(cell)) / f64::from(cell));
    let v = |i: i32, j: i32| f64::from(hash(cx + i, cy + j, seed) & 0xFFFF) / 65535.0;
    let s = |t: f64| t * t * (3.0 - 2.0 * t);
    let (sx, sy) = (s(fx), s(fy));
    let top = v(0, 0) + (v(1, 0) - v(0, 0)) * sx;
    let bot = v(0, 1) + (v(1, 1) - v(0, 1)) * sx;
    top + (bot - top) * sy
}

/// The game world behind the UI: sky, horizon and ground with soft blobs and +-6 per-pixel noise.
pub fn world_pixel(x: i32, y: i32, w: i32, h: i32) -> [u8; 3] {
    let _ = w;
    let fy = f64::from(y) / f64::from(h.max(1));
    let horizon = 0.42 + 0.05 * (value_noise(x, 0, 300, 7) - 0.5);
    let base: [f64; 3] = if fy < horizon {
        let t = fy / horizon;
        [70.0 + 80.0 * t, 110.0 + 60.0 * t, 160.0 + 20.0 * t]
    } else {
        let t = ((fy - horizon) / (1.0 - horizon)).clamp(0.0, 1.0);
        let n = value_noise(x, y, 90, 11);
        let m = value_noise(x, y, 23, 13);
        [70.0 - 30.0 * t + 40.0 * n + 12.0 * m, 85.0 - 30.0 * t + 35.0 * n + 10.0 * m, 50.0 - 15.0 * t + 15.0 * n]
    };
    let noise = (hash(x, y, 99) % 13) as f64 - 6.0;
    [
        (base[0] + noise).round().clamp(0.0, 255.0) as u8,
        (base[1] + noise).round().clamp(0.0, 255.0) as u8,
        (base[2] + noise).round().clamp(0.0, 255.0) as u8,
    ]
}

/// The world background for a whole client area (BGRA).
pub fn make_world(w: i32, h: i32) -> Vec<u8> {
    let mut v = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let p = world_pixel(x, y, w, h);
            let i = ((y * w + x) * 4) as usize;
            v[i] = p[2];
            v[i + 1] = p[1];
            v[i + 2] = p[0];
            v[i + 3] = 255;
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_ops() {
        let a = IRect::new(0, 0, 10, 10);
        let b = IRect::new(5, 5, 10, 10);
        assert_eq!(a.intersect(&b), Some(IRect::new(5, 5, 5, 5)));
        assert_eq!(a.union(&b), IRect::new(0, 0, 15, 15));
        assert_eq!(IRect::new(0, 0, 2, 2).intersect(&IRect::new(2, 0, 2, 2)), None);
        assert_eq!(IRect::from_edges(1.4, 2.6, 7.5, 9.49), IRect::new(1, 3, 7, 6));
    }

    #[test]
    fn coverage_fill_blends_edges() {
        let mut buf = vec![0u8; 10 * 4 * 4];
        let mut s = Surface::new(&mut buf, 10, 4);
        s.fill_aa(1.5, 0.0, 4.25, 2.0, [200, 100, 0], 1.0, IRect::new(0, 0, 10, 4));
        assert_eq!(s.get(0, 0), [0, 0, 0]);
        assert_eq!(s.get(1, 0), [100, 50, 0]); // half covered
        assert_eq!(s.get(2, 1), [200, 100, 0]);
        assert_eq!(s.get(4, 0), [50, 25, 0]); // quarter covered
        assert_eq!(s.get(2, 2), [0, 0, 0]);
        // clipped
        s.fill_aa(0.0, 0.0, 10.0, 4.0, [255, 255, 255], 1.0, IRect::new(8, 3, 5, 5));
        assert_eq!(s.get(9, 3), [255, 255, 255]);
        assert_eq!(s.get(7, 3), [0, 0, 0]);
    }

    #[test]
    fn world_is_deterministic_and_noisy() {
        let a = world_pixel(100, 900, 1920, 1080);
        assert_eq!(a, world_pixel(100, 900, 1920, 1080));
        let distinct: std::collections::HashSet<[u8; 3]> = (0..50).map(|x| world_pixel(x, 900, 1920, 1080)).collect();
        assert!(distinct.len() > 5);
    }
}
