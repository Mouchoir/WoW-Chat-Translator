//! Synthetic captures for tests: strips under colour distortions and chat panels with tags,
//! plates, text and covering rectangles. Compiled for unit tests and with the "synth" feature.
//! Everything is deterministic from a seed; all text is random glyph shapes (no chat text).

use crate::{crc, image, strip::Mode, tags};

// ------------------------------------------------------------------------------------ random
/// SplitMix64: small, fast, deterministic.
#[derive(Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn uniform(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }

    /// Uniform integer in lo..=hi.
    pub fn int(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next_u64() % ((hi - lo + 1) as u64)) as i64
    }

    pub fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }

    pub fn pick<'a, T>(&mut self, s: &'a [T]) -> &'a T {
        &s[self.int(0, s.len() as i64 - 1) as usize]
    }
}

// ------------------------------------------------------------------------------------ canvas
pub type Rgb = [f32; 3];

/// An RGB image in drawn values (0..255 floats), drawn in client coordinates from (ox, oy).
#[derive(Clone)]
pub struct Canvas {
    pub w: i32,
    pub h: i32,
    pub ox: i32,
    pub oy: i32,
    pub px: Vec<Rgb>,
    /// true: box-filtered edges (antialiased); false: pixel-centre rasterization (hard edges).
    pub aa: bool,
}

/// A BGRA capture owned by the test (see [`Capture::bgra`]).
pub struct Capture {
    pub buf: Vec<u8>,
    pub w: u32,
    pub h: u32,
    pub ox: i32,
    pub oy: i32,
}

impl Capture {
    pub fn bgra(&self) -> image::Bgra<'_> {
        image::Bgra { px: &self.buf, width: self.w, height: self.h, stride: self.w * 4, origin_x: self.ox, origin_y: self.oy }
    }
}

impl Canvas {
    pub fn new(w: i32, h: i32, c: Rgb) -> Self {
        Canvas { w, h, ox: 0, oy: 0, px: vec![c; (w * h) as usize], aa: true }
    }

    pub fn at(&self, x: i32, y: i32) -> Rgb {
        self.px[((y - self.oy) * self.w + (x - self.ox)) as usize]
    }

    pub fn set(&mut self, x: i32, y: i32, c: Rgb) {
        let (lx, ly) = (x - self.ox, y - self.oy);
        if lx >= 0 && ly >= 0 && lx < self.w && ly < self.h {
            self.px[(ly * self.w + lx) as usize] = c;
        }
    }

    /// Blends colour `c` at opacity `alpha` over the rectangle [x0, x1) x [y0, y1).
    pub fn rect(&mut self, x0: f64, y0: f64, x1: f64, y1: f64, c: Rgb, alpha: f32) {
        let (ox, oy) = (f64::from(self.ox), f64::from(self.oy));
        let (x0, x1, y0, y1) = (x0 - ox, x1 - ox, y0 - oy, y1 - oy);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let xa = x0.floor().max(0.0) as i32;
        let xb = (x1.ceil() as i32).min(self.w);
        let ya = y0.floor().max(0.0) as i32;
        let yb = (y1.ceil() as i32).min(self.h);
        for y in ya..yb {
            let cy = if self.aa {
                ((y + 1) as f64).min(y1) - (y as f64).max(y0)
            } else {
                f64::from(u8::from(y as f64 + 0.5 >= y0 && (y as f64 + 0.5) < y1))
            };
            if cy <= 0.0 {
                continue;
            }
            for x in xa..xb {
                let cx = if self.aa {
                    ((x + 1) as f64).min(x1) - (x as f64).max(x0)
                } else {
                    f64::from(u8::from(x as f64 + 0.5 >= x0 && (x as f64 + 0.5) < x1))
                };
                let a = (cx * cy) as f32 * alpha;
                if a <= 0.0 {
                    continue;
                }
                let p = &mut self.px[(y * self.w + x) as usize];
                for k in 0..3 {
                    p[k] = p[k] * (1.0 - a) + c[k] * a;
                }
            }
        }
    }

    /// Adds uniform integer noise in [-amp, amp] per pixel and channel.
    pub fn noise(&mut self, rng: &mut Rng, amp: i64) {
        if amp == 0 {
            return;
        }
        let span = (2 * amp + 1) as u64;
        for p in &mut self.px {
            // one draw per pixel: 16 random bits per channel
            let r = rng.next_u64();
            for (k, c) in p.iter_mut().enumerate() {
                *c += (((r >> (16 * k)) & 0xFFFF) % span) as f32 - amp as f32;
            }
        }
    }

