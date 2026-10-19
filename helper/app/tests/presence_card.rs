//! The presence card (2026-10-19, user request R1), pixel level. The addon's Translate Companion
//! popup, in the middle of the screen, holds a one-row card of slot 125 (a tag and its plate) with
//! the addon's own grey caption line under it (COMPANION_CARD_CAPTION). A synthetic game
//! capture with that popup goes through the real tag search, card build, background sampling,
//! scene and overlay rendering, twice:
//!
//! - **discovery**: an app started while the popup is open has no VIEW; its discovery scan must
//!   find the tag in the middle of the client (`app::discovery_areas`: `tags::scan` alone searches
//!   the left max(200 px, 35%) of an area);
//! - **VIEW 201**: the popup's own VIEW, as the addon sends it when the popup opens.
//!
//! Both times the card must say "Translate Companion detected" in green from the tag's left edge,
//! the tag and the plate must disappear into the card's BUBBLE_RGB fill (no plate colour left), and
//! nothing else may be painted: not the grey line under the card, not the popup's title or border,
//! not the game world.
//!
//! The card is drawn with the addon's own geometry (`CompanionDialog.CardMetrics` at 720 px, UI
//! scale 1: ppu 0.9375, fractional px): the opaque fill, 6 units of it above, under and right of
//! the row, 14 units left of the tag (0.75 of the row height, the discovery scan's reach, and 3 px;
//! integration review: with 6 units the discovery sample left of the tag fell on the popup's own
//! background and painted a darker strip), tag blocks of 6 units, a row of 13 px; VIEW 201 is the
//! fill rounded to physical px, as the addon sends it.

use std::collections::HashMap;

use tp_bridge::cards;
use tp_bridge::image::Rect;
use tp_bridge::msg::{Message, View};
use tp_bridge::synth::{self, Canvas as Synth, Rng, TagDraw, TagTruth};
use tp_bridge::tags::TagHit;
use tp_helper::app::{dedup_hits, discovery_areas, keep_discovery_hit};
use tp_helper::backdrop;
use tp_helper::i18n::Lang;
use tp_helper::overlay::OverlayManager;
use tp_helper::scene::{CardKind, SceneMachine, TickInput, WowGeom, WowTick, PRESENCE_SLOT};
use tp_helper::tagscan::TagScanner;

const W: i32 = 1920;
const H: i32 = 1080;
/// `CompanionDialog.CardMetrics` at a 1080 px client and UI scale 1 (physical px per unit; the
/// addon's units below are converted with it, so most edges are fractional px as in game).
const PPU: f64 = 1080.0 / 768.0;
/// The popup: border box (400 units wide, centred) and its interior (client px).
const POPUP: [f64; 4] = [(1920.0 - 400.0 * PPU) / 2.0, 400.0, (1920.0 + 400.0 * PPU) / 2.0, 700.0];
const BORDER_PX: f64 = 3.0;
const POPUP_RGB: [f32; 3] = [20.0, 18.0, 16.0];
const BORDER_RGB: [f32; 3] = [150.0, 120.0, 70.0];
const TITLE_RGB: [f32; 3] = [255.0, 209.0, 0.0];
const GREY_RGB: [f32; 3] = [128.0, 128.0, 128.0];
const PLATE: [i32; 3] = [32, 40, 64];
/// The card's fill (the addon's `Bridge.BUBBLE_RGB`).
const BUBBLE_RGB: [f32; 3] = [36.0, 30.0, 24.0];
/// The card in client px: the fill's left edge (20 units into the popup) and top, its width (360
/// units), the fill left of the tag (13 units: max(6, ceil((0.75 x 20 + 3) / ppu))), the pad (6
/// units), a tag block (4 units: 5 px at least) and the row (round(14 ppu) = 20 px).
const CARD_X: f64 = POPUP[0] + 20.0 * PPU;
const CARD_Y: f64 = 480.0;
const CARD_W: f64 = 360.0 * PPU;
const LEFT: f64 = 13.0 * PPU;
const PAD: f64 = 6.0 * PPU;
const BLOCK: f64 = 4.0 * PPU;
const ROW_PX: f64 = 20.0;
/// Largest per-channel difference from the popup's background where no text is drawn.
const DELTA: i32 = 6;
const SESSION: u16 = 4243;

