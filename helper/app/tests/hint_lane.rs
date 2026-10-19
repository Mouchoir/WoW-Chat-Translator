//! Hint lanes, pixel level (user request U1, 2026-10-19): a synthetic capture with a card and the
//! addon's hint lane under it goes through the real tag scan, card build, background sampling,
//! lane search and painting; the overlay is then composed over the capture. Where the lane was,
//! the result must be the ground (chat) or the bubble fill (window), with no gold left; a tooltip
//! over part of the lane must be left untouched; and with no lane (the next chat line right under
//! the card) nothing is painted under the card.

use tp_bridge::cards::{self, Card};
use tp_bridge::image::{Bgra, Rect};
use tp_bridge::msg::View;
use tp_bridge::synth::{self, Canvas as Synth, TagDraw};
use tp_bridge::tags;
use tp_helper::backdrop::{self, CardBg};
use tp_helper::lane::{self, Lane, LaneKind};
use tp_helper::overlay::paint_lane;
use tp_helper::overlay::render::{paint_card, Canvas};

const W: i32 = 480;
const H: i32 = 130;
/// The hint's colour (WoW's NORMAL_FONT_COLOR).
const GOLD: [f32; 3] = [255.0, 209.0, 0.0];
/// The [Download] link's colour (the addon's links, 66bbff).
const LINK: [f32; 3] = [102.0, 187.0, 255.0];
/// Largest per-channel difference from the ground allowed in a painted chat lane (the gradient
/// between two samples against a textured ground).
const DELTA: i32 = 8;
/// The conversation window's bubble fill (`Bridge.BUBBLE_RGB`).
const BUBBLE: [f32; 3] = [36.0, 30.0, 24.0];

/// The chat ground: a world from dark green to brown through a 30% black chat, with a little
/// texture.
fn ground_at(x: i32, y: i32) -> [f32; 3] {
    let t = x as f32 / (W - 1) as f32;
    let tex = ((x * 7 + y * 13) % 7) as f32 - 3.0;
    [0.7 * (40.0 + 70.0 * t + tex), 0.7 * (80.0 - 15.0 * t + tex), 0.7 * (45.0 - 10.0 * t + tex)]
}

/// Chat-like glyphs of `rgb` from x0 to x1 on rows y0..y1 (capitals and descenders reach the
/// line's top and bottom rows), with soft sides and a 1 px black shadow at (+1, +1).
fn glyphs(cv: &mut Synth, x0: f64, x1: f64, y0: i32, y1: i32, rgb: [f32; 3]) {
    let mut x = x0;
    let mut n = 0;
    while x + 4.0 < x1 {
        let w = 1.6 + f64::from(n % 3) * 0.7;
        let (top, bot) = match n % 4 {
            0 => (y0, y1),
            2 => (y0 + 3, y1),
            _ => (y0 + 3, y1 - 3),
        };
        cv.aa = true;
        cv.rect(x + 1.0, f64::from(top) + 1.0, x + w + 1.0, f64::from(bot.min(y1 - 1)) + 1.0, [0.0; 3], 0.6);
        cv.rect(x, f64::from(top), x + w, f64::from(bot), rgb, 1.0);
        x += w + 2.7;
        n += 1;
    }
}

struct Scan {
    capture: synth::Capture,
    card: Card,
    bg: CardBg,
    lane: Option<Lane>,
}

/// Two tag rows (slot 12, K = 2) at fractional positions, 17 px apart, in a chat view of W x H;
/// `extra` draws the lane (or the next line) and covers; `window`: VIEW 200 over a bubble fill.
fn scan(window: bool, extra: impl Fn(&mut Synth)) -> Scan {
    scan_on(window, &ground_at, extra)
}

/// [`scan`] over another chat ground.
fn scan_on(window: bool, ground: &dyn Fn(i32, i32) -> [f32; 3], extra: impl Fn(&mut Synth)) -> Scan {
    let mut cv = Synth::new(W, H, [0.0; 3]);
    for y in 0..H {
        for x in 0..W {
            cv.set(x, y, if window { BUBBLE } else { ground(x, y) });
        }
    }
    cv.aa = true;
    let (x, y0, pitch) = if window { (20.0, 30.0, 17.0) } else { (20.4, 30.3, 17.0) };
    for row in 0..2u8 {
        synth::draw_tag(
            &mut cv,
            &TagDraw {
                slot: if window { 101 } else { 12 },
                row,
                sess_bit: 1,
                x,
                y: y0 + pitch * f64::from(row),
                block_w: 6.0,
                block_h: pitch,
                gap: 0.0,
                alpha: 1.0,
                plate_w: 353.0,
                colors: None,
            },
        );
    }
    extra(&mut cv);
    let capture = cv.capture();
    let img = capture.bgra();
    let id = if window { 200 } else { 1 };
    let view = View { id, x: 0, y: 0, w: W as u16, h: H as u16, font_px: 14, row_px: 17 };
    let area = Rect { x: 0, y: 0, w: W, h: H };
    let mut hits = Vec::new();
    tags::scan(&img, area, None, &mut hits);
    assert_eq!(hits.len(), 2, "both tags found");
    let built = cards::build_with_rows(&img, &hits, &[view], &|_| Some(true), &|_| Some(2), Some(1));
    assert_eq!(built.len(), 1, "one card");
    let card = built[0].clone();
    let bg = backdrop::sample_card(&img, &card, area, false);
    let lane = if window {
        lane::find_window_lane(&img, &card, Some(2), 17.0, area, &bg)
    } else {
        lane::find_chat_lane(&img, &card, Some(2), 17.0, area, &bg)
    };
    Scan { capture, card, bg, lane }
}