    /// Per-channel transfer curve (linear interpolation of the 8-bit table).
    pub fn apply_curve(&mut self, curve: &Curve) {
        for p in &mut self.px {
            for (c, v) in p.iter_mut().enumerate() {
                *v = curve.map(c, *v);
            }
        }
    }

    /// Colour mixing matrix (rows: output channels).
    pub fn apply_mix(&mut self, m: &[[f32; 3]; 3]) {
        for p in &mut self.px {
            let q = *p;
            for (c, row) in m.iter().enumerate() {
                p[c] = row[0] * q[0] + row[1] * q[1] + row[2] * q[2];
            }
        }
    }

    /// Rounds and clamps to a BGRA capture.
    pub fn capture(&self) -> Capture {
        // round half up without a libm call (the values are clamped to 0..255 first)
        let q = |v: f32| (v.clamp(0.0, 255.0) + 0.5) as u8;
        let mut buf = vec![255u8; self.px.len() * 4];
        for (o, p) in buf.chunks_exact_mut(4).zip(&self.px) {
            o[0] = q(p[2]);
            o[1] = q(p[1]);
            o[2] = q(p[0]);
        }
        Capture { buf, w: self.w as u32, h: self.h as u32, ox: self.ox, oy: self.oy }
    }

    /// A smooth random colour field (the game world behind the chat), any hue.
    pub fn world(rng: &mut Rng, w: i32, h: i32) -> Self {
        let mut cv = Canvas::new(w, h, [0.0; 3]);
        let k: Vec<(f64, f64, Rgb)> = (0..4)
            .map(|_| {
                (rng.uniform(0.0, f64::from(w)), rng.uniform(0.0, f64::from(h)), [
                    rng.uniform(0.0, 255.0) as f32,
                    rng.uniform(0.0, 255.0) as f32,
                    rng.uniform(0.0, 255.0) as f32,
                ])
            })
            .collect();
        let scale = rng.uniform(80.0, 400.0);
        for y in 0..h {
            for x in 0..w {
                let mut s = [0f32; 3];
                let mut wsum = 0f32;
                for (kx, ky, c) in &k {
                    let d2 = ((f64::from(x) - kx).powi(2) + (f64::from(y) - ky).powi(2)) / (scale * scale);
                    let wt = (1.0 / (1.0 + d2)) as f32;
                    wsum += wt;
                    for i in 0..3 {
                        s[i] += wt * c[i];
                    }
                }
                cv.px[(y * w + x) as usize] = [s[0] / wsum, s[1] / wsum, s[2] / wsum];
            }
        }
        cv
    }

    /// Copies another canvas into this one at its own origin (clipped).
    pub fn blit(&mut self, other: &Canvas) {
        for y in 0..other.h {
            for x in 0..other.w {
                self.set(other.ox + x, other.oy + y, other.px[(y * other.w + x) as usize]);
            }
        }
    }

    /// Darkens everything by `dark` (0 = none, 1 = black): the chat frame background.
    pub fn darken(&mut self, dark: f32) {
        for p in &mut self.px {
            for c in p.iter_mut() {
                *c *= 1.0 - dark;
            }
        }
    }

    /// A ground of one kind (see [`Ground`]): a base colour with a smooth texture of `amp` levels
    /// (value noise on a `cell` px grid, the same on every channel up to a tint) and per-pixel
    /// noise of +-`noise` levels, like grass or dirt seen through a chat frame.
    pub fn ground(rng: &mut Rng, kind: Ground, w: i32, h: i32, amp: f32, cell: i32, noise: i64) -> Self {
        let base = kind.base(rng);
        let mut cv = Canvas::new(w, h, base);
        let cell = cell.max(2);
        let (gw, gh) = (w / cell + 2, h / cell + 2);
        let grid: Vec<f32> = (0..gw * gh).map(|_| rng.uniform(-1.0, 1.0) as f32).collect();
        let tint = [rng.uniform(0.6, 1.0) as f32, rng.uniform(0.6, 1.0) as f32, rng.uniform(0.6, 1.0) as f32];
        let s = |t: f32| t * t * (3.0 - 2.0 * t);
        for y in 0..h {
            for x in 0..w {
                let (gx, gy) = ((x / cell) as usize, (y / cell) as usize);
                let (fx, fy) = (s((x % cell) as f32 / cell as f32), s((y % cell) as f32 / cell as f32));
                let v = |i: usize, j: usize| grid[(j * gw as usize + i).min(grid.len() - 1)];
                let top = v(gx, gy) + (v(gx + 1, gy) - v(gx, gy)) * fx;
                let bot = v(gx, gy + 1) + (v(gx + 1, gy + 1) - v(gx, gy + 1)) * fx;
                let n = top + (bot - top) * fy;
                let p = &mut cv.px[(y * w + x) as usize];
                for c in 0..3 {
                    p[c] = (p[c] + amp * n * tint[c]).clamp(0.0, 255.0);
                }
            }
        }
        cv.noise(rng, noise);
        cv
    }
}