struct Popup {
    capture: synth::Capture,
    tag: TagTruth,
    /// The grey line's rectangle under the card.
    grey: [i32; 4],
}

fn draw() -> Popup {
    let mut rng = Rng::new(125);
    let mut cv = Synth::world(&mut rng, W, H);
    cv.noise(&mut rng, 6);
    cv.aa = true;
    let [x0, y0, x1, y1] = POPUP;
    cv.rect(x0, y0, x1, y1, BORDER_RGB, 1.0);
    // an opaque dialog background (DialogBorderDarkTemplate-like), flat so the check is exact
    cv.rect(x0 + BORDER_PX, y0 + BORDER_PX, x1 - BORDER_PX, y1 - BORDER_PX, POPUP_RGB, 1.0);
    // the title, glyph-like strokes
    for gx in (720..900).step_by(9) {
        cv.rect(f64::from(gx), 425.0, f64::from(gx) + 5.0, 441.0, TITLE_RGB, 1.0);
    }
    // the card: its opaque fill, then one row of slot 125, the plate ending PAD before the fill's
    // right edge
    cv.rect(CARD_X, CARD_Y, CARD_X + CARD_W, CARD_Y + 2.0 * PAD + ROW_PX, BUBBLE_RGB, 1.0);
    let plate_w = CARD_W - LEFT - PAD - 8.0 * BLOCK;
    let tag = synth::draw_tag(
        &mut cv,
        &TagDraw { slot: PRESENCE_SLOT, row: 0, sess_bit: (SESSION % 2) as u8, x: CARD_X + LEFT, y: CARD_Y + PAD, block_w: BLOCK, block_h: ROW_PX, gap: 0.0, alpha: 1.0, plate_w, colors: None },
    );
    // the addon's grey caption under the card (COMPANION_CARD_CAPTION, never on it)
    let grey = [(CARD_X + LEFT) as i32, (CARD_Y + 2.0 * PAD + ROW_PX + 6.0) as i32, 230, 11];
    for gx in (grey[0]..grey[0] + grey[2]).step_by(7) {
        cv.rect(f64::from(gx), f64::from(grey[1]), f64::from(gx) + 4.0, f64::from(grey[1] + grey[3]), GREY_RGB, 1.0);
    }
    Popup { capture: cv.capture(), tag, grey }
}

fn tick() -> TickInput {
    TickInput { now_ms: 10, wow: Some(WowTick { geom: WowGeom { hwnd: 1, origin_x: 0, origin_y: 0, client_w: W, client_h: H }, minimized: false, foreground: true }), fresh: true }
}

/// The rendered view composed over the capture: (RGB, overlay alpha) per client pixel of `rect`.
fn compose<'a>(p: &'a Popup, bits: &[u8], rect: Rect) -> impl Fn(i32, i32) -> ([i32; 3], u8) + 'a {
    let img_rgb = move |x: i32, y: i32| p.capture.bgra().rgb(x, y).unwrap();
    let bits = bits.to_vec();
    move |x: i32, y: i32| {
        let i = (((y - rect.y) * rect.w + (x - rect.x)) * 4) as usize;
        let (pm, a) = ([bits[i + 2], bits[i + 1], bits[i]], bits[i + 3]);
        let c = img_rgb(x, y);
        let inv = 255 - i32::from(a);
        ([0, 1, 2].map(|k| i32::from(pm[k]) + (i32::from(c[k]) * inv + 127) / 255), a)
    }
}

