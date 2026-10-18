//! Strip decoding under colour distortions, ported from tools/bridge (sim.py, curves.py): gamma
//! 0.45-2.5, contrast, brightness, per-channel gain down to 0.45, highlight clipping (the design
//! envelope), harsher curves for robust mode, grey-preserving colour mixing for mono mode, noise
//! +-6, a 0.5 px offset and tearing. Each category's decode rate must reach the rate of the
//! Python reference decoder measured on the same categories (`python -I tools/bridge/sim.py
//! rates`, numbers in docs/BRIDGE.md), and a torn or distorted capture must never give a frame
//! that differs from what was drawn.

use tp_bridge::strip::{self, decode_strip, Mode, StripRead};
use tp_bridge::synth::{self, Canvas, Curve, CurveKind, Rng};

struct Drawn {
    keys: Vec<u8>,
    bytes: Vec<u8>,
}

fn random_frame(rng: &mut Rng, mode: Mode) -> Drawn {
    let session = rng.int(1, 65535) as u16;
    let mut records = Vec::new();
    for k in 0..rng.int(1, 3) {
        let req = rng.int(1, 65535) as u16;
        let m = synth::random_message(rng, req, 220);
        let size = strip::max_chunk_data(1280, mode).min(300);
        records.extend(synth::chunk_records(k as u16 + 1, &m, size).into_iter().take(1));
    }
    let bytes = synth::frame(mode, rng.int(0, 255) as u8, session, (2560, 1440), 1, &records);
    let keys = synth::block_keys(&synth::to_symbols(&bytes, mode), mode);
    Drawn { keys, bytes }
}

#[derive(Clone, Copy, PartialEq)]
enum Noise {
    Pixel,
    Block,
}

struct Shot {
    mode: Mode,
    curve: Curve,
    mix: Option<[[f32; 3]; 3]>,
    noise: Noise,
    offset: f64,
    tear: bool,
}

/// Draws the strip (and, when torn, a second frame of the same mode over part of it), applies
/// the colour path and decodes. Returns (decoded, decoded but different from every drawn frame).
fn shoot(rng: &mut Rng, s: &Shot) -> (bool, bool) {
    let a = random_frame(rng, s.mode);
    let b = if s.tear { Some(random_frame(rng, s.mode)) } else { None };
    let blocks = a.keys.len().max(b.as_ref().map_or(0, |b| b.keys.len()));
    let w = (blocks.div_ceil(2) * 2 + 40) as i32;
    let base = [rng.uniform(0.0, 255.0) as f32, rng.uniform(0.0, 255.0) as f32, rng.uniform(0.0, 255.0) as f32];
    let mut bg = Canvas::new(w, 8, base);
    bg.noise(rng, 40);
    let mut cv = bg.clone();
    cv.aa = true;
    synth::draw_strip(&mut cv, &a.keys, s.offset, 0.0);
    if let Some(b) = &b {
        let mut cb = bg.clone();
        cb.aa = true;
        synth::draw_strip(&mut cb, &b.keys, s.offset, 0.0);
        if rng.chance(0.3) {
            // a row split (the capture caught a frame change mid-scan), sometimes blended
            let r = rng.int(1, 3) as i32;
            for y in r..8 {
                for x in 0..w {
                    cv.set(x, y, cb.at(x, y));
                }
            }
        } else {
            let cut = rng.int(0, i64::from(w) - 1) as i32;
            for y in 0..8 {
                for x in cut..w {
                    cv.set(x, y, cb.at(x, y));
                }
                if cut > 0 && rng.chance(0.5) {
                    let (p, q) = (cv.at(cut - 1, y), cb.at(cut - 1, y));
                    cv.set(cut - 1, y, [(p[0] + q[0]) / 2.0, (p[1] + q[1]) / 2.0, (p[2] + q[2]) / 2.0]);
                }
            }
        }
    }
    if let Some(m) = &s.mix {
        cv.apply_mix(m);
    }
    cv.apply_curve(&s.curve);
    match s.noise {
        Noise::Pixel => cv.noise(rng, 6),
        Noise::Block => {
            // the same offset over each 2x2 block (harsher than per-pixel noise)
            for by in 0..4 {
                for bx in 0..(w / 2) {
                    let d = [rng.int(-6, 6) as f32, rng.int(-6, 6) as f32, rng.int(-6, 6) as f32];
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let (x, y) = (2 * bx + dx, 2 * by + dy);
                        if y < 8 {
                            let p = cv.at(x, y);
                            cv.set(x, y, [p[0] + d[0], p[1] + d[1], p[2] + d[2]]);
                        }
                    }
                }
            }
        }
    }
    let cap = cv.capture();
    match decode_strip(&cap.bgra()) {
        StripRead::Frame(f) => {
            let same = |d: &Drawn| {
                let w = strip::parse_frame_bytes(&d.bytes).unwrap();
                (f.seq, f.session, f.chunks.len()) == (w.seq, w.session, w.chunks.len())
                    && f.chunks.iter().zip(&w.chunks).all(|(x, y)| (x.msg_id, x.idx, &x.data) == (y.msg_id, y.idx, &y.data))
            };
            let ok = same(&a) || b.as_ref().is_some_and(same);
            (true, !ok)
        }
        _ => (false, false),
    }
}

fn curve_with_gap(rng: &mut Rng, kind: CurveKind, min_gap: f32) -> Curve {
    loop {
        let c = Curve::random(rng, kind);
        if c.min_gap() >= min_gap {
            return c;
        }
    }
}

