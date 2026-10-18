//! Display curve correction for the tag scan and the cards.
//!
//! The tag reader and the plate rules assume a linear capture: a pixel on a block edge is a
//! linear blend of its neighbours, and a faded plate reads `lo + d * PLATE / 255`. A display curve
//! between the game and the capture (the game's Gamma setting, contrast, brightness) bends every
//! channel, so those blends no longer add up: measured on the E2E rig at Gamma 2.2, after the first
//! scroll 4 of 8 tags were refused (blended edge pixels off the blend line) and no plate passed,
//! while the exact inverse curve brought back all 8 tags and all plates.
//!
//! The strip's calibration blocks carry the curve: the addon draws levels 0, 104, 146 and 255 on
//! every channel and the decoder measures them (`strip::Calib::levels`). Per channel, a curve
//! `out = black + (white - black) * (in / 255) ^ e` is fitted to the two middle levels, and its
//! inverse is applied to the scan area before `tags::scan` and `cards::build`. Near-linear
//! calibrations (exponent within 0.12 of 1, black and white within 12 of 0 and 255) count as
//! identity and are left alone, so the usual case costs nothing.

use crate::image::Bgra;

/// The drawn reference levels of the strip calibration blocks.
pub const DRAWN: [f32; 4] = [0.0, 104.0, 146.0, 255.0];
/// Largest exponent distance from 1 that still counts as linear.
pub const IDENTITY_EXP: f32 = 0.12;
/// Largest black (or 255 - white) that still counts as linear.
pub const IDENTITY_ENDS: f32 = 12.0;

/// A fitted display curve and its inverse lookup table.
#[derive(Clone, Debug)]
pub struct Tone {
    exp: [f32; 3],
    black: [f32; 3],
    white: [f32; 3],
    inv: [[u8; 256]; 3],
    identity: bool,
}

impl Default for Tone {
    fn default() -> Self {
        Tone::identity()
    }
}

impl Tone {
    /// The linear capture (nothing to undo).
    pub fn identity() -> Tone {
        let mut inv = [[0u8; 256]; 3];
        for t in inv.iter_mut() {
            for (v, o) in t.iter_mut().enumerate() {
                *o = v as u8;
            }
        }
        Tone { exp: [1.0; 3], black: [0.0; 3], white: [255.0; 3], inv, identity: true }
    }

    /// Fits the curve to the four measured levels (black, 104, 146, white) of each channel.
    pub fn from_levels(levels: &[[f32; 3]; 4]) -> Tone {
        let mut exp = [1.0f32; 3];
        let mut black = [0.0f32; 3];
        let mut white = [255.0f32; 3];
        let mut identity = true;
        for c in 0..3 {
            let (b, w) = (levels[0][c], levels[3][c]);
            if !(b.is_finite() && w.is_finite()) || w - b < 32.0 {
                return Tone::identity();
            }
            let mut sum = 0.0f32;
            let mut n = 0.0f32;
            for k in 1..3 {
                let t = ((levels[k][c] - b) / (w - b)).clamp(0.01, 0.99);
                let x = DRAWN[k] / 255.0;
                sum += t.ln() / x.ln();
                n += 1.0;
            }
            let e = (sum / n).clamp(0.25, 4.0);
            exp[c] = e;
            black[c] = b;
            white[c] = w;
            if (e - 1.0).abs() > IDENTITY_EXP || b > IDENTITY_ENDS || 255.0 - w > IDENTITY_ENDS {
                identity = false;
            }
        }
        if identity {
            return Tone::identity();
        }
        let mut inv = [[0u8; 256]; 3];
        for c in 0..3 {
            for v in 0..256usize {
                let t = ((v as f32 - black[c]) / (white[c] - black[c])).clamp(0.0, 1.0);
                inv[c][v] = (255.0 * t.powf(1.0 / exp[c])).round().clamp(0.0, 255.0) as u8;
            }
        }
        Tone { exp, black, white, inv, identity: false }
    }

    pub fn is_identity(&self) -> bool {
        self.identity
    }

    /// Fitted exponent per channel (R, G, B): `out ~ in ^ e`; above 1 darkens, below 1 brightens.
    pub fn exponents(&self) -> [f32; 3] {
        self.exp
    }

    /// True when both curves map every level within 2 of each other (no need to rebuild).
    pub fn close_to(&self, other: &Tone) -> bool {
        if self.identity || other.identity {
            return self.identity == other.identity;
        }
        (0..3).all(|c| (0..256).all(|v| self.inv[c][v].abs_diff(other.inv[c][v]) <= 2))
    }

