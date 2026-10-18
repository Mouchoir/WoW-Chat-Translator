//! Per-call cost of the bridge on synthetic 1920x1080 and 2560x1440 frames (P2 acceptance (c)).
//! Run in release mode: `cargo test --offline --release --test perf -- --ignored --nocapture`.
//! Each frame has the strip of a typical TRANSLATE (dense), a chat frame at the bottom left with
//! text lines and two cards of 3 rows, over a random world. Reported: mean microseconds per call
//! for `strip::decode_strip` (strip shown and absent), `tags::scan` (the VIEW area, and the
//! discovery area: the whole client below row 16) and `cards::build`.

use std::hint::black_box;
use std::time::Instant;

use tp_bridge::msg::View;
use tp_bridge::strip::{self, Mode, StripRead};
use tp_bridge::synth::{self, Canvas, Rng, TagDraw};
use tp_bridge::{cards, image, tags};

struct Frame {
    full: synth::Capture,
    absent: synth::Capture,
    view: View,
    w: i32,
    h: i32,
}

fn make_frame(rng: &mut Rng, w: i32, h: i32) -> Frame {
    // the world (a smooth field is slow to compute at full size: tile a 256 px patch)
    let tile = Canvas::world(rng, 256, 256);
    let mut cv = Canvas::new(w, h, [0.0; 3]);
    for y in 0..h {
        for x in 0..w {
            cv.px[(y * w + x) as usize] = tile.px[((y % 256) * 256 + (x % 256)) as usize];
        }
    }
    let scale = h as f64 / 1080.0;
    let (vx, vw, vh) = ((10.0 * scale) as i32, (430.0 * scale) as i32, (180.0 * scale) as i32);
    let vy = h - vh - (60.0 * scale) as i32;
    let view = View { id: 1, x: vx as u16, y: vy as u16, w: vw as u16, h: vh as u16, font_px: (14.0 * scale) as u8, row_px: (16.0 * scale) as u8 };
    // chat panel: darkened world, text lines, two cards of 3 rows
    for y in vy..vy + vh {
        for x in vx..vx + vw {
            let p = &mut cv.px[(y * w + x) as usize];
            for c in p.iter_mut() {
                *c *= 0.6;
            }
        }
    }
    let font = 14.0 * scale;
    let line = 16.0 * scale;
    let mut y = f64::from(vy) + 2.0;
    let mut card = 0;
    while y + line < f64::from(vy + vh) {
        synth::draw_chat_line(&mut cv, rng, f64::from(vx) + 4.0, y, font, f64::from(vx + vw) - 4.0, 0.05);
        y += line;
        if card < 2 && y > f64::from(vy) + 3.0 * line {
            for row in 0..3u8 {
                synth::draw_tag(&mut cv, &TagDraw {
                    slot: 10 + card,
                    row,
                    sess_bit: 1,
                    x: f64::from(vx) + 4.0 + 18.0 * scale,
                    y,
                    block_w: (6.0 * scale).round(),
                    block_h: font.round(),
                    gap: 0.0,
                    alpha: 1.0,
                    plate_w: f64::from(vw) * 0.7,
                    colors: None,
                });
                y += line;
            }
            card += 1;
        }
    }
    cv.noise(rng, 2);
    let absent = cv.capture();
    // the strip of a typical TRANSLATE (about 200 B) in dense mode
    let msg = synth::random_message(rng, 7, 260);
    let records = synth::chunk_records(1, &msg, strip::max_chunk_data((w / 2) as usize, Mode::Dense));
    let bytes = synth::frame(Mode::Dense, 1, 4321, (w as u16, h as u16), 1, &records);
    let keys = synth::block_keys(&synth::to_symbols(&bytes, Mode::Dense), Mode::Dense);
    synth::draw_strip(&mut cv, &keys, 0.0, 0.0);
    Frame { full: cv.capture(), absent, view, w, h }
}