/// Kinds of ground behind a chat frame, for the fade and cover tests.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ground {
    Green,
    Brown,
    Grey,
    Dark,
    Bright,
    /// Dark blue (night water): the faded plate and a tooltip backdrop look alike over it.
    Blue,
}

impl Ground {
    pub const ALL: [Ground; 6] = [Ground::Green, Ground::Brown, Ground::Grey, Ground::Dark, Ground::Bright, Ground::Blue];

    pub fn base(self, rng: &mut Rng) -> Rgb {
        let u = |rng: &mut Rng, a: f64, b: f64| rng.uniform(a, b) as f32;
        match self {
            Ground::Green => [u(rng, 40.0, 90.0), u(rng, 70.0, 130.0), u(rng, 20.0, 60.0)],
            Ground::Brown => [u(rng, 90.0, 140.0), u(rng, 60.0, 95.0), u(rng, 30.0, 55.0)],
            Ground::Grey => {
                let v = u(rng, 20.0, 200.0);
                [v, v, v]
            }
            Ground::Dark => [u(rng, 0.0, 30.0), u(rng, 0.0, 30.0), u(rng, 0.0, 30.0)],
            Ground::Bright => [u(rng, 150.0, 245.0), u(rng, 150.0, 245.0), u(rng, 150.0, 245.0)],
            Ground::Blue => [u(rng, 10.0, 40.0), u(rng, 20.0, 50.0), u(rng, 50.0, 90.0)],
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Ground::Green => "green",
            Ground::Brown => "brown",
            Ground::Grey => "grey",
            Ground::Dark => "dark",
            Ground::Bright => "bright",
            Ground::Blue => "blue",
        }
    }
}

// ------------------------------------------------------------------------------------ colour curves
/// A per-channel transfer curve: captured value for each drawn 8-bit value.
#[derive(Clone)]
pub struct Curve {
    pub lut: [[f32; 256]; 3],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CurveKind {
    Identity,
    /// The design envelope: gamma 0.45-2.5, contrast 0.7-1.4, brightness -0.08..0.12,
    /// per-channel gains down to 0.45, highlight clipping at 0.8-1.0.
    Envelope,
    /// Beyond the envelope.
    Harsh,
}

impl Curve {
    pub fn identity() -> Self {
        let mut lut = [[0f32; 256]; 3];
        for c in 0..3 {
            for (v, o) in lut[c].iter_mut().enumerate() {
                *o = v as f32;
            }
        }
        Curve { lut }
    }

    /// Port of tools/bridge/curves.py random_curve.
    pub fn random(rng: &mut Rng, kind: CurveKind) -> Self {
        if kind == CurveKind::Identity {
            return Self::identity();
        }
        let env = kind == CurveKind::Envelope;
        let g = if env { rng.uniform(0.45f64.ln(), 2.5f64.ln()).exp() } else { rng.uniform(0.3f64.ln(), 3.5f64.ln()).exp() };
        let contrast = if env { rng.uniform(0.7, 1.4) } else { rng.uniform(0.5, 1.8) };
        let offset = if env { rng.uniform(-0.08, 0.12) } else { rng.uniform(-0.2, 0.25) };
        let gains = if env {
            [rng.uniform(0.85, 1.0), rng.uniform(0.7, 1.0), rng.uniform(0.45, 1.0)]
        } else {
            [rng.uniform(0.5, 1.0), rng.uniform(0.4, 1.0), rng.uniform(0.25, 1.0)]
        };
        let wclip = if env { rng.uniform(0.8, 1.0) } else { rng.uniform(0.6, 1.0) };
        let wig = if env { rng.uniform(0.0, 0.3) } else { rng.uniform(0.0, 0.6) };
        let mut lut = [[0f32; 256]; 3];
        for c in 0..3 {
            let gc = g * rng.uniform(0.93, 1.07);
            let mut knots = [0f64; 9];
            for k in 1..9 {
                knots[k] = knots[k - 1] + rng.uniform(0.3, 1.7);
            }
            let last = knots[8];
            for k in &mut knots {
                *k /= last;
            }
            for v in 0..256 {
                let x = ((v as f64 / 255.0 - 0.5) * contrast + 0.5 + offset).clamp(0.0, 1.0);
                let y = x.powf(gc);
                let pos = y * 8.0;
                let i = (pos.floor() as usize).min(7);
                let w = knots[i] + (knots[i + 1] - knots[i]) * (pos - i as f64);
                let y = (1.0 - wig) * y + wig * w;
                lut[c][v] = (255.0 * wclip.min(gains[c] * y)) as f32;
            }
        }
        Curve { lut }
    }

