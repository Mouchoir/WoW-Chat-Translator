//! Row tags on synthetic chat panels (design P2 acceptance):
//! - recall: panels with dark 25-100% backgrounds over a random world, antialiased text in chat,
//!   class and item colours (including whisper (255,128,255) and Battle.net (0,255,246)), raid
//!   icons, tags with blocks of 4-12 px, gaps of 0-2 px, alpha 0.3-1.0, an indent offset of
//!   +-2 px, fractional positions with hard or antialiased edges, top clipping and two frames:
//!   every visible tag must be found with the right identity (100%), nothing else reported;
//! - false hits: 10k tag-free panels (text, item links, raid icon runs, tooltips): 0 hits;
//! - mutations: 1M tags whose data blocks are redrawn with random digits (uniform blocks, so the
//!   uniformity check passes): a tag is reported exactly when the drawn digits form a valid
//!   code word, always with the drawn identity (0 misdecodes);
//! - random colours: 1M tags with data blocks redrawn in uniformly random RGB colours; the
//!   accepted substitutions are counted and reported (residual risk, see docs/BRIDGE.md).
//! Counts scale with the TP_TAGS_SCALE environment variable (default 1.0).

use tp_bridge::synth::{self, Canvas, PanelOpts, Rng, TagDraw, TagTruth};
use tp_bridge::{crc, image, tags};

fn scale() -> f64 {
    std::env::var("TP_TAGS_SCALE").ok().and_then(|s| s.parse().ok()).unwrap_or(1.0)
}

fn n(base: usize) -> usize {
    ((base as f64) * scale()).max(1.0) as usize
}

const DIGITS: [[u8; 3]; 6] = [[0, 0, 0], [255, 0, 0], [0, 0, 255], [255, 255, 0], [0, 255, 255], [255, 255, 255]];

fn digit(c: [u8; 3]) -> u32 {
    DIGITS.iter().position(|d| *d == c).expect("data block colour") as u32
}

#[test]
fn every_tag_decodes_back() {
    for slot in 0..=tags::TEST_SLOT {
        for row in 0..4u8 {
            for sess in 0..2u8 {
                let c = tags::tag_colors(slot, row, sess);
                let v = c[2..].iter().fold(0u32, |v, &b| v * 6 + digit(b));
                assert!(v < 32768);
                let payload = (v >> 5) as u16;
                assert_eq!(crc::crc5_tag(payload) as u32, v & 31);
                assert_eq!(payload, (u16::from(slot) << 3) | (u16::from(row) << 1) | u16::from(sess));
            }
        }
    }
}

#[test]
fn plate_colour() {
    assert_eq!(tags::PLATE_RGB, [32, 40, 64]);
}

/// A tag counts for recall when its visible height is at least 5 px (so at least 4 full rows),
/// it is not faded under 0.3 and B0 starts inside the scan's reach.
fn eligible(t: &TagTruth, view: image::Rect) -> bool {
    let top = t.y0.max(f64::from(view.y));
    let bot = t.y1.min(f64::from(view.y + view.h));
    let reach = view.w.min(200.max((0.35 * view.w as f32) as i32));
    bot - top >= 5.0 && t.alpha >= 0.3 && t.x0 >= f64::from(view.x) && t.x0 < f64::from(view.x + reach) - 1.0
        && t.x1 + 8.0 < f64::from(view.x + view.w)
}