/// A sub-image of a capture (rows y0..y1, all columns), as the helper grabs it.
fn rows(c: &synth::Capture, y0: i32, y1: i32) -> image::Bgra<'_> {
    let stride = c.w * 4;
    image::Bgra {
        px: &c.buf[(y0 as usize) * stride as usize..(y1 as usize) * stride as usize],
        width: c.w,
        height: (y1 - y0) as u32,
        stride,
        origin_x: 0,
        origin_y: y0,
    }
}

/// Mean microseconds per call: the best of 5 batches of `n` calls (other programs, a game among
/// them, share this PC; the best batch is the least disturbed one).
fn time<T>(n: usize, mut f: impl FnMut() -> T) -> f64 {
    for _ in 0..n.min(20) {
        black_box(f());
    }
    let mut best = f64::MAX;
    for _ in 0..5 {
        let t = Instant::now();
        for _ in 0..n {
            black_box(f());
        }
        best = best.min(t.elapsed().as_secs_f64() * 1e6 / n as f64);
    }
    best
}

#[test]
#[ignore]
fn per_call_cost() {
    let mut rng = Rng::new(0x9E5F);
    for (w, h) in [(1920, 1080), (2560, 1440)] {
        let f = make_frame(&mut rng, w, h);
        let strip_img = rows(&f.full, 0, 8);
        let StripRead::Frame(_) = strip::decode_strip(&strip_img) else { panic!("strip not decoded") };
        let absent_img = rows(&f.absent, 0, 8);
        assert!(matches!(strip::decode_strip(&absent_img), StripRead::Absent));
        let client = rows(&f.full, 0, f.h);
        let area = image::Rect { x: i32::from(f.view.x), y: i32::from(f.view.y), w: i32::from(f.view.w), h: i32::from(f.view.h) };
        let disc = image::Rect { x: 0, y: 16, w: f.w, h: f.h - 16 };
        let mut hits = Vec::new();
        tags::scan(&client, area, None, &mut hits);
        assert_eq!(hits.len(), 6, "6 tag rows in the view");
        let views = [f.view];
        let cards_v = cards::build(&client, &hits, &views, &|_| Some(true), Some(1));
        assert_eq!(cards_v.iter().map(|c| c.bands.len()).sum::<usize>(), 6);
        let t_strip = time(2000, || strip::decode_strip(&strip_img));
        let t_absent = time(20000, || strip::decode_strip(&absent_img));
        let t_scan = time(200, || {
            let mut v = Vec::new();
            tags::scan(&client, area, None, &mut v);
            v.len()
        });
        let t_disc = time(20, || {
            let mut v = Vec::new();
            tags::scan(&client, disc, None, &mut v);
            v.len()
        });
        let t_cards = time(200, || cards::build(&client, &hits, &views, &|_| Some(true), Some(1)).len());
        // the same view with its tags painted over (the chat only)
        let mut bare = synth::Capture { buf: f.full.buf.clone(), w: f.full.w, h: f.full.h, ox: 0, oy: 0 };
        for hh in &hits {
            for y in hh.y_top..=hh.y_bot {
                for x in hh.x0..hh.x1 + 2 {
                    let o = ((y as u32 * bare.w + x as u32) * 4) as usize;
                    bare.buf[o..o + 3].copy_from_slice(&[20, 20, 20]);
                }
            }
        }
        let bare_img = rows(&bare, 0, f.h);
        let t_bare = time(200, || {
            let mut v = Vec::new();
            tags::scan(&bare_img, area, None, &mut v);
            v.len()
        });
        println!("{w}x{h}: tags::scan on the same view without tags {t_bare:.1} us");
        println!(
            "{w}x{h}: decode_strip {t_strip:.1} us (strip shown), {t_absent:.2} us (absent); tags::scan {t_scan:.1} us \
             (view {}x{}), {t_disc:.1} us (discovery {}x{}); cards::build {t_cards:.1} us (6 rows)",
            f.view.w,
            f.view.h,
            disc.w,
            disc.h
        );
    }
}