    pub fn map(&self, c: usize, v: f32) -> f32 {
        let v = v.clamp(0.0, 255.0);
        let i = (v.floor() as usize).min(254);
        let f = v - i as f32;
        self.lut[c][i] * (1.0 - f) + self.lut[c][i + 1] * f
    }

    /// Smallest captured gap between adjacent strip levels over the 3 channels.
    pub fn min_gap(&self) -> f32 {
        let lv = crate::strip::LEVELS;
        let mut g = f32::MAX;
        for c in 0..3 {
            for k in 0..3 {
                g = g.min(self.lut[c][lv[k + 1] as usize] - self.lut[c][lv[k] as usize]);
            }
        }
        g
    }
}

/// A grey-preserving colour mixing matrix (rows sum to 1): identity blended with a random
/// row-stochastic matrix by a random weakness factor, like the WoW colourblind filter.
pub fn random_mix(rng: &mut Rng) -> [[f32; 3]; 3] {
    let w = rng.uniform(0.3, 1.0) as f32;
    let mut m = [[0f32; 3]; 3];
    for (i, row) in m.iter_mut().enumerate() {
        let r: Vec<f32> = (0..3).map(|_| (rng.unit() as f32).powi(2) + 1e-3).collect();
        let s: f32 = r.iter().sum();
        for j in 0..3 {
            row[j] = w * r[j] / s + if i == j { 1.0 - w } else { 0.0 };
        }
    }
    m
}

// ------------------------------------------------------------------------------------ strip frames
pub const LEVELS: [u8; 4] = crate::strip::LEVELS;

/// Frame bytes to symbols (most significant bit first, last symbol zero padded).
pub fn to_symbols(fb: &[u8], mode: Mode) -> Vec<u8> {
    let bps = mode.bits();
    let mut out = Vec::with_capacity(fb.len() * 8 / bps + 1);
    let (mut acc, mut n) = (0u32, 0usize);
    for &b in fb {
        acc = (acc << 8) | u32::from(b);
        n += 8;
        while n >= bps {
            n -= bps;
            out.push(((acc >> n) & ((1 << bps) - 1)) as u8);
        }
        acc &= (1 << n) - 1;
    }
    if n > 0 {
        out.push(((acc << (bps - n)) & ((1 << bps) - 1)) as u8);
    }
    out
}

/// Colour keys (level indices r * 16 + g * 4 + b) of every block: calibration then data.
pub fn block_keys(symbols: &[u8], mode: Mode) -> Vec<u8> {
    let mut keys: Vec<u8> = crate::strip::CALIB_LEVELS.iter().map(|&l| l as u8 * 21).collect();
    for &s in symbols {
        keys.push(match mode {
            Mode::Dense => s,
            Mode::Robust => 3 * ((s >> 2) & 1) * 16 + 3 * ((s >> 1) & 1) * 4 + 3 * (s & 1),
            Mode::Mono => s * 21,
        });
    }
    keys
}

pub fn key_rgb(key: u8) -> Rgb {
    let l = |i: u8| f32::from(LEVELS[usize::from(i & 3)]);
    [l(key >> 4), l(key >> 2), l(key)]
}

/// CHUNK records of one message for a chunk size.
pub fn chunk_records(msg_id: u16, msg: &[u8], size: usize) -> Vec<Vec<u8>> {
    let count = msg.len().div_ceil(size).max(1);
    let mcrc = crc::crc32(msg);
    (0..count)
        .map(|i| {
            let data = &msg[(i * size).min(msg.len())..((i + 1) * size).min(msg.len())];
            let mut r = vec![crate::strip::REC_CHUNK];
            r.extend_from_slice(&((crate::strip::CHUNK_FIXED + data.len()) as u16).to_be_bytes());
            r.extend_from_slice(&msg_id.to_be_bytes());
            r.extend_from_slice(&[i as u8, count as u8]);
            r.extend_from_slice(&(msg.len() as u16).to_be_bytes());
            r.extend_from_slice(&mcrc.to_be_bytes());
            r.extend_from_slice(data);
            r
        })
        .collect()
}

/// A complete frame (header, STATUS, records, CRC-32).
pub fn frame(mode: Mode, seq: u8, session: u16, phys: (u16, u16), build: u16, records: &[Vec<u8>]) -> Vec<u8> {
    let mut body = vec![crate::strip::REC_STATUS, 0, 6];
    body.extend_from_slice(&phys.0.to_be_bytes());
    body.extend_from_slice(&phys.1.to_be_bytes());
    body.extend_from_slice(&build.to_be_bytes());
    for r in records {
        body.extend_from_slice(r);
    }
    let mut fb = vec![mode.magic(), crate::strip::VERSION, seq, 0];
    fb.extend_from_slice(&session.to_be_bytes());
    fb.extend_from_slice(&(body.len() as u16).to_be_bytes());
    fb.extend_from_slice(&body);
    let c = crc::crc32(&fb);
    fb.extend_from_slice(&c.to_be_bytes());
    fb
}

/// Draws strip blocks (keys) with the strip's top-left at (x, y) client px.
pub fn draw_strip(cv: &mut Canvas, keys: &[u8], x: f64, y: f64) {
    for (k, &key) in keys.iter().enumerate() {
        let bx = x + 2.0 * (k / 2) as f64;
        let by = y + 2.0 * (k % 2) as f64;
        cv.rect(bx, by, bx + 2.0, by + 2.0, key_rgb(key), 1.0);
    }
}

/// A random message (synthetic content) as the addon would build it.
pub fn random_message(rng: &mut Rng, req: u16, max_len: usize) -> Vec<u8> {
    let r = rng.unit();
    let mut m = Vec::new();
    if r < 0.75 {
        m.push(0x10);
        m.extend_from_slice(&req.to_be_bytes());
        m.extend_from_slice(&[rng.int(0, 126) as u8, rng.int(1, 4) as u8, rng.int(0, 3) as u8]);
        let target: &[u8] = rng.pick(&[&b"en"[..], b"fr", b"de", b"zh-TW", b"ko"]);
        m.push(target.len() as u8);
        m.extend_from_slice(target);
        let n = (rng.int(1, 255) as usize).min(max_len.saturating_sub(40).max(1));
        let text: Vec<u8> = (0..n)
            .map(|_| if rng.chance(0.05) { 2 + rng.int(0, 1) as u8 } else { b' ' + rng.int(0, 94) as u8 })
            .collect();
        m.extend_from_slice(&(text.len() as u16).to_be_bytes());
        m.extend_from_slice(&text);
        let np = rng.int(0, 3) as u8;
        m.push(np);
        for _ in 0..np {
            for len in [rng.int(1, 12), rng.int(1, 30)] {
                m.push(len as u8);
                m.extend((0..len).map(|_| b'a' + rng.int(0, 25) as u8));
            }
        }
    } else if r < 0.85 {
        m.push(0x14);
        let n = rng.int(1, 4) as u8;
        m.push(n);
        for _ in 0..n {
            m.push(rng.int(1, 10) as u8);
            for _ in 0..4 {
                m.extend_from_slice(&(rng.int(0, 2000) as u16).to_be_bytes());
            }
            m.extend_from_slice(&[rng.int(10, 30) as u8, rng.int(12, 36) as u8]);
        }
    } else if r < 0.95 {
        m.push(0x11);
        m.extend_from_slice(&req.to_be_bytes());
        m.push(rng.int(0, 127) as u8);
    } else {
        m.push(0x13);
        m.extend_from_slice(&(rng.int(0, 65535) as u16).to_be_bytes());
    }
    m
}

// ------------------------------------------------------------------------------------ tags
/// How to draw one tag (and its plate) in a chat row.
#[derive(Clone, Debug)]
pub struct TagDraw {
    pub slot: u8,
    pub row: u8,
    pub sess_bit: u8,
    /// Left edge of B0 and top of the blocks (client px, fractional).
    pub x: f64,
    pub y: f64,
    pub block_w: f64,
    pub block_h: f64,
    /// Space between consecutive blocks (and between B7 and the plate).
    pub gap: f64,
    pub alpha: f32,
    /// Plate width (0 = no plate).
    pub plate_w: f64,
    /// Override the 8 block colours (mutation tests).
    pub colors: Option<[Rgb; 8]>,
}

/// Where a tag and its plate ended up.
#[derive(Clone, Debug)]
pub struct TagTruth {
    pub slot: u8,
    pub row: u8,
    pub sess_bit: u8,
    pub x0: f64,
    /// Right edge of B7.
    pub x1: f64,
    pub y0: f64,
    pub y1: f64,
    pub block_w: f64,
    pub alpha: f32,
    /// Plate rectangle (x0, y0, x1, y1), when drawn.
    pub plate: Option<[f64; 4]>,
}

pub fn draw_tag(cv: &mut Canvas, d: &TagDraw) -> TagTruth {
    let cols: [Rgb; 8] = d.colors.unwrap_or_else(|| {
        let c = tags::tag_colors(d.slot, d.row, d.sess_bit);
        let mut o = [[0f32; 3]; 8];
        for i in 0..8 {
            o[i] = [f32::from(c[i][0]), f32::from(c[i][1]), f32::from(c[i][2])];
        }
        o
    });
    let pitch = d.block_w + d.gap;
    for (i, c) in cols.iter().enumerate() {
        let x = d.x + i as f64 * pitch;
        cv.rect(x, d.y, x + d.block_w, d.y + d.block_h, *c, d.alpha);
    }
    let x1 = d.x + 7.0 * pitch + d.block_w;
    let plate = if d.plate_w > 0.0 {
        let px0 = x1 + d.gap;
        let p = tags::PLATE_RGB;
        cv.rect(px0, d.y, px0 + d.plate_w, d.y + d.block_h, [f32::from(p[0]), f32::from(p[1]), f32::from(p[2])], d.alpha);
        Some([px0, d.y, px0 + d.plate_w, d.y + d.block_h])
    } else {
        None
    };
    TagTruth {
        slot: d.slot,
        row: d.row,
        sess_bit: d.sess_bit,
        x0: d.x,
        x1,
        y0: d.y,
        y1: d.y + d.block_h,
        block_w: d.block_w,
        alpha: d.alpha,
        plate,
    }
}

// ------------------------------------------------------------------------------------ chat text
/// Chat, class and item colours (whisper (255,128,255) and Battle.net whisper (0,255,246) are
/// magenta-like and green-ish; Monk (0,255,152) is green-like; Paladin (245,140,186) magenta-like).
pub const CHAT_COLORS: [[u8; 3]; 28] = [
    [255, 255, 255], [255, 64, 64], [255, 128, 255], [0, 255, 246], [170, 170, 255], [64, 255, 64],
    [64, 192, 64], [255, 127, 0], [255, 72, 0], [255, 192, 192], [255, 255, 0], [255, 128, 64],
    [159, 212, 255], [245, 140, 186], [255, 125, 10], [171, 212, 115], [63, 199, 235], [255, 244, 104],
    [0, 112, 221], [135, 136, 238], [198, 155, 109], [196, 30, 58], [0, 255, 152], [51, 147, 127],
    [157, 157, 157], [30, 255, 0], [163, 53, 238], [255, 128, 0],
];

/// Raid target icon colours (star, circle, diamond, triangle, moon, square, cross, skull).
pub const ICON_COLORS: [[u8; 3]; 8] =
    [[255, 230, 40], [255, 140, 20], [200, 70, 230], [60, 210, 40], [180, 200, 230], [40, 120, 255], [230, 40, 30], [235, 235, 235]];

fn rgbf(c: [u8; 3]) -> Rgb {
    [f32::from(c[0]), f32::from(c[1]), f32::from(c[2])]
}

/// A pseudo glyph: 5 x 7 cells made of 1-3 strokes, chosen by `code`.
fn glyph(code: u64) -> [u8; 7] {
    let mut r = Rng::new(code);
    let mut g = [0u8; 7];
    for _ in 0..r.int(1, 3) {
        match r.int(0, 4) {
            0 => {
                let c = r.int(0, 4);
                let (a, b) = (r.int(0, 3), r.int(4, 6));
                for row in &mut g[a as usize..=b as usize] {
                    *row |= 1 << c;
                }
            }
            1 => {
                let row = r.int(0, 6) as usize;
                let (a, b) = (r.int(0, 2), r.int(2, 4));
                for c in a..=b {
                    g[row] |= 1 << c;
                }
            }
            2 => {
                for i in 0..5 {
                    g[(i + r.int(0, 2) as usize).min(6)] |= 1 << i;
                }
            }
            3 => {
                g[2] |= 0b01110;
                g[3] |= 0b10001;
                g[4] |= 0b10001;
                g[5] |= 0b01110;
            }
            _ => {
                let (c, row) = (r.int(0, 3), r.int(0, 5) as usize);
                g[row] |= 0b11 << c;
                g[row + 1] |= 0b11 << c;
            }
        }
    }
    g
}

/// Draws `n` pseudo glyphs from x on a text line whose top is y (font size `size` px).
/// Returns the x after the last glyph.
pub fn draw_text(cv: &mut Canvas, rng: &mut Rng, x: f64, y: f64, size: f64, c: Rgb, alpha: f32, n: usize) -> f64 {
    let cell = size / 8.0;
    let mut x = x;
    let aa = cv.aa;
    cv.aa = true;
    for _ in 0..n {
        if rng.chance(0.18) {
            x += size * 0.3; // space
            continue;
        }
        let g = glyph(rng.next_u64());
        for (row, bits) in g.iter().enumerate() {
            for col in 0..5 {
                if bits & (1 << col) != 0 {
                    let gx = x + col as f64 * cell;
                    let gy = y + size * 0.1 + row as f64 * cell;
                    cv.rect(gx, gy, gx + cell, gy + cell, c, alpha);
                }
            }
        }
        x += cell * 6.0;
    }
    cv.aa = aa;
    x
}

/// A shaded raid-target-like icon (size x size) at (x, y): a filled shape with a vertical
/// light gradient and a dark outline.
pub fn draw_icon(cv: &mut Canvas, x: f64, y: f64, size: f64, kind: usize, alpha: f32) {
    let c = rgbf(ICON_COLORS[kind % 8]);
    let steps = (size.ceil() as i32).max(4);
    for i in 0..steps {
        let t = i as f64 / steps as f64;
        let rowy = y + t * size;
        let half = match kind % 4 {
            0 => size * 0.5,                                 // square-ish
            1 => size * (0.5 - (t - 0.5).abs()),             // diamond
            2 => size * 0.5 * t,                             // triangle
            _ => size * 0.5 * (1.0 - (2.0 * t - 1.0).powi(2)).sqrt(), // circle
        };
        let shade = (1.15 - 0.55 * t) as f32;
        let col = [(c[0] * shade).min(255.0), (c[1] * shade).min(255.0), (c[2] * shade).min(255.0)];
        let cx = x + size / 2.0;
        cv.rect(cx - half - 1.0, rowy, cx + half + 1.0, rowy + size / steps as f64, [10.0, 10.0, 10.0], alpha);
        cv.rect(cx - half, rowy, cx + half, rowy + size / steps as f64, col, alpha);
    }
}

/// One random chat line: words in one colour, sometimes item links (another colour) and icons.
pub fn draw_chat_line(cv: &mut Canvas, rng: &mut Rng, x: f64, y: f64, size: f64, max_x: f64, icons_p: f64) {
    let mut col = rgbf(*rng.pick(&CHAT_COLORS));
    let alpha = rng.uniform(0.3, 1.0) as f32;
    let mut x = x;
    while x < max_x - size {
        let r = rng.unit();
        if r < icons_p {
            let n = rng.int(1, 8);
            for _ in 0..n {
                draw_icon(cv, x, y, size, rng.int(0, 7) as usize, alpha);
                x += size + rng.uniform(0.0, 2.0);
            }
        } else if r < icons_p + 0.15 {
            let link = rgbf(*rng.pick(&CHAT_COLORS));
            let n = rng.int(4, 14) as usize;
            x = draw_text(cv, rng, x, y, size, link, alpha, n);
        } else {
            if rng.chance(0.1) {
                col = rgbf(*rng.pick(&CHAT_COLORS));
            }
            let n = rng.int(2, 9) as usize;
            x = draw_text(cv, rng, x, y, size, col, alpha, n);
        }
        x += size * 0.3;
    }
}

// ------------------------------------------------------------------------------------ chat panels
#[derive(Clone, Debug)]
pub struct PanelOpts {
    pub w: i32,
    pub h: i32,
    /// Draw tag rows (cards); false gives a tag-free panel.
    pub tags: bool,
    pub alpha: (f64, f64),
    pub block_w: (f64, f64),
    pub gap: (f64, f64),
    /// Cut the panel's first rows at the top (the chat frame clips its lines).
    pub clip_top: bool,
    /// Probability that a chat text element is a run of raid icons.
    pub icons_p: f64,
    /// Number of covering rectangles (tooltips) over the panel.
    pub covers: usize,
}

impl Default for PanelOpts {
    fn default() -> Self {
        PanelOpts {
            w: 480,
            h: 220,
            tags: true,
            alpha: (0.3, 1.0),
            block_w: (4.0, 12.0),
            gap: (0.0, 2.0),
            clip_top: false,
            icons_p: 0.05,
            covers: 0,
        }
    }
}

/// A chat panel capture with its truth.
pub struct Panel {
    pub canvas: Canvas,
    pub view: image::Rect,
    pub tags: Vec<TagTruth>,
    /// Covering rectangles (x0, y0, x1, y1).
    pub covers: Vec<[f64; 4]>,
}

/// A tooltip-like covering rectangle: the default tooltip backdrop (0.09, 0.09, 0.19) at 0.95
/// over what is below, framed by a 2 px light border.
pub fn draw_cover(cv: &mut Canvas, r: [f64; 4]) {
    let border = [200.0, 200.0, 200.0];
    cv.rect(r[0] + 2.0, r[1] + 2.0, r[2] - 2.0, r[3] - 2.0, [23.0, 23.0, 48.0], 0.95);
    cv.rect(r[0], r[1], r[2], r[1] + 2.0, border, 1.0);
    cv.rect(r[0], r[3] - 2.0, r[2], r[3], border, 1.0);
    cv.rect(r[0], r[1] + 2.0, r[0] + 2.0, r[3] - 2.0, border, 1.0);
    cv.rect(r[2] - 2.0, r[1] + 2.0, r[2], r[3] - 2.0, border, 1.0);
}

/// A random chat panel at (ox, oy): world, dark chat background, text lines and, when
/// `opts.tags`, cards of 1-4 tag rows (tag + plate) under some lines.
pub fn chat_panel(rng: &mut Rng, ox: i32, oy: i32, opts: &PanelOpts) -> Panel {
    let mut cv = Canvas::world(rng, opts.w, opts.h);
    cv.ox = ox;
    cv.oy = oy;
    cv.darken(rng.uniform(0.25, 1.0) as f32);
    cv.noise(rng, 3);
    cv.aa = rng.chance(0.5);
    let font = rng.uniform(10.0, 22.0);
    let line_h = font * rng.uniform(1.1, 1.35);
    let (fx, fy) = (f64::from(ox), f64::from(oy));
    let mut y = fy + if opts.clip_top { -rng.uniform(0.0, 3.0 * line_h) } else { rng.uniform(0.0, line_h) };
    let mut truths = Vec::new();
    let sess = rng.int(0, 1) as u8;
    while y < fy + f64::from(opts.h) {
        draw_chat_line(&mut cv, rng, fx + 4.0, y, font, fx + f64::from(opts.w) - 4.0, opts.icons_p);
        y += line_h;
        if opts.tags && rng.chance(0.6) {
            let k = rng.int(1, 4) as u8;
            let slot = rng.int(0, 127) as u8;
            let bw = rng.uniform(opts.block_w.0, opts.block_w.1);
            let gap = rng.uniform(opts.gap.0, opts.gap.1);
            let alpha = rng.uniform(opts.alpha.0, opts.alpha.1) as f32;
            let indent = fx + 4.0 + font * 1.3 + rng.uniform(-2.0, 2.0);
            let bh = font.round().max(6.0);
            let row_gap = line_h - bh;
            let plate_w = rng.uniform(80.0, f64::from(opts.w) * 0.8);
            for row in 0..k {
                let t = draw_tag(&mut cv, &TagDraw {
                    slot,
                    row,
                    sess_bit: sess,
                    x: indent,
                    y,
                    block_w: bw,
                    block_h: bh,
                    gap,
                    alpha,
                    plate_w,
                    colors: None,
                });
                truths.push(t);
                y += bh + row_gap.max(0.0);
            }
        }
    }
    let mut covers = Vec::new();
    for _ in 0..opts.covers {
        let w = rng.uniform(60.0, 200.0);
        let h = rng.uniform(20.0, 90.0);
        let x = fx + rng.uniform(0.0, f64::from(opts.w) - w);
        let yy = fy + rng.uniform(0.0, f64::from(opts.h) - h);
        let r = [x, yy, x + w, yy + h];
        draw_cover(&mut cv, r);
        covers.push(r);
    }
    cv.noise(rng, 2);
    Panel { canvas: cv, view: image::Rect { x: ox, y: oy, w: opts.w, h: opts.h }, tags: truths, covers }
}