/// The overlay (card and lane) composed over the capture, RGB per pixel.
fn compose(s: &Scan) -> Vec<[i32; 3]> {
    let img: Bgra = s.capture.bgra();
    let area = Rect { x: 0, y: 0, w: W, h: H };
    let mut canvas = Canvas::new(area);
    paint_card(&mut canvas, &s.card, &s.bg, &[], false);
    if let Some(l) = &s.lane {
        paint_lane(&mut canvas, l);
    }
    let mut out = Vec::with_capacity((W * H) as usize);
    for y in 0..H {
        for x in 0..W {
            let (pm, a) = canvas.pixel(x, y).unwrap();
            let c = img.rgb(x, y).unwrap();
            let inv = 255 - i32::from(a);
            out.push([0, 1, 2].map(|k| i32::from(pm[k]) + (i32::from(c[k]) * inv + 127) / 255));
        }
    }
    out
}

/// Visibly brighter than the ground (a glyph left over).
fn is_ink(p: [i32; 3], g: [i32; 3]) -> bool {
    (0..3).map(|k| p[k] - g[k]).max().unwrap() > 20
}

/// The addon's inline lane on rows y0..y1: the hint in gold from the tag's x0 (after the row's
/// indent), then "[Download]" in the links' blue, before the plate's end.
fn chat_hint(cv: &mut Synth, y0: i32, y1: i32) {
    glyphs(cv, 21.0, 330.0, y0, y1, GOLD);
    glyphs(cv, 336.0, 400.0, y0, y1, LINK);
}

#[test]
fn a_chat_lane_disappears_into_the_chat_ground() {
    // the hint line on the line right under the card's second row
    let s = scan(false, |cv| chat_hint(cv, 64, 81));
    let l = s.lane.as_ref().expect("chat lane found");
    assert_eq!(l.kind, LaneKind::Chat);
    // 17 rows, or 18 with the row the card's band does not hold (a fractional line position)
    assert!((l.rect.y - 64).abs() <= 1 && (17..=18).contains(&l.rect.h), "{:?}", l.rect);
    assert!(l.passing() as f32 >= 0.95 * l.cols.len() as f32, "{} of {}", l.passing(), l.cols.len());
    let out = compose(&s);
    let (mut n, mut worst) = (0, 0);
    for y in l.rect.y..l.rect.y + l.rect.h {
        for (i, ok) in l.cols.iter().enumerate() {
            let x = l.rect.x + i as i32;
            if !ok {
                continue;
            }
            let g = ground_at(x, y).map(|v| (v + 0.5) as i32);
            let p = out[(y * W + x) as usize];
            let d = (0..3).map(|k| (p[k] - g[k]).abs()).max().unwrap();
            worst = worst.max(d);
            assert!(d <= DELTA, "({x},{y}): {p:?} vs ground {g:?}");
            assert!(!is_ink(p, g), "hint left at ({x},{y})");
            n += 1;
        }
    }
    println!("chat lane: {n} pixels painted, worst difference from the ground {worst}");
}

/// A busy world behind a transparent chat: per-pixel texture of +-20 levels and a darker trunk
/// 40 px wide that neither background sample sees (integration review, 2026-10-19; the lane
/// unit tests go to +-30 and +-40 on synthetic glyphs).
fn busy_ground(x: i32, y: i32) -> [f32; 3] {
    let mut h = (x as u32).wrapping_mul(374_761_393) ^ (y as u32).wrapping_mul(668_265_263);
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    let n = (h % 41) as f32 - 20.0;
    let dark = if (200..240).contains(&x) { -35.0 } else { 0.0 };
    ground_at(x, y).map(|v| (v + 25.0 + n + dark).clamp(0.0, 255.0))
}

#[test]
fn a_chat_lane_over_a_busy_world_disappears() {
    let s = scan_on(false, &busy_ground, |cv| chat_hint(cv, 64, 81));
    let l = s.lane.as_ref().expect("chat lane found over a busy world");
    assert!(l.passing() as f32 >= 0.95 * l.cols.len() as f32, "{} of {}", l.passing(), l.cols.len());
    let out = compose(&s);
    let near = |p: [i32; 3], c: [f32; 3]| (0..3).all(|k| (p[k] - c[k] as i32).abs() <= 40);
    let mut left = 0;
    for y in 64..81 {
        for x in 21..400 {
            let p = out[(y * W + x) as usize];
            left += usize::from(near(p, GOLD) || near(p, LINK));
        }
    }
    assert_eq!(left, 0, "{left} hint pixels left visible");
}