#[test]
fn recall_on_synthetic_chat_panels() {
    let mut rng = Rng::new(0x7A65);
    let panels = n(3000);
    let (mut want, mut found, mut wrong, mut extra) = (0usize, 0usize, 0usize, 0usize);
    let mut by_alpha = [(0usize, 0usize); 8];
    for i in 0..panels {
        let two = i % 3 == 0;
        let opts = PanelOpts { clip_top: i % 4 == 1, icons_p: 0.05, ..PanelOpts::default() };
        let a = synth::chat_panel(&mut rng, 0, 0, &opts);
        let mut list = vec![a];
        if two {
            let b = synth::chat_panel(&mut rng, opts.w + 24, 40, &opts);
            list.push(b);
        }
        let mut cv = Canvas::new(2 * opts.w + 40, opts.h + 60, [10.0, 10.0, 10.0]);
        for p in &list {
            cv.blit(&p.canvas);
        }
        let cap = cv.capture();
        for p in &list {
            let mut hits = Vec::new();
            tags::scan(&cap.bgra(), p.view, None, &mut hits);
            let mut used = vec![false; hits.len()];
            for t in &p.tags {
                if !eligible(t, p.view) {
                    continue;
                }
                want += 1;
                let bucket = ((t.alpha - 0.3) / 0.0875).clamp(0.0, 7.0) as usize;
                by_alpha[bucket].0 += 1;
                let m = hits.iter().position(|h| {
                    (f64::from(h.x0) - t.x0).abs() <= 1.5 && h.y_top as f64 >= t.y0.floor() - 1.0 && h.y_bot as f64 <= t.y1 + 1.0
                });
                match m {
                    Some(k) => {
                        used[k] = true;
                        let h = &hits[k];
                        if (h.slot, h.row, h.sess_bit) == (t.slot, t.row, t.sess_bit) {
                            found += 1;
                            by_alpha[bucket].1 += 1;
                        } else {
                            wrong += 1;
                        }
                    }
                    None => {
                        if want - found - wrong <= 5 {
                            eprintln!(
                                "missed panel {i}: x0 {:.2} y {:.2}..{:.2} bw {:.2} alpha {:.2} aa {}",
                                t.x0, t.y0, t.y1, t.block_w, t.alpha, p.canvas.aa
                            );
                        }
                    }
                }
            }
            // a hit on a non-eligible (clipped) tag of the right identity is fine; anything else is not
            for (k, h) in hits.iter().enumerate() {
                if used[k] {
                    continue;
                }
                let real = p.tags.iter().any(|t| {
                    (f64::from(h.x0) - t.x0).abs() <= 1.5
                        && (h.slot, h.row, h.sess_bit) == (t.slot, t.row, t.sess_bit)
                        && f64::from(h.y_bot) >= t.y0 - 1.0
                        && f64::from(h.y_top) <= t.y1 + 1.0
                });
                if !real {
                    extra += 1;
                }
            }
        }
    }
    println!(
        "tag recall: panels {panels}, eligible tags {want}, found {found} ({:.4}%), wrong identity {wrong}, \
         unexpected hits {extra}",
        100.0 * found as f64 / want.max(1) as f64
    );
    for (b, (w, f)) in by_alpha.iter().enumerate() {
        println!("  alpha {:.2}-{:.2}: {f}/{w}", 0.3 + b as f64 * 0.0875, 0.3 + (b + 1) as f64 * 0.0875);
    }
    assert!(want > 1000);
    assert_eq!((found, wrong, extra), (want, 0, 0));
}

#[test]
fn no_false_hits_on_tag_free_panels() {
    let mut rng = Rng::new(0xF4153);
    let panels = n(10_000);
    let mut hits_total = 0;
    for i in 0..panels {
        let opts = PanelOpts {
            tags: false,
            icons_p: if i % 2 == 0 { 0.25 } else { 0.05 },
            covers: (i % 3) as usize,
            clip_top: i % 4 == 1,
            ..PanelOpts::default()
        };
        let p = synth::chat_panel(&mut rng, 0, 0, &opts);
        let cap = p.canvas.capture();
        let mut hits = Vec::new();
        tags::scan(&cap.bgra(), p.view, None, &mut hits);
        if !hits.is_empty() {
            eprintln!("false hit on panel {i}: {:?}", hits[0]);
        }
        hits_total += hits.len();
    }
    println!("false hits: {hits_total} over {panels} tag-free panels");
    assert_eq!(hits_total, 0);
}

/// One tag on a small panel; `colors` overrides the 8 block colours.
fn one_tag(rng: &mut Rng, slot: u8, row: u8, sess: u8, colors: Option<[[f32; 3]; 8]>) -> (Canvas, TagTruth) {
    let bw = rng.uniform(4.0, 12.0);
    let gap = rng.uniform(0.0, 2.0);
    let bh = rng.uniform(8.0, 22.0).round();
    let w = (8.0 * (bw + gap) + 90.0) as i32;
    let h = bh as i32 + 12;
    let bg = [rng.uniform(0.0, 70.0) as f32, rng.uniform(0.0, 70.0) as f32, rng.uniform(0.0, 70.0) as f32];
    let mut cv = Canvas::new(w, h, bg);
    cv.aa = rng.chance(0.5);
    let t = synth::draw_tag(&mut cv, &TagDraw {
        slot,
        row,
        sess_bit: sess,
        x: 10.0 + rng.uniform(0.0, 3.0),
        y: 5.0 + rng.uniform(0.0, 1.0),
        block_w: bw,
        block_h: bh,
        gap,
        alpha: rng.uniform(0.3, 1.0) as f32,
        plate_w: 60.0,
        colors,
    });
    cv.noise(rng, 4);
    (cv, t)
}