    /// The colour the game drew, from a captured `[R, G, B]`.
    pub fn linear(&self, rgb: [u8; 3]) -> [u8; 3] {
        [self.inv[0][rgb[0] as usize], self.inv[1][rgb[1] as usize], self.inv[2][rgb[2] as usize]]
    }

    /// The colour a capture shows for a drawn `[R, G, B]` (the forward curve).
    pub fn display(&self, rgb: [u8; 3]) -> [u8; 3] {
        if self.identity {
            return rgb;
        }
        let f = |c: usize| {
            let t = (f32::from(rgb[c]) / 255.0).powf(self.exp[c]);
            (self.black[c] + (self.white[c] - self.black[c]) * t).round().clamp(0.0, 255.0) as u8
        };
        [f(0), f(1), f(2)]
    }

    /// Copies `src` through the inverse curve into `dst` (tightly packed BGRA) and returns a view
    /// of it at the same client position.
    pub fn apply<'b>(&self, src: &Bgra, dst: &'b mut Vec<u8>) -> Bgra<'b> {
        let (w, h) = (src.width as usize, src.height as usize);
        dst.clear();
        dst.resize(w * h * 4, 0);
        let (ir, ig, ib) = (&self.inv[0], &self.inv[1], &self.inv[2]);
        for y in 0..h {
            let s = &src.px[y * src.stride as usize..y * src.stride as usize + w * 4];
            let d = &mut dst[y * w * 4..(y + 1) * w * 4];
            for (sp, dp) in s.chunks_exact(4).zip(d.chunks_exact_mut(4)) {
                dp[0] = ib[sp[0] as usize];
                dp[1] = ig[sp[1] as usize];
                dp[2] = ir[sp[2] as usize];
                dp[3] = sp[3];
            }
        }
        Bgra { px: dst, width: src.width, height: src.height, stride: src.width * 4, origin_x: src.origin_x, origin_y: src.origin_y }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn levels_of(f: impl Fn(f32) -> f32) -> [[f32; 3]; 4] {
        std::array::from_fn(|k| [f(DRAWN[k]); 3])
    }

    #[test]
    fn nominal_levels_are_identity() {
        assert!(Tone::from_levels(&levels_of(|x| x)).is_identity());
        // capture noise on the levels stays identity
        assert!(Tone::from_levels(&levels_of(|x| (x + 3.0).min(255.0))).is_identity());
    }

    #[test]
    fn a_gamma_curve_is_undone() {
        for g in [2.2f32, 1.6, 0.4545] {
            let fwd = |x: f32| 255.0 * (x / 255.0).powf(1.0 / g);
            let t = Tone::from_levels(&levels_of(|x| fwd(x).round()));
            assert!(!t.is_identity(), "gamma {g}");
            assert!((t.exponents()[0] - 1.0 / g).abs() < 0.03, "gamma {g}: {:?}", t.exponents());
            // the drawn colours of the bridge come back within a few levels (dark levels of a
            // darkening curve are coarsely quantized by the capture)
            for v in [0u8, 32, 40, 64, 104, 146, 200, 255] {
                let shown = fwd(f32::from(v)).round() as u8;
                let back = t.linear([shown; 3])[0];
                let tol = if g < 1.0 && v < 70 { 8 } else { 3 };
                assert!(back.abs_diff(v) <= tol, "gamma {g}: {v} -> {shown} -> {back}");
                assert!(t.display([v; 3])[0].abs_diff(shown) <= 2, "gamma {g}: forward {v}");
            }
        }
    }

    #[test]
    fn apply_keeps_geometry_and_alpha() {
        let t = Tone::from_levels(&levels_of(|x| (255.0 * (x / 255.0).powf(1.0 / 2.2)).round()));
        // 2 x 2 image with a padded stride
        let px = vec![10, 20, 30, 255, 40, 50, 60, 255, 0, 0, 70, 80, 90, 7, 100, 110, 120, 9, 0, 0];
        let src = Bgra { px: &px, width: 2, height: 2, stride: 10, origin_x: 5, origin_y: 6 };
        let mut buf = Vec::new();
        let out = t.apply(&src, &mut buf);
        assert_eq!((out.width, out.height, out.stride, out.origin_x, out.origin_y), (2, 2, 8, 5, 6));
        assert_eq!(out.rgb(6, 7), Some(t.linear([120, 110, 100])));
        assert_eq!(buf[3], 255);
        assert_eq!(buf[15], 9);
    }
}