#[test]
fn a_tooltip_over_a_chat_lane_is_left_untouched() {
    // a tooltip from inside the lane down, over its right part (a grey border, a navy body)
    let tip = [300.0, 68.0, 470.0, 125.0];
    let s = scan(false, |cv| {
        chat_hint(cv, 64, 81);
        cv.aa = false;
        cv.rect(tip[0], tip[1], tip[2], tip[3], [160.0, 160.0, 160.0], 1.0);
        cv.rect(tip[0] + 1.0, tip[1] + 1.0, tip[2] - 1.0, tip[3] - 1.0, [23.0, 23.0, 48.0], 1.0);
    });
    let l = s.lane.as_ref().expect("the clean part is painted");
    let out = compose(&s);
    let img = s.capture.bgra();
    for y in tip[1] as i32..tip[3] as i32 {
        for x in tip[0] as i32..tip[2] as i32 {
            let c = img.rgb(x, y).unwrap().map(i32::from);
            assert_eq!(out[(y * W + x) as usize], c, "tooltip pixel painted at ({x},{y})");
        }
    }
    assert!(l.cols.iter().enumerate().all(|(i, ok)| !ok || l.rect.x + (i as i32) < tip[0] as i32));
}

#[test]
fn the_next_chat_line_under_a_card_is_never_painted() {
    // an older addon (no lane): the next message right under the card, from the chat's left edge
    let s = scan(false, |cv| glyphs(cv, 3.0, 420.0, 64, 81, [255.0, 255.0, 255.0]));
    assert!(s.lane.is_none(), "{:?}", s.lane.as_ref().map(|l| l.rect));
    let out = compose(&s);
    let img = s.capture.bgra();
    for y in 64..81 {
        for x in 0..W {
            assert_eq!(out[(y * W + x) as usize], img.rgb(x, y).unwrap().map(i32::from), "({x},{y}) painted");
        }
    }
}

#[test]
fn a_bubble_lane_is_painted_with_the_fill() {
    // the window: the hint clear of the lane's top and bottom 2 px, from the plate's x
    let s = scan(true, |cv| glyphs(cv, 72.0, 400.0, 66, 78, GOLD));
    let l = s.lane.as_ref().expect("window lane found");
    assert_eq!(l.kind, LaneKind::Window);
    assert_eq!((l.rect.y, l.rect.h), (64, 17));
    assert_eq!(l.passing(), l.cols.len());
    let out = compose(&s);
    for y in l.rect.y..l.rect.y + l.rect.h {
        for x in l.rect.x..l.rect.x + l.rect.w {
            assert_eq!(out[(y * W + x) as usize], [36, 30, 24], "({x},{y})");
        }
    }
}

#[test]
#[ignore = "timing: cargo test --offline --release --test hint_lane -- --ignored --nocapture"]
fn lane_search_cost() {
    // The lane under a 2-row card with its hint, over the textured ground: it runs on every capture
    // whose scan area changed (a world moving behind a translucent chat changes it every time).
    let s = scan(false, |cv| chat_hint(cv, 64, 81));
    let img = s.capture.bgra();
    let area = Rect { x: 0, y: 0, w: W, h: H };
    let n = 2000u32;
    let t0 = std::time::Instant::now();
    let mut found = 0;
    for _ in 0..n {
        found += usize::from(lane::find_chat_lane(&img, &s.card, Some(2), 17.0, area, &s.bg).is_some());
    }
    let t_chat = t0.elapsed().as_secs_f64() * 1e6 / f64::from(n);
    let w = scan(true, |cv| glyphs(cv, 72.0, 400.0, 66, 78, GOLD));
    let wimg = w.capture.bgra();
    let t1 = std::time::Instant::now();
    for _ in 0..n {
        found += usize::from(lane::find_window_lane(&wimg, &w.card, Some(2), 17.0, area, &w.bg).is_some());
    }
    let t_window = t1.elapsed().as_secs_f64() * 1e6 / f64::from(n);
    // over a busy world the gradient fails some columns: the local ground is made too
    let b = scan_on(false, &busy_ground, |cv| chat_hint(cv, 64, 81));
    let bimg = b.capture.bgra();
    let t2 = std::time::Instant::now();
    for _ in 0..n {
        found += usize::from(lane::find_chat_lane(&bimg, &b.card, Some(2), 17.0, area, &b.bg).is_some());
    }
    let t_busy = t2.elapsed().as_secs_f64() * 1e6 / f64::from(n);
    assert_eq!(found, 3 * n as usize);
    println!("lane search, {} px wide: chat {t_chat:.1} us, window {t_window:.1} us, chat over a busy world {t_busy:.1} us", s.lane.as_ref().map_or(0, |l| l.rect.w));
}