fn tag_rgb(slot: u8, row: u8, sess: u8) -> [[f32; 3]; 8] {
    let c = tags::tag_colors(slot, row, sess);
    let mut o = [[0f32; 3]; 8];
    for i in 0..8 {
        o[i] = [f32::from(c[i][0]), f32::from(c[i][1]), f32::from(c[i][2])];
    }
    o
}

#[test]
fn random_digit_mutations_never_misdecode() {
    let mut rng = Rng::new(0x3D17);
    let trials = n(1_000_000);
    let (mut valid, mut found, mut missed, mut misdecodes) = (0usize, 0usize, 0usize, 0usize);
    for _ in 0..trials {
        let (slot, row, sess) = (rng.int(0, 127) as u8, rng.int(0, 3) as u8, rng.int(0, 1) as u8);
        let mut cols = tag_rgb(slot, row, sess);
        let k = rng.int(1, 6) as usize;
        for _ in 0..k {
            let b = rng.int(2, 7) as usize;
            let d = DIGITS[rng.int(0, 5) as usize];
            cols[b] = [f32::from(d[0]), f32::from(d[1]), f32::from(d[2])];
        }
        // what is drawn now
        let mut v = 0u32;
        for c in &cols[2..] {
            v = v * 6 + digit([c[0] as u8, c[1] as u8, c[2] as u8]);
        }
        let ok = v < 32768 && u32::from(crc::crc5_tag((v >> 5) as u16)) == v & 31;
        let (cv, _) = one_tag(&mut rng, slot, row, sess, Some(cols));
        let cap = cv.capture();
        let mut hits = Vec::new();
        tags::scan(&cap.bgra(), image::Rect { x: 0, y: 0, w: cv.w, h: cv.h }, None, &mut hits);
        if ok {
            valid += 1;
            let payload = (v >> 5) as u16;
            match hits.as_slice() {
                [] => missed += 1,
                [h] if (u16::from(h.slot) << 3 | u16::from(h.row) << 1 | u16::from(h.sess_bit)) == payload => found += 1,
                _ => misdecodes += 1,
            }
        } else if !hits.is_empty() {
            misdecodes += 1;
        }
    }
    println!(
        "digit mutations: {trials}, drawn valid code words {valid} (found {found}, missed {missed}), \
         invalid {} (all rejected: {}), misdecodes {misdecodes}",
        trials - valid,
        misdecodes == 0
    );
    assert_eq!(misdecodes, 0);
    assert_eq!(missed, 0);
}

#[test]
fn random_colour_substitutions_are_counted() {
    let mut rng = Rng::new(0xC0102);
    let trials = n(1_000_000);
    let (mut uniform_pass, mut accepted_same, mut accepted_other) = (0usize, 0usize, 0usize);
    for _ in 0..trials {
        let (slot, row, sess) = (rng.int(0, 127) as u8, rng.int(0, 3) as u8, rng.int(0, 1) as u8);
        let mut cols = tag_rgb(slot, row, sess);
        let k = rng.int(1, 6) as usize;
        for _ in 0..k {
            let b = rng.int(2, 7) as usize;
            cols[b] = [rng.uniform(0.0, 255.0) as f32, rng.uniform(0.0, 255.0) as f32, rng.uniform(0.0, 255.0) as f32];
        }
        let (cv, _) = one_tag(&mut rng, slot, row, sess, Some(cols));
        uniform_pass += 1; // every block is drawn uniformly, so it passes the uniformity check
        let cap = cv.capture();
        let mut hits = Vec::new();
        tags::scan(&cap.bgra(), image::Rect { x: 0, y: 0, w: cv.w, h: cv.h }, None, &mut hits);
        for h in &hits {
            if (h.slot, h.row, h.sess_bit) == (slot, row, sess) {
                accepted_same += 1;
            } else {
                accepted_other += 1;
            }
        }
    }
    println!(
        "random colour substitutions: {trials} (uniform {uniform_pass}), decoded as the original tag {accepted_same}, \
         decoded as another tag {accepted_other} ({:.2e} per substitution)",
        accepted_other as f64 / trials as f64
    );
    // Not zero by construction: a random colour close to another digit's colour is that digit,
    // and CRC-5 misses 5.5% of single-digit substitutions. Bounded here; see docs/BRIDGE.md.
    assert!((accepted_other as f64) < 1e-3 * trials as f64);
}

