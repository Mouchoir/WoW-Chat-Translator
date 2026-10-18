//! Native look, pixel level: a synthetic chat capture (a coloured gradient ground, tags and plates
//! at fractional positions with antialiased edges) goes through the real tag scan, card build,
//! background sampling and card painting; the overlay is then composed over the capture. Where
//! the reserved rows were, the result must be within a small delta of the ground without them and
//! hold no plate colour; a tooltip over part of a plate must be left untouched. Plates are sized
//! the way the addon sizes them: 0.88 of the room between the tag and the view's right edge.

use tp_bridge::cards;
use tp_bridge::image::Rect;
use tp_bridge::msg::View;
use tp_bridge::synth::{self, Canvas as Synth, TagDraw, TagTruth};
use tp_bridge::tags;
use tp_helper::backdrop;
use tp_helper::overlay::render::{paint_card, Canvas, Coverage, TextDraw};

const W: i32 = 480;
const H: i32 = 120;
/// Largest per-channel difference from the ground allowed after composition.
const DELTA: i32 = 6;
const PLATE: [i32; 3] = [32, 40, 64];
/// The plate the addon draws after a tag ending at x 68.4 in a 480 px view.
const PLATE_W: f64 = 353.4;

/// The ground: dark green on the left to brown on the right (`span` = 1: red rises by 70 levels
/// across the capture; 0.5: by 35), slightly redder lower down.
fn ground_at(x: i32, y: i32, span: f32) -> [f32; 3] {
    let t = span * x as f32 / (W - 1) as f32;
    let (l, r) = ([30.0f32, 70.0, 40.0], [100.0f32, 55.0, 30.0]);
    [l[0] + t * (r[0] - l[0]) + y as f32 / 6.0, l[1] + t * (r[1] - l[1]), l[2] + t * (r[2] - l[2])]
}

struct Out {
    ground: Synth,
    capture: synth::Capture,
    truths: Vec<TagTruth>,
    /// The overlay composed over the capture, RGB per pixel.
    out: Vec<[i32; 3]>,
    text_px: usize,
}

fn run(alpha: f32, span: f32, cover: Option<[f64; 4]>, text: bool) -> Out {
    let mut cv = Synth::new(W, H, [0.0; 3]);
    for y in 0..H {
        for x in 0..W {
            cv.set(x, y, ground_at(x, y, span));
        }
    }
    cv.aa = true;
    let ground = cv.clone();
    let mut truths = Vec::new();
    for row in 0..2u8 {
        truths.push(synth::draw_tag(
            &mut cv,
            &TagDraw {
                slot: 12,
                row,
                sess_bit: 1,
                x: 20.4,
                y: 30.3 + 17.0 * f64::from(row),
                block_w: 6.0,
                block_h: 14.0,
                gap: 0.0,
                alpha,
                plate_w: PLATE_W,
                colors: None,
            },
        ));
    }
    if let Some(r) = cover {
        cv.aa = false;
        cv.rect(r[0], r[1], r[2], r[3], [16.0, 16.0, 24.0], 1.0);
    }
    let capture = cv.capture();
    let img = capture.bgra();
    let view = View { id: 1, x: 0, y: 0, w: W as u16, h: H as u16, font_px: 14, row_px: 17 };
    let area = Rect { x: 0, y: 0, w: W, h: H };
    let mut hits = Vec::new();
    tags::scan(&img, area, None, &mut hits);
    assert_eq!(hits.len(), 2, "both tags found");
    let built = cards::build_with_rows(&img, &hits, &[view], &|_| Some(true), &|_| Some(2), Some(1));
    assert_eq!(built.len(), 1, "one card");
    let card = &built[0];
    assert_eq!(card.bands.len(), 2);
    let bg = backdrop::sample_card(&img, card, area, false);
    if std::env::var("TP_DEBUG_NATIVE").is_ok() {
        for (b, h) in card.bands.iter().zip(&hits) {
            eprintln!("band row {} text {:?} tag {:?} mask {:?} hit y {}..{}", b.row, b.text_rect, b.tag_rect, b.mask.as_ref().map(|m| m.rect), h.y_top, h.y_bot);
        }
        eprintln!("plate_rgb {:?} bg {:?}", card.plate_rgb, bg);
    }
    assert!(bg.bands.iter().all(|b| b.left.is_some()), "{bg:?}");
    let mut canvas = Canvas::new(area);
    let cov = Coverage { w: 200, h: card.bands[0].text_rect.h, a: vec![255; (200 * card.bands[0].text_rect.h) as usize] };
    let texts: Vec<TextDraw> = if text { vec![TextDraw { band: 1, cov: &cov, rgb: [168, 216, 255], alpha_mul: 1.0 }] } else { Vec::new() };
    let st = paint_card(&mut canvas, card, &bg, &texts, false);
    let mut out = Vec::with_capacity((W * H) as usize);
    for y in 0..H {
        for x in 0..W {
            let (pm, a) = canvas.pixel(x, y).unwrap();
            let c = img.rgb(x, y).unwrap();
            let inv = 255 - i32::from(a);
            out.push([0, 1, 2].map(|k| i32::from(pm[k]) + (i32::from(c[k]) * inv + 127) / 255));
        }
    }
    Out { ground, capture, truths, out, text_px: st.text_px }
}