/// The checks of both runs, on the rendered view `rect`.
fn check(p: &Popup, bits: &[u8], rect: Rect, label: &str) {
    let px = compose(p, bits, rect);
    let t = &p.tag;
    let plate = t.plate.unwrap();
    // the reserved row: the tag and the plate plus one pixel all around
    let (rx0, rx1, ry0, ry1) = (t.x0.floor() as i32 - 1, plate[2].ceil() as i32, t.y0.floor() as i32 - 1, t.y1.ceil() as i32);
    let (mut green, mut from_tag) = (0, 0);
    for y in ry0..=ry1 {
        for x in rx0..=rx1 {
            let (c, _) = px(x, y);
            let to_plate = (0..3).map(|k| (c[k] - PLATE[k]).abs()).max().unwrap();
            assert!(to_plate > 10, "{label}: plate colour left at ({x},{y}): {c:?}");
            if c[1] > 120 && c[1] > c[0] + 60 && c[1] > c[2] + 60 {
                green += 1;
                if (x as f64) < t.x1 {
                    from_tag += 1;
                }
            }
        }
    }
    assert!(green > 40, "{label}: the green text is drawn ({green} px)");
    assert!(from_tag > 5, "{label}: the text starts over the painted tag ({from_tag} px)");
    // right of the text (the line is about 240 px long from the tag at this size): the card's fill
    let ground = BUBBLE_RGB.map(|v| v as i32);
    let mut worst = 0;
    for y in (t.y0.ceil() as i32)..(t.y1.floor() as i32) {
        for x in (t.x0 as i32 + 320)..(plate[2].floor() as i32 - 1) {
            let (c, a) = px(x, y);
            assert_eq!(a, 255, "{label}: the plate is painted opaque at ({x},{y})");
            let d = (0..3).map(|k| (c[k] - ground[k]).abs()).max().unwrap();
            worst = worst.max(d);
        }
    }
    assert!(worst <= DELTA, "{label}: the plate became the card's fill within {worst} levels");
    // nothing painted anywhere else: the grey line, the title, the border, the world
    for y in rect.y..rect.y + rect.h {
        for x in rect.x..rect.x + rect.w {
            let inside = x >= rx0 - 1 && x <= rx1 + 1 && y >= ry0 - 1 && y <= ry1 + 1;
            if !inside {
                assert_eq!(px(x, y).1, 0, "{label}: painted outside the card at ({x},{y})");
            }
        }
    }
    let g = p.grey;
    assert!(g[1] > ry1 + 4, "the grey line is under the card");
}

#[test]
fn an_app_started_while_the_popup_is_open_finds_the_card_by_discovery() {
    let p = draw();
    let img = p.capture.bgra();
    // discovery: the client below row 16, no VIEW
    let area = Rect { x: 0, y: 16, w: W, h: H - 16 };
    let areas = discovery_areas(area);
    let mut hits: Vec<TagHit> = Vec::new();
    TagScanner::new().scan(&img, &areas, None, &mut hits);
    hits.retain(|h| keep_discovery_hit(h, area));
    dedup_hits(&mut hits, &mut Default::default());
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].slot, PRESENCE_SLOT);
    // the first area alone (the discovery of before) does not reach it
    let mut alone = Vec::new();
    tp_bridge::tags::scan(&img, area, None, &mut alone);
    assert!(alone.is_empty(), "the popup's tag is out of the left 35%");
    // the app's discovery view: the font from the tag height
    let h = hits[0].y_bot - hits[0].y_top + 1;
    let view = View { id: 0, x: 0, y: 16, w: W as u16, h: (H - 16) as u16, font_px: h as u8, row_px: ((h as f32) * 1.15).round() as u8 };
    let built = cards::build_with_rows(&img, &hits, &[view], &|_| None, &|_| None, None);
    assert_eq!(built.len(), 1);
    assert!(built[0].plate && built[0].bands.len() == 1);
    let bg = backdrop::sample_card(&img, &built[0], area, true);
    // the left sample (0.75 of the row height left of the tag) lands in the card's fill, not in the
    // popup's background
    let left = bg.bands[0].left.expect("a left sample in discovery").rgb;
    let fill = BUBBLE_RGB.map(|v| v as i32);
    assert!((0..3).all(|k| (i32::from(left[k]) - fill[k]).abs() <= DELTA), "discovery left sample {left:?}, the fill {fill:?}");
    let mut m = SceneMachine::new();
    let scene = m.update_with_bg(&tick(), vec![(built[0].clone(), bg)], &hits, &mut |_| false);
    assert_eq!(scene.views[0].cards[0].kind, CardKind::Presence);
    let mut o = OverlayManager::new(true, false, [0xA8, 0xD8, 0xFF], 1.0);
    let reports = o.render(&scene, &HashMap::new(), 1.0, Lang::En);
    assert_eq!(reports.len(), 1);
    assert!(reports[0].stats.text_px > 0 && !o.animating);
    check(&p, &o.memory_pixels(0).unwrap(), scene.views[0].rect, "discovery");
}