struct Rate {
    name: &'static str,
    n: usize,
    ok: usize,
    wrong: usize,
}

fn run(name: &'static str, n: usize, seed: u64, mut make: impl FnMut(&mut Rng) -> Shot) -> Rate {
    let mut rng = Rng::new(seed);
    let mut r = Rate { name, n, ok: 0, wrong: 0 };
    for _ in 0..n {
        let s = make(&mut rng);
        let (ok, wrong) = shoot(&mut rng, &s);
        r.ok += usize::from(ok && !wrong);
        r.wrong += usize::from(wrong);
    }
    println!("{:<34} {:>6} captures, decoded {:>8.3}%, wrong frames {}", r.name, r.n, 100.0 * r.ok as f64 / r.n as f64, r.wrong);
    r
}

const N: usize = 4000;

/// Reference decode rates of tools/bridge/proto.py on the same categories (percent), measured
/// with `python -I tools/bridge/sim.py rates`; the Rust decoder may not be worse by more than
/// RATE_SLACK percentage points (sampling noise of 4000 captures).
const PY_DENSE_IDENTITY: f64 = 100.0;
const PY_DENSE_ENVELOPE: f64 = 99.9;
const PY_ROBUST: f64 = 99.5;
const PY_MONO_MIX: f64 = 99.9;
const PY_SUBPX: f64 = 100.0;
const RATE_SLACK: f64 = 0.5;

fn pct(r: &Rate) -> f64 {
    100.0 * r.ok as f64 / r.n as f64
}

#[test]
fn dense_identity() {
    let r = run("dense, identity, noise +-6", N, 11, |_| Shot {
        mode: Mode::Dense,
        curve: Curve::identity(),
        mix: None,
        noise: Noise::Pixel,
        offset: 0.0,
        tear: false,
    });
    assert_eq!(r.wrong, 0);
    assert!(pct(&r) >= PY_DENSE_IDENTITY - RATE_SLACK);
}

#[test]
fn dense_envelope_curves() {
    // the curves auto mode keeps dense for (smallest level gap >= 16; else the weak-signal hint)
    let r = run("dense, envelope curves (gap >= 16)", N, 12, |rng| Shot {
        mode: Mode::Dense,
        curve: curve_with_gap(rng, CurveKind::Envelope, 16.0),
        mix: None,
        noise: Noise::Block,
        offset: 0.0,
        tear: false,
    });
    assert_eq!(r.wrong, 0);
    assert!(pct(&r) >= PY_DENSE_ENVELOPE - RATE_SLACK);
}

#[test]
fn robust_envelope_and_harsh_curves() {
    let r = run("robust, envelope + harsh curves", N, 13, |rng| {
        let kind = if rng.chance(0.5) { CurveKind::Envelope } else { CurveKind::Harsh };
        Shot { mode: Mode::Robust, curve: Curve::random(rng, kind), mix: None, noise: Noise::Block, offset: 0.0, tear: false }
    });
    assert_eq!(r.wrong, 0);
    assert!(pct(&r) >= PY_ROBUST - RATE_SLACK);
}

#[test]
fn mono_colourblind_mixing() {
    let r = run("mono, colour mixing + envelope", N, 14, |rng| Shot {
        mode: Mode::Mono,
        curve: Curve::random(rng, CurveKind::Envelope),
        mix: Some(synth::random_mix(rng)),
        noise: Noise::Block,
        offset: 0.0,
        tear: false,
    });
    assert_eq!(r.wrong, 0);
    assert!(pct(&r) >= PY_MONO_MIX - RATE_SLACK);
}

#[test]
fn half_pixel_offset() {
    let r = run("dense, 0.5 px offset", N, 15, |rng| {
        let curve = if rng.chance(0.5) { Curve::identity() } else { curve_with_gap(rng, CurveKind::Envelope, 16.0) };
        Shot { mode: Mode::Dense, curve, mix: None, noise: Noise::Pixel, offset: 0.5, tear: false }
    });
    assert_eq!(r.wrong, 0);
    assert!(pct(&r) >= PY_SUBPX - RATE_SLACK);
}

#[test]
fn torn_captures_never_give_a_wrong_frame() {
    let mut wrong = 0;
    for (i, mode) in Mode::ALL.into_iter().enumerate() {
        let r = run(["torn, dense", "torn, robust", "torn, mono"][i], N, 16 + i as u64, |_| Shot {
            mode,
            curve: Curve::identity(),
            mix: None,
            noise: Noise::Pixel,
            offset: 0.0,
            tear: true,
        });
        wrong += r.wrong;
    }
    assert_eq!(wrong, 0);
}

#[test]
fn colourblind_mixing_breaks_dense_but_not_mono() {
    // why auto picks mono when the colourblind filter is on
    let mut rng = Rng::new(17);
    let (mut dense, mut mono) = (0, 0);
    for _ in 0..300 {
        let m = synth::random_mix(&mut rng);
        let id = Curve::identity();
        dense += usize::from(shoot(&mut rng, &Shot { mode: Mode::Dense, curve: id.clone(), mix: Some(m), noise: Noise::Pixel, offset: 0.0, tear: false }).0);
        mono += usize::from(shoot(&mut rng, &Shot { mode: Mode::Mono, curve: id, mix: Some(m), noise: Noise::Pixel, offset: 0.0, tear: false }).0);
    }
    println!("colour mixing, 300 captures each: dense decoded {dense}, mono decoded {mono}");
    assert_eq!(mono, 300);
    assert!(dense < 200);
}