/// The reserved row area of a tag: the tag and its plate plus one pixel all around.
fn row_area(t: &TagTruth) -> (i32, i32, i32, i32) {
    let p = t.plate.unwrap();
    (t.x0.floor() as i32 - 1, p[2].ceil() as i32, t.y0.floor() as i32 - 1, t.y1.ceil() as i32)
}

fn check_rows_disappear(o: &Out, skip: Option<[f64; 4]>) -> (usize, i32) {
    let (mut n, mut worst) = (0usize, 0i32);
    for t in &o.truths {
        let (x0, x1, y0, y1) = row_area(t);
        for y in y0..=y1 {
            for x in x0..=x1 {
                if skip.is_some_and(|r| f64::from(x) >= r[0] - 1.0 && f64::from(x) <= r[2] && f64::from(y) >= r[1] - 1.0 && f64::from(y) <= r[3]) {
                    continue;
                }
                let g = o.ground.at(x, y).map(|v| (v.clamp(0.0, 255.0) + 0.5) as i32);
                let p = o.out[(y * W + x) as usize];
                let d = (0..3).map(|k| (p[k] - g[k]).abs()).max().unwrap();
                worst = worst.max(d);
                assert!(d <= DELTA, "({x},{y}): {p:?} vs ground {g:?}");
                let to_plate = (0..3).map(|k| (p[k] - PLATE[k]).abs()).max().unwrap();
                assert!(to_plate > 10, "plate colour left at ({x},{y}): {p:?}");
                n += 1;
            }
        }
    }
    (n, worst)
}

#[test]
fn plates_and_tags_disappear_into_the_gradient_ground() {
    for (alpha, span) in [(1.0f32, 1.0f32), (1.0, 0.5), (0.6, 0.5)] {
        let o = run(alpha, span, None, false);
        let (n, worst) = check_rows_disappear(&o, None);
        assert!(n > 2 * 300 * 14, "{n} pixels checked");
        println!("alpha {alpha}, span {span}: {n} pixels of the reserved rows, worst difference from the ground {worst}");
    }
}

#[test]
fn a_tooltip_over_a_plate_is_never_painted() {
    // over the right part of row 1's plate and below it
    let r = [250.0, 44.0, 380.0, 90.0];
    let o = run(1.0, 1.0, Some(r), true);
    let img = o.capture.bgra();
    let mut inside = 0;
    // every pixel of the tooltip, its outer rows and columns included
    for y in (r[1] as i32)..(r[3] as i32) {
        for x in (r[0] as i32)..(r[2] as i32) {
            let c = img.rgb(x, y).unwrap().map(i32::from);
            assert_eq!(o.out[(y * W + x) as usize], c, "tooltip pixel painted at ({x},{y})");
            inside += 1;
        }
    }
    assert!(inside > 1000);
    assert!(o.text_px > 0, "the text was drawn on the visible plate");
    // outside the tooltip the rows still disappear (text aside: row 1 holds text, row 0 none)
    let row0 = Out { truths: vec![o.truths[0].clone()], ground: o.ground.clone(), capture: o.capture, out: o.out.clone(), text_px: 0 };
    check_rows_disappear(&row0, Some(r));
}

/// What the overlay paints for a synthetic chat: the real tag scan, card build, background
/// sampling and painting, composed over the capture. `view` is the card's view, `discovery` as
/// in the Helper (no VIEW known: the view is the whole client width).
struct Painted {
    ground: Synth,
    truths: Vec<TagTruth>,
    out: Vec<[i32; 3]>,
    w: i32,
}