#[test]
fn the_popup_view_201_gets_the_card_painted() {
    let p = draw();
    let img = p.capture.bgra();
    // the popup's VIEW: the card's fill in physical px (CompanionDialog.ViewEntry: rounded), its
    // font (round(12 ppu)) and row
    let (vx, vy) = (CARD_X.round() as i32, CARD_Y.round() as i32);
    let (vw, vh) = (CARD_W.round() as i32, (2.0 * PAD + ROW_PX).round() as i32);
    let view = View { id: 201, x: vx as u16, y: vy as u16, w: vw as u16, h: vh as u16, font_px: (12.0 * PPU).round() as u8, row_px: ROW_PX as u8 };
    let rect = Rect { x: vx, y: vy, w: vw, h: vh };
    let mut hits: Vec<TagHit> = Vec::new();
    TagScanner::new().scan(&img, &[rect], None, &mut hits);
    assert_eq!(hits.iter().map(|h| h.slot).collect::<Vec<_>>(), vec![PRESENCE_SLOT]);
    let mut m = SceneMachine::new();
    m.on_message(SESSION, Message::View(vec![view]));
    let built = cards::build_with_rows(&img, &hits, &[view], &|s| m.plate_expected(s), &|s| m.rows_expected(s), m.sess_bit());
    assert_eq!(built.len(), 1);
    let bg = backdrop::sample_card(&img, &built[0], rect, false);
    assert!(bg.bands[0].left.is_some() && bg.bands[0].right.is_some(), "both sides sampled in the card's fill: {bg:?}");
    let fill = BUBBLE_RGB.map(|v| v as i32);
    for side in [bg.bands[0].left, bg.bands[0].right].into_iter().flatten() {
        assert!((0..3).all(|k| (i32::from(side.rgb[k]) - fill[k]).abs() <= DELTA), "sample {:?}, the fill {fill:?}", side.rgb);
    }
    let scene = m.update_with_bg(&tick(), vec![(built[0].clone(), bg)], &hits, &mut |_| false);
    assert_eq!(scene.views[0].id, 201);
    assert_eq!(scene.views[0].cards[0].kind, CardKind::Presence);
    let mut o = OverlayManager::new(true, false, [0xA8, 0xD8, 0xFF], 1.0);
    o.render(&scene, &HashMap::new(), 1.0, Lang::Fr);
    check(&p, &o.memory_pixels(201).unwrap(), scene.views[0].rect, "view 201");
}

/// Cost of a discovery scan over a 1920 x 1064 game picture with no tag, with the whole-client area
/// alone (before 0.6.0) and with the presence areas. Run:
/// `cargo test --offline --release --test presence_card -- --ignored --nocapture`.
#[test]
#[ignore]
fn discovery_scan_cost() {
    let mut rng = Rng::new(7);
    let mut cv = Synth::world(&mut rng, 1920, 1080);
    cv.noise(&mut rng, 12);
    let cap = cv.capture();
    let img = cap.bgra();
    let area = Rect { x: 0, y: 16, w: 1920, h: 1064 };
    let n = 40;
    for (label, areas) in [("whole client", vec![area]), ("with the presence areas", discovery_areas(area))] {
        let mut sc = TagScanner::new();
        let mut hits = Vec::new();
        sc.scan(&img, &areas, None, &mut hits);
        let t0 = std::time::Instant::now();
        for _ in 0..n {
            hits.clear();
            sc.scan(&img, &areas, None, &mut hits);
        }
        let per = t0.elapsed().as_secs_f64() * 1000.0 / f64::from(n);
        eprintln!("discovery scan, {label} ({} areas): {per:.3} ms, {} hits", areas.len(), hits.len());
    }
}
