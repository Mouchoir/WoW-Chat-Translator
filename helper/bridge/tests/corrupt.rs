//! Corrupted strips are never accepted (P2 acceptance): 200k captures of real frames whose blocks
//! were corrupted the way the pixel channel corrupts them, decoded by `strip::decode_strip`. A
//! frame that passes but differs from every frame drawn is an undetected corruption.
//! Run with: `cargo test --offline --release -- --ignored` (count scales with TP_CORRUPT_SCALE).
//!
//! Corruptions (one per capture, blocks addressed by their colour key r*16 + g*4 + b):
//! - adjacent: 1-20 blocks misread on one channel by one level (dense), a channel flipped between
//!   its two levels (robust), the grey one level up or down (mono);
//! - splice: a second frame of the same session drawn over the strip from a random block on (a
//!   torn capture), the block at the tear sometimes a random colour;
//! - burst: 1-40 consecutive blocks of random colours of the mode's alphabet.

use tp_bridge::strip::{self, decode_strip, Mode, StripRead};
use tp_bridge::synth::{self, Canvas, Rng};

fn scale() -> f64 {
    std::env::var("TP_CORRUPT_SCALE").ok().and_then(|s| s.parse().ok()).unwrap_or(1.0)
}

/// A random frame of `mode` for `session` (synthetic messages): (bytes, block keys).
fn frame(rng: &mut Rng, mode: Mode, session: u16, seq: u8) -> (Vec<u8>, Vec<u8>) {
    let mut records = Vec::new();
    for k in 0..rng.int(1, 3) {
        let req = rng.int(1, 65535) as u16;
        let m = synth::random_message(rng, req, 300);
        let size = strip::max_chunk_data(640, mode).min(400);
        records.extend(synth::chunk_records(k as u16 + 1, &m, size).into_iter().take(1));
    }
    let bytes = synth::frame(mode, seq, session, (1280, 720), 1, &records);
    let keys = synth::block_keys(&synth::to_symbols(&bytes, mode), mode);
    (bytes, keys)
}

/// A random block colour of the mode's alphabet.
fn random_key(rng: &mut Rng, mode: Mode) -> u8 {
    match mode {
        Mode::Dense => rng.int(0, 63) as u8,
        Mode::Robust => {
            let b = rng.int(0, 7) as u8;
            3 * ((b >> 2) & 1) * 16 + 3 * ((b >> 1) & 1) * 4 + 3 * (b & 1)
        }
        Mode::Mono => rng.int(0, 3) as u8 * 21,
    }
}

fn step(rng: &mut Rng, lv: u8) -> u8 {
    match lv {
        0 => 1,
        3 => 2,
        _ => {
            if rng.chance(0.5) {
                lv - 1
            } else {
                lv + 1
            }
        }
    }
}

#[test]
#[ignore]
fn corrupted_frames_are_never_accepted() {
    let mut rng = Rng::new(0xC0AA);
    let n = ((200_000.0 * scale()) as usize).max(100);
    let (mut accepted_true, mut rejected, mut undetected) = (0usize, 0usize, 0usize);
    let mut kinds = [0usize; 3];
    for i in 0..n {
        let mode = Mode::ALL[i % 3];
        let session = rng.int(1, 65535) as u16;
        let seq = rng.int(0, 255) as u8;
        let (a_bytes, a_keys) = frame(&mut rng, mode, session, seq);
        let mut keys = a_keys.clone();
        let mut truth = vec![a_bytes];
        let r = rng.unit();
        if r < 0.5 {
            kinds[0] += 1;
            for _ in 0..rng.int(1, 20) {
                let k = rng.int(0, keys.len() as i64 - 1) as usize;
                let key = keys[k];
                keys[k] = match mode {
                    Mode::Dense => {
                        let sh = 2 * rng.int(0, 2) as u8;
                        let lv = (key >> sh) & 3;
                        (key & !(3 << sh)) | (step(&mut rng, lv) << sh)
                    }
                    Mode::Robust => {
                        let sh = 2 * rng.int(0, 2) as u8;
                        key ^ (3 << sh)
                    }
                    Mode::Mono => step(&mut rng, key & 3) * 21,
                };
            }
        } else if r < 0.8 {
            kinds[1] += 1;
            let seq = rng.int(0, 255) as u8;
            let (b_bytes, b_keys) = frame(&mut rng, mode, session, seq);
            let cut = rng.int(0, keys.len().max(b_keys.len()) as i64 - 1) as usize;
            keys.resize(keys.len().max(b_keys.len()), 0);
            for k in cut..b_keys.len() {
                keys[k] = b_keys[k];
            }
            if rng.chance(0.5) && cut < keys.len() {
                keys[cut] = random_key(&mut rng, mode);
            }
            truth.push(b_bytes);
        } else {
            kinds[2] += 1;
            let p = rng.int(0, keys.len() as i64 - 1) as usize;
            for k in p..(p + rng.int(1, 40) as usize).min(keys.len()) {
                keys[k] = random_key(&mut rng, mode);
            }
        }
        // whatever follows the strip in the capture: random blocks of the alphabet
        for _ in 0..64 {
            keys.push(random_key(&mut rng, mode));
        }
        let w = (keys.len().div_ceil(2) * 2) as i32;
        let mut cv = Canvas::new(w, 8, [rng.uniform(0.0, 255.0) as f32; 3]);
        cv.aa = false;
        synth::draw_strip(&mut cv, &keys, 0.0, 0.0);
        let cap = cv.capture();
        match decode_strip(&cap.bgra()) {
            StripRead::Frame(f) => {
                let same = truth.iter().any(|t| {
                    let w = strip::parse_frame_bytes(t).unwrap();
                    (f.mode, f.seq, f.session, f.chunks.len()) == (w.mode, w.seq, w.session, w.chunks.len())
                        && f.chunks.iter().zip(&w.chunks).all(|(x, y)| {
                            (x.msg_id, x.idx, x.count, x.total, x.msg_crc, &x.data) == (y.msg_id, y.idx, y.count, y.total, y.msg_crc, &y.data)
                        })
                });
                if same {
                    accepted_true += 1;
                } else {
                    undetected += 1;
                }
            }
            _ => rejected += 1,
        }
    }
    println!(
        "corrupted captures {n} (adjacent {}, splice {}, burst {}): rejected {rejected}, decoded as a drawn frame \
         {accepted_true}, undetected {undetected}",
        kinds[0], kinds[1], kinds[2]
    );
    assert_eq!(undetected, 0);
}