#[allow(clippy::too_many_arguments)]
fn paint(w: i32, h: i32, ground: impl Fn(i32, i32) -> [f32; 3], rows: u8, plate_w: f64, cover: Option<([f64; 4], [f32; 3])>, view: View, discovery: bool) -> Painted {
    let mut cv = Synth::new(w, h, [0.0; 3]);
    for y in 0..h {
        for x in 0..w {
            cv.set(x, y, ground(x, y));
        }
    }
    cv.aa = true;
    let g = cv.clone();
    let mut truths = Vec::new();
    for row in 0..rows {
        truths.push(synth::draw_tag(
            &mut cv,
            &TagDraw { slot: 12, row: row % 4, sess_bit: 1, x: 20.0, y: 30.0 + 17.0 * f64::from(row), block_w: 6.0, block_h: 14.0, gap: 0.0, alpha: 1.0, plate_w, colors: None },
        ));
    }
    if let Some((r, c)) = cover {
        cv.aa = false;
        cv.rect(r[0], r[1], r[2], r[3], c, 1.0);
    }
    let capture = cv.capture();
    let img = capture.bgra();
    let area = Rect { x: 0, y: 0, w, h };
    let mut hits = Vec::new();
    tags::scan(&img, area, None, &mut hits);
    assert_eq!(hits.len(), usize::from(rows), "tags found");
    let built = cards::build_with_rows(&img, &hits, &[view], &|_| Some(true), &|_| Some(rows), Some(1));
    assert_eq!(built.len(), 1, "one card");
    let card = &built[0];
    let rect = Rect { x: view.x.into(), y: view.y.into(), w: view.w.into(), h: view.h.into() };
    let bg = backdrop::sample_card(&img, card, rect, discovery);
    let mut canvas = Canvas::new(area);
    paint_card(&mut canvas, card, &bg, &[], false);
    let mut out = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let (pm, a) = canvas.pixel(x, y).unwrap();
            let c = img.rgb(x, y).unwrap();
            let inv = 255 - i32::from(a);
            out.push([0, 1, 2].map(|k| i32::from(pm[k]) + (i32::from(c[k]) * inv + 127) / 255));
        }
    }
    Painted { ground: g, truths, out, w }
}

/// Worst per-channel difference from the ground over the visible plate pixels left of `x_max`.
fn worst_on_plates(p: &Painted, x_max: i32) -> i32 {
    let mut worst = 0;
    for t in &p.truths {
        let pl = t.plate.unwrap();
        for y in (t.y0.ceil() as i32)..(t.y1.floor() as i32) {
            for x in (pl[0].ceil() as i32)..(pl[2].floor() as i32).min(x_max) {
                let g = p.ground.at(x, y).map(|v| (v.clamp(0.0, 255.0) + 0.5) as i32);
                let o = p.out[(y * p.w + x) as usize];
                worst = worst.max((0..3).map(|k| (o[k] - g[k]).abs()).max().unwrap());
            }
        }
    }
    worst
}

#[test]
fn a_frame_over_the_right_end_of_every_row_is_not_taken_for_the_chat() {
    // Review 2026-10-18: a frame (a damage meter docked over the chat, a bag, ItemRefTooltip)
    // covering the right part of every row of a card. Before the fix the far end of the view,
    // under the frame, became the right sample: up to 117 levels off for a parchment frame.
    let (w, h) = (480, 120);
    let chat = [18.0f32, 18.0, 20.0];
    let view = View { id: 1, x: 0, y: 0, w: w as u16, h: h as u16, font_px: 14, row_px: 17 };
    for colour in [[200.0f32, 180.0, 140.0], [23.0, 23.0, 48.0]] {
        for rows in [1u8, 2] {
            let p = paint(w, h, |_, _| chat, rows, PLATE_W, Some(([300.0, 10.0, 480.0, 110.0], colour)), view, false);
            let worst = worst_on_plates(&p, 298);
            println!("frame {colour:?}, {rows} row(s): worst visible plate pixel {worst} levels off the chat");
            assert!(worst <= DELTA, "frame {colour:?}, {rows} row(s): {worst}");
        }
    }
}