#[test]
fn magenta_background_does_not_stretch_the_sync_block() {
    // The literal rule (first magenta-like run of 4..40 px) fails here: the purple world seen
    // through a light chat background is magenta-like too, so B0's run merges with it.
    let mut cv = Canvas::new(260, 30, [150.0, 75.0, 165.0]);
    synth::draw_tag(&mut cv, &TagDraw {
        slot: 42,
        row: 1,
        sess_bit: 1,
        x: 60.0,
        y: 8.0,
        block_w: 6.0,
        block_h: 14.0,
        gap: 1.0,
        alpha: 1.0,
        plate_w: 80.0,
        colors: None,
    });
    let cap = cv.capture();
    let img = cap.bgra();
    let run = (0..200).take_while(|&x| {
        let p = img.rgb(x, 14).unwrap();
        i32::from(p[0]) - i32::from(p[1]) >= 40 && i32::from(p[2]) - i32::from(p[1]) >= 40
    });
    assert!(run.count() > 40, "the background merges with B0 into one magenta-like run");
    let mut hits = Vec::new();
    tags::scan(&img, image::Rect { x: 0, y: 0, w: 260, h: 30 }, None, &mut hits);
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].slot, hits[0].row, hits[0].sess_bit, hits[0].x0), (42, 1, 1, 60));
}

#[test]
fn alpha_follows_the_strip_calibration() {
    let mut cv = Canvas::new(200, 30, [0.0, 0.0, 0.0]);
    synth::draw_tag(&mut cv, &TagDraw {
        slot: 7,
        row: 0,
        sess_bit: 0,
        x: 20.0,
        y: 6.0,
        block_w: 8.0,
        block_h: 14.0,
        gap: 0.0,
        alpha: 0.5,
        plate_w: 60.0,
        colors: None,
    });
    let cap = cv.capture();
    let mut hits = Vec::new();
    tags::scan(&cap.bgra(), image::Rect { x: 0, y: 0, w: 200, h: 30 }, None, &mut hits);
    assert!((hits[0].alpha - 0.5).abs() < 0.02, "alpha {}", hits[0].alpha);
    let mut hits = Vec::new();
    tags::scan(&cap.bgra(), image::Rect { x: 0, y: 0, w: 200, h: 30 }, Some(([200.0; 3], [72.0; 3])), &mut hits);
    assert!((hits[0].alpha - 1.0).abs() < 0.02, "alpha {}", hits[0].alpha);
    assert!((hits[0].block_px - 8.0).abs() < 0.3 && (hits[0].pitch_px - 8.0).abs() < 0.3);
    assert_eq!((hits[0].x0, hits[0].x1, hits[0].y_top, hits[0].y_bot), (20, 84, 6, 19));
}

#[test]
fn narrow_area_inside_a_wider_capture_never_panics() {
    // Review regression: the app grabs one capture covering every VIEW and scans each view
    // separately, so the scan area can end well before the image. A sync pair straddling the
    // area's right edge used to hand read_tag a B1 past the end of its profile (index panic).
    for case in 0..48 {
        let aa = case % 2 == 1;
        let gap = [0.0, 1.0, 2.0][(case / 2) % 3];
        let block_w = [5.6, 6.0, 7.0, 4.3][(case / 6) % 4];
        let shift = (case / 24) as f64 * 0.5;
        let mut cv = Canvas::new(300, 40, [20.0, 20.0, 30.0]);
        cv.aa = aa;
        // B0 ends at or near the area's right edge (x = 100), B1 starts just past it
        synth::draw_tag(&mut cv, &TagDraw {
            slot: 5,
            row: 0,
            sess_bit: 0,
            x: 100.0 - block_w + shift - 0.25,
            y: 10.0,
            block_w,
            block_h: 14.0,
            gap,
            alpha: 1.0,
            plate_w: 120.0,
            colors: None,
        });
        let cap = cv.capture();
        let img = cap.bgra();
        let mut hits = Vec::new();
        tags::scan(&img, image::Rect { x: 0, y: 0, w: 100, h: 40 }, None, &mut hits);
        // the tag is not inside the area, so nothing may be read from it
        assert!(hits.is_empty(), "case {case}: {} hits", hits.len());
        // the same tag with the whole capture as the area is found
        let mut all = Vec::new();
        tags::scan(&img, image::Rect { x: 0, y: 0, w: 300, h: 40 }, None, &mut all);
        assert_eq!(all.len(), 1, "case {case}");
        // diagnose follows the same bound
        let _ = tags::diagnose(&img, image::Rect { x: 0, y: 0, w: 100, h: 40 }, None, 99, 16);
    }
}