#[test]
fn discovery_takes_no_sample_outside_the_chat() {
    // Review 2026-10-18: with no VIEW (a Helper restarted while WoW runs), the view is the whole
    // client and its right edge lies in the game world: 71 levels off before the fix. The chat
    // spans x 8..520 here, a bright world shows on both sides.
    let (w, h) = (1200, 140);
    let ground = |x: i32, _y: i32| if (8..520).contains(&x) { [18.0f32, 18.0, 20.0] } else { [140.0f32, 165.0, 200.0] };
    let chat_view = View { id: 1, x: 8, y: 16, w: 512, h: (h - 16) as u16, font_px: 14, row_px: 17 };
    let plate_w = 0.88 * (520.0 - 68.0) - 9.0;
    let known = paint(w, h, ground, 2, plate_w, None, chat_view, false);
    let disc_view = View { id: 0, x: 0, y: 16, w: w as u16, h: (h - 16) as u16, font_px: 14, row_px: 16 };
    let disc = paint(w, h, ground, 2, plate_w, None, disc_view, true);
    let (a, b) = (worst_on_plates(&known, i32::MAX), worst_on_plates(&disc, i32::MAX));
    println!("VIEW known: worst {a}; discovery: worst {b}");
    assert!(a <= DELTA && b <= DELTA, "{a} {b}");
}

#[test]
fn a_borderless_panel_right_under_a_plate_is_never_painted() {
    // Review 2026-10-18: WoW's default tooltip background (23, 23, 48) right under row 0's plate
    // (whole-pixel edges) over a dark chat passes for a plate-to-chat blend: its top row was
    // painted with the chat colour (269 of 280 px).
    let (w, h) = (480, 120);
    let tip = [100.0, 44.0, 380.0, 110.0];
    let colour = [23.0f32, 23.0, 48.0];
    let view = View { id: 1, x: 0, y: 0, w: w as u16, h: h as u16, font_px: 14, row_px: 17 };
    let p = paint(w, h, |_, _| [12.0, 12.0, 14.0], 2, PLATE_W, Some((tip, colour)), view, false);
    let mut painted = 0;
    for y in 44..110 {
        for x in 100..380 {
            if p.out[(y * w + x) as usize] != colour.map(|c| c as i32) {
                painted += 1;
            }
        }
    }
    assert_eq!(painted, 0, "panel pixels changed");
}

#[test]
#[ignore = "timing: cargo test --offline --release --test native_look -- --ignored --nocapture"]
fn sample_card_cost() {
    // A 6-row card in a 645 px view over a textured ground (review 2026-10-18: 451 us per capture
    // before the column step and the fractional-edge gate, against about 400 us for the card build).
    let (w, h) = (645, 160);
    let mut cv = Synth::new(w, h, [0.0; 3]);
    for y in 0..h {
        for x in 0..w {
            cv.set(x, y, [20.0 + x as f32 / 40.0, 18.0 + (y % 7) as f32, 16.0]);
        }
    }
    cv.aa = true;
    let plate_w = 0.88 * (645.0 - 68.0) - 9.0;
    for row in 0..6u8 {
        synth::draw_tag(&mut cv, &TagDraw { slot: 12, row: row % 4, sess_bit: 1, x: 20.0, y: 30.3 + 17.0 * f64::from(row), block_w: 6.0, block_h: 14.0, gap: 0.0, alpha: 1.0, plate_w, colors: None });
    }
    let capture = cv.capture();
    let img = capture.bgra();
    let area = Rect { x: 0, y: 0, w, h };
    let mut hits = Vec::new();
    tags::scan(&img, area, None, &mut hits);
    let view = View { id: 1, x: 0, y: 0, w: w as u16, h: h as u16, font_px: 14, row_px: 17 };
    let n = 500;
    let t0 = std::time::Instant::now();
    let mut built = Vec::new();
    for _ in 0..n {
        built = cards::build_with_rows(&img, &hits, &[view], &|_| Some(true), &|_| Some(6), Some(1));
    }
    let t_build = t0.elapsed().as_secs_f64() * 1e6 / f64::from(n);
    let t1 = std::time::Instant::now();
    let mut edges = 0;
    for _ in 0..n {
        edges = backdrop::sample_card(&img, &built[0], area, false).edges.len();
    }
    let t_sample = t1.elapsed().as_secs_f64() * 1e6 / f64::from(n);
    println!("6-row card: cards::build_with_rows {t_build:.1} us, backdrop::sample_card {t_sample:.1} us ({edges} edge runs)");
}
