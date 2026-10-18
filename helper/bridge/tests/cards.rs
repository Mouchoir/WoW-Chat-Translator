//! Cards (design P2 acceptance): a covering rectangle over part of a plate is excluded from the
//! mask (IoU against the truth >= 0.98), gap fill between rows, no-plate mode, plus grouping,
//! session bit, fade and the dropped-row rule; plates faded over coloured grounds (shown down to
//! alpha 0.33, text ends at the plate end); covers whose edge lies at the plate start are not taken
//! for plates. Counts scale with TP_TAGS_SCALE (default 1.0).

use tp_bridge::cards::{self, Card};
use tp_bridge::msg::View;
use tp_bridge::synth::{self, Canvas, Rng, TagDraw, TagTruth};
use tp_bridge::{image, tags};

fn scale() -> f64 {
    std::env::var("TP_TAGS_SCALE").ok().and_then(|s| s.parse().ok()).unwrap_or(1.0)
}

struct Case {
    canvas: Canvas,
    view: View,
    truths: Vec<TagTruth>,
    covers: Vec<[f64; 4]>,
    /// The ground alone (before the card was drawn), when kept.
    ground: Option<Canvas>,
}

/// One card of `k` rows on a chat background; `cover` puts a tooltip over part of the plates.
fn card_case(rng: &mut Rng, k: u8, alpha: f32, plate: bool, cover: bool, cover_rgb: Option<[f32; 3]>) -> Case {
    card_case_edges(rng, k, alpha, plate, cover, cover_rgb, false)
}

/// `cover_aa`: draw the cover with antialiased edges (UI frames are rasterized quads with hard
/// edges; their border art may still be soft, which this approximates).
fn card_case_edges(
    rng: &mut Rng,
    k: u8,
    alpha: f32,
    plate: bool,
    cover: bool,
    cover_rgb: Option<[f32; 3]>,
    cover_aa: bool,
) -> Case {
    let (w, h) = (520, 160);
    let mut cv = Canvas::world(rng, w, h);
    cv.darken(rng.uniform(0.6, 1.0) as f32);
    cv.noise(rng, 3);
    cv.aa = rng.chance(0.5);
    let bw = rng.uniform(4.0, 12.0);
    let gap = rng.uniform(0.0, 2.0);
    let bh = rng.uniform(10.0, 20.0).round();
    let row_gap = rng.int(0, 4) as f64;
    let x = 20.0 + rng.uniform(-2.0, 2.0);
    let mut y = 12.0 + rng.uniform(0.0, 1.0);
    let plate_w = rng.uniform(150.0, 380.0);
    let slot = rng.int(0, 126) as u8;
    let sess = rng.int(0, 1) as u8;
    let mut truths = Vec::new();
    for row in 0..k {
        truths.push(synth::draw_tag(&mut cv, &TagDraw {
            slot,
            row,
            sess_bit: sess,
            x,
            y,
            block_w: bw,
            block_h: bh,
            gap,
            alpha,
            plate_w: if plate { plate_w } else { 0.0 },
            colors: None,
        }));
        y += bh + row_gap;
    }
    let mut covers = Vec::new();
    if cover {
        // over the plates, right of where the plate colour is sampled
        let t0 = &truths[0];
        let px0 = t0.x1 + 2.0 * bw + 20.0;
        let cx0 = rng.uniform(px0, px0 + plate_w * 0.5);
        let cw = rng.uniform(40.0, 160.0);
        let cy0 = t0.y0 + rng.uniform(-10.0, (y - t0.y0) * 0.5);
        let r = [cx0, cy0, cx0 + cw, cy0 + rng.uniform(20.0, 80.0)];
        let aa = cv.aa;
        cv.aa = cover_aa;
        match cover_rgb {
            None => synth::draw_cover(&mut cv, r),
            Some(c) => cv.rect(r[0], r[1], r[2], r[3], c, 1.0),
        }
        cv.aa = aa;
        covers.push(r);
    }
    // UI quads are flat colours and the capture is lossless: only quantization-level noise here
    // (the world behind the chat got its texture noise before the tags were drawn)
    cv.noise(rng, 1);
    Case {
        canvas: cv,
        view: View { id: 1, x: 0, y: 0, w: w as u16, h: h as u16, font_px: 14, row_px: 16 },
        truths,
        covers,
        ground: None,
    }
}

fn build(c: &Case, plate_expected: Option<bool>) -> (Vec<tags::TagHit>, Vec<Card>) {
    let cap = c.canvas.capture();
    let img = cap.bgra();
    let mut hits = Vec::new();
    let area = image::Rect { x: 0, y: 0, w: i32::from(c.view.w), h: i32::from(c.view.h) };
    tags::scan(&img, area, None, &mut hits);
    let cards = cards::build(&img, &hits, &[c.view], &|_| plate_expected, None);
    (hits, cards)
}

fn inside(r: &[f64; 4], x: i32, y: i32) -> bool {
    let (cx, cy) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
    cx >= r[0] && cx < r[2] && cy >= r[1] && cy < r[3]
}

/// Fraction of pixel (x, y) covered by rectangle r.
fn coverage(r: &[f64; 4], x: i32, y: i32) -> f64 {
    let (x, y) = (f64::from(x), f64::from(y));
    let w = ((x + 1.0).min(r[2]) - x.max(r[0])).max(0.0);
    let h = ((y + 1.0).min(r[3]) - y.max(r[1])).max(0.0);
    w * h
}

/// Truth for one pixel of a row: Some(true) visible plate, Some(false) not plate, None for a
/// boundary pixel the antialiased renderer blended (plate or cover edge): left out of the score.
fn truth_px(t: &TagTruth, covers: &[[f64; 4]], aa: bool, x: i32, y: i32) -> Option<bool> {
    let p = t.plate.unwrap();
    if !aa {
        return Some(inside(&p, x, y) && !covers.iter().any(|c| inside(c, x, y)));
    }
    let cp = coverage(&p, x, y);
    let cc = covers.iter().map(|c| coverage(c, x, y)).fold(0.0, f64::max);
    if cc >= 1.0 || cp <= 0.0 {
        Some(false)
    } else if cp >= 1.0 && cc <= 0.0 {
        Some(true)
    } else {
        None
    }
}

#[test]
fn covered_plates_are_masked_out() {
    let (iou, worst) = mask_iou(false);
    assert!(iou >= 0.98, "aggregate IoU {iou}");
    assert!(worst >= 0.98, "worst card IoU {worst}");
}

#[test]
fn covered_plates_with_soft_cover_edges() {
    // a soft border edge can blend into the plate colour on one pixel row: reported, aggregate only
    let (iou, _) = mask_iou(true);
    assert!(iou >= 0.98, "aggregate IoU {iou}");
}

fn mask_iou(cover_aa: bool) -> (f64, f64) {
    let mut rng = Rng::new(0xCA7D5);
    let n = ((2000.0 * scale()) as usize).max(20);
    let (mut inter, mut union) = (0u64, 0u64);
    let mut worst = 1.0f64;
    let mut worst_case = 0;
    let mut cards_n = 0;
    let mut covered_px = 0u64;
    let mut covered_in_mask = 0u64;
    let (mut leak, mut plate_px, mut leak_cards) = (0u64, 0u64, 0u64);
    for i in 0..n {
        // tooltips (border + (23,23,48) backdrop) or menus of random colours over unfaded rows
        let cover_rgb = if i % 2 == 0 {
            None
        } else {
            Some([rng.uniform(60.0, 255.0) as f32, rng.uniform(0.0, 255.0) as f32, rng.uniform(0.0, 255.0) as f32])
        };
        let alpha = 1.0;
        let k = rng.int(1, 4) as u8;
        let c = card_case_edges(&mut rng, k, alpha, true, true, cover_rgb, cover_aa);
        let (hits, cards) = build(&c, Some(true));
        assert_eq!(cards.len(), 1, "case {i}: one card");
        let card = &cards[0];
        if card.bands.len() != usize::from(k) {
            let cap = c.canvas.capture();
            for t in &c.truths {
                let yc = ((t.y0 + t.y1) / 2.0) as i32;
                let area = image::Rect { x: 0, y: 0, w: i32::from(c.view.w), h: i32::from(c.view.h) };
                let d: Vec<String> = (19..30).map(|x| format!("{x}:{:?}", tags::diagnose(&cap.bgra(), area, None, x, yc).map(|h| (h.row, h.y_top, h.y_bot)))).collect();
                eprintln!("row {} x0 {:.2} y {:.2}..{:.2} bw {:.2} pitch {:.2} aa {} diag {}", t.row, t.x0, t.y0, t.y1, t.block_w, (t.x1 - t.x0 - t.block_w) / 7.0, c.canvas.aa, d.join(" "));
            }
            for h in &hits { eprintln!("hit row {} y {}..{} x1 {}", h.row, h.y_top, h.y_bot, h.x1); }
        }
        assert_eq!(card.bands.len(), usize::from(k), "case {i}: every row found");
        cards_n += 1;
        let (mut ci, mut cu) = (0u64, 0u64);
        for (b, t) in card.bands.iter().zip(&c.truths) {
            let m = b.mask.as_ref().expect("plate mode has a mask");
            let plate_end = t.plate.unwrap()[2];
            for y in m.rect.y..m.rect.y + m.rect.h {
                for x in m.rect.x..m.rect.x + m.rect.w {
                    let a = m.get(x, y);
                    if f64::from(x) >= plate_end + 1.0 {
                        // beyond the plate: the design's mask runs to the view's edge, so a
                        // background matching the plate colour leaks in (reported, not scored)
                        leak += u64::from(a);
                        continue;
                    }
                    let Some(tr) = truth_px(t, &c.covers, c.canvas.aa, x, y) else { continue };
                    ci += u64::from(a && tr);
                    cu += u64::from(a || tr);
                    if c.covers.iter().any(|r| inside(r, x, y)) && inside(&t.plate.unwrap(), x, y) {
                        covered_px += 1;
                        covered_in_mask += u64::from(a);
                    }
                }
            }
            plate_px += (m.rect.h as f64 * (plate_end - f64::from(m.rect.x))) as u64;
        }
        leak_cards += u64::from(card.bands.iter().zip(&c.truths).any(|(b, t)| {
            b.text_rect.x + b.text_rect.w > t.plate.unwrap()[2] as i32 + 2
        }));
        let iou = ci as f64 / cu.max(1) as f64;
        if iou < worst {
            worst = iou;
            worst_case = i;
        }
        inter += ci;
        union += cu;
    }
    let iou = inter as f64 / union as f64;
    println!(
        "mask IoU (cover edges {}) over {cards_n} cards (unambiguous pixels over the plate's extent): aggregate {iou:.4}, worst card \
         {worst:.4} (case {worst_case}); covered plate pixels {covered_px}, of which in the mask {covered_in_mask}; \
         beyond the plate end: {leak} mask px ({:.3}% of plate px), text past the plate end on {leak_cards} cards",
        if cover_aa { "soft" } else { "hard" },
        100.0 * leak as f64 / plate_px.max(1) as f64
    );
    (iou, worst)
}

#[test]
fn gaps_between_rows_are_filled() {
    let mut rng = Rng::new(0x6A9);
    let mut filled = 0;
    for i in 0..200 {
        let c = card_case(&mut rng, 3, 1.0, true, false, None);
        let (_, cards) = build(&c, Some(true));
        let card = &cards[0];
        assert_eq!(card.bands.len(), 3, "case {i}");
        for w in card.bands.windows(2) {
            // the tag rows (tag_rect may also hold a blended edge row)
            let gap = w[1].text_rect.y - (w[0].text_rect.y + w[0].text_rect.h);
            let in_gap: Vec<_> = card.gaps.iter().filter(|g| g.y == w[0].text_rect.y + w[0].text_rect.h).collect();
            if (1..=4).contains(&gap) {
                // the whole visible plate width is bridged (minus the edges)
                let px: i32 = in_gap.iter().map(|g| g.w).sum();
                let plate_w = w[0].mask.as_ref().unwrap().count() as i32 / w[0].text_rect.h;
                assert!(px as f64 >= 0.95 * plate_w as f64, "case {i}: gap {gap} filled {px} of {plate_w}");
                assert!(in_gap.iter().all(|g| g.h == gap && g.x >= w[0].tag_rect.x + w[0].tag_rect.w));
                filled += 1;
            } else {
                assert!(in_gap.is_empty());
            }
        }
    }
    println!("row gaps filled: {filled}");
    assert!(filled > 100);
}

#[test]
fn no_plate_mode_fills_to_the_view_edge() {
    let mut rng = Rng::new(0x9071);
    for i in 0..200 {
        let k = rng.int(1, 4) as u8;
        let c = card_case(&mut rng, k, 1.0, false, false, None);
        let (hits, cards) = build(&c, Some(false));
        assert_eq!(cards.len(), 1, "case {i}");
        let card = &cards[0];
        assert!(!card.plate);
        assert_eq!(card.bands.len(), hits.len());
        for (b, h) in card.bands.iter().zip(&hits) {
            assert!(b.mask.is_none());
            assert_eq!(b.text_rect.x, h.x1 + 3);
            assert_eq!(b.text_rect.x + b.text_rect.w, i32::from(c.view.w) - 4);
        }
        // fill colour: the background near the view's right edge
        let cap = c.canvas.capture();
        let p = cap.bgra().rgb(i32::from(c.view.w) - 10, hits[0].y_top + 2).unwrap();
        assert!((0..3).all(|k| (i32::from(p[k]) - i32::from(card.plate_rgb[k])).abs() <= 12), "case {i}");
    }
}

#[test]
fn plate_expected_but_covered_drops_the_row() {
    let mut rng = Rng::new(0xD20);
    let mut c = card_case(&mut rng, 2, 1.0, true, false, None);
    // a tooltip right over the first row's plate start (where Pobs is sampled)
    let t = c.truths[0].clone();
    let r = [t.x1 - 1.0, t.y0 - 3.0, t.x1 + 120.0, t.y1 + 1.0];
    synth::draw_cover(&mut c.canvas, r);
    let (hits, cards) = build(&c, Some(true));
    assert!(hits.iter().any(|h| h.row == 0), "the tag itself is still read");
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].bands.iter().map(|b| b.row).collect::<Vec<_>>(), vec![1], "row 0 is dropped, never painted");
    // an unknown slot (no TRANSLATE seen) is treated like a plate request
    let (_, cards) = build(&c, None);
    assert_eq!(cards[0].bands.len(), 1);
}

#[test]
fn text_rect_stops_at_a_cover_and_tag_rect_covers_the_tag() {
    let mut rng = Rng::new(0x7E7);
    for i in 0..100 {
        let c = card_case(&mut rng, 1, 1.0, true, true, None);
        let (hits, cards) = build(&c, Some(true));
        let b = &cards[0].bands[0];
        let (h, t, cov) = (&hits[0], &c.truths[0], c.covers[0]);
        // the tag plus its blended edges: columns x0 - 1 ..= x1, rows yTop..=yBot and at most one
        // partial row on each side
        assert_eq!((b.tag_rect.x, b.tag_rect.w), (h.x0 - 1, h.x1 - h.x0 + 2), "case {i}");
        let (top, bot) = (b.tag_rect.y, b.tag_rect.y + b.tag_rect.h - 1);
        assert!((h.y_top - 1..=h.y_top).contains(&top) && (h.y_bot..=h.y_bot + 1).contains(&bot), "case {i}");
        assert!((f64::from(h.x0) - t.x0).abs() <= 1.0 && (f64::from(h.x1) - t.x1).abs() <= 1.5, "case {i}");
        let right = b.text_rect.x + b.text_rect.w;
        let plate_end = t.plate.unwrap()[2];
        // the middle row decides (covers are drawn with hard edges: pixel centres)
        let mid = f64::from((h.y_top + h.y_bot) / 2) + 0.5;
        let covers_mid = cov[1] <= mid && cov[3] > mid;
        if covers_mid {
            assert!((f64::from(right) - cov[0]).abs() <= 2.0, "case {i}: text ends at the cover ({right} vs {:.1})", cov[0]);
        } else {
            assert!(
                (f64::from(right) - plate_end).abs() <= 2.0,
                "case {i}: text ends at the plate end ({right} vs {plate_end:.1}), cover {cov:?} mid {mid}"
            );
        }
    }
}

#[test]
fn grouping_session_and_fade() {
    let mut rng = Rng::new(0x6609);
    // two copies of the same card in two views, and a third far below in the first view
    let mut cv = Canvas::new(1000, 400, [12.0, 12.0, 16.0]);
    cv.aa = false;
    let draw = |cv: &mut Canvas, x: f64, y: f64, row: u8, sess: u8, alpha: f32| {
        synth::draw_tag(cv, &TagDraw {
            slot: 9,
            row,
            sess_bit: sess,
            x,
            y,
            block_w: 6.0,
            block_h: 14.0,
            gap: 0.0,
            alpha,
            plate_w: 200.0,
            colors: None,
        })
    };
    for row in 0..2 {
        draw(&mut cv, 20.0, 20.0 + 15.0 * f64::from(row), row, 1, 1.0);
        draw(&mut cv, 520.0, 60.0 + 15.0 * f64::from(row), row, 1, 1.0);
        draw(&mut cv, 20.0, 300.0 + 15.0 * f64::from(row), row, 1, 1.0);
    }
    draw(&mut cv, 20.0, 200.0, 0, 0, 1.0); // other session
    draw(&mut cv, 20.0, 240.0, 0, 1, 0.2); // faded out
    cv.noise(&mut rng, 2);
    let cap = cv.capture();
    let img = cap.bgra();
    let views = [
        View { id: 1, x: 0, y: 0, w: 480, h: 400, font_px: 14, row_px: 15 },
        View { id: 3, x: 500, y: 0, w: 480, h: 400, font_px: 14, row_px: 15 },
    ];
    let mut hits = Vec::new();
    for v in &views {
        tags::scan(&img, image::Rect { x: i32::from(v.x), y: i32::from(v.y), w: i32::from(v.w), h: i32::from(v.h) }, None, &mut hits);
    }
    assert_eq!(hits.len(), 8);
    let cards = cards::build(&img, &hits, &views, &|_| Some(true), Some(1));
    let mut got: Vec<(u8, usize, i32)> = cards.iter().map(|c| (c.view_id, c.bands.len(), c.bands[0].tag_rect.y)).collect();
    got.sort();
    assert_eq!(got, vec![(1, 2, 20), (1, 2, 300), (3, 2, 60)]);
    // without a session filter the other session's tag forms its own card; the faded one never shows
    let cards = cards::build(&img, &hits, &views, &|_| Some(true), None);
    assert_eq!(cards.len(), 4);
    assert!(cards.iter().all(|c| c.alpha >= cards::ALPHA_MIN));
}

// ------------------------------------------------------------------------------------ fade and covers
use tp_bridge::synth::Ground;

/// One card of `k` rows faded to `alpha` over a ground of `kind` (smooth texture plus +-6 noise,
/// sometimes under a 30% black chat panel, like the E2E rig's world).
fn ground_case(rng: &mut Rng, kind: Ground, k: u8, alpha: f32) -> Case {
    let (w, h) = (520, 120);
    let amp = rng.uniform(0.0, 18.0) as f32;
    let cell = rng.int(12, 60) as i32;
    let mut cv = Canvas::ground(rng, kind, w, h, amp, cell, 6);
    if rng.chance(0.5) {
        cv.darken(0.3);
    }
    cv.aa = rng.chance(0.5);
    let ground = cv.clone();
    let bw = rng.uniform(4.0, 12.0);
    let gap = rng.uniform(0.0, 2.0);
    let bh = rng.uniform(10.0, 20.0).round();
    let row_gap = rng.int(0, 4) as f64;
    let x = 20.0 + rng.uniform(-2.0, 2.0);
    let mut y = 12.0 + rng.uniform(0.0, 1.0);
    let plate_w = rng.uniform(150.0, 400.0);
    let slot = rng.int(0, 126) as u8;
    let sess = rng.int(0, 1) as u8;
    let mut truths = Vec::new();
    for row in 0..k {
        truths.push(synth::draw_tag(&mut cv, &TagDraw {
            slot,
            row,
            sess_bit: sess,
            x,
            y,
            block_w: bw,
            block_h: bh,
            gap,
            alpha,
            plate_w,
            colors: None,
        }));
        y += bh + row_gap;
    }
    Case {
        canvas: cv,
        view: View { id: 1, x: 0, y: 0, w: w as u16, h: h as u16, font_px: 14, row_px: 16 },
        truths,
        covers: vec![],
        ground: Some(ground),
    }
}

/// The plate's contrast against the ground right after its end: alpha * max |PLATE - ground|.
fn end_step(c: &Case, t: &TagTruth) -> f64 {
    let g = c.ground.as_ref().unwrap();
    let end = t.plate.unwrap()[2].ceil() as i32 + 1;
    let ym = ((t.y0 + t.y1) / 2.0) as i32;
    let (mut s, mut n) = ([0f64; 3], 0.0);
    for y in ym - 1..=ym + 1 {
        for x in end..(end + 8).min(g.w) {
            let p = g.at(x, y);
            for k in 0..3 {
                s[k] += f64::from(p[k]);
            }
            n += 1.0;
        }
    }
    if n == 0.0 {
        return 255.0;
    }
    let p = tags::PLATE_RGB;
    (0..3).map(|k| f64::from(t.alpha) * (f64::from(p[k]) - s[k] / n).abs()).fold(0.0, f64::max)
}

/// Rows faded to under this alpha may already be hidden (the alpha estimate has noise).
const ALPHA_SURE: f32 = 0.33;
/// The plate's end is visible when its step against the ground beyond is at least this.
const STEP_SEEN: f64 = 12.0;

#[test]
fn fade_over_coloured_grounds() {
    let mut rng = Rng::new(0xFADE);
    let per = ((300.0 * scale()) as usize).max(10);
    let (mut rows_all, mut rows_found, mut ext_bad, mut cut_short) = (0usize, 0usize, 0usize, 0usize);
    let (mut iou_i, mut iou_u, mut amb, mut amb_over) = (0u64, 0u64, 0usize, Vec::new());
    // rows at alpha >= 0.45 whose plate end stands out by 16 or more
    let (mut clear_rows, mut clear_bad) = (0usize, 0usize);
    for kind in Ground::ALL {
        let mut by_alpha = [(0usize, 0usize); 7];
        let (mut kr, mut kf, mut kext, mut kamb) = (0usize, 0usize, 0usize, 0usize);
        for i in 0..per {
            let alpha = rng.uniform(0.3, 1.0) as f32;
            let k = rng.int(1, 3) as u8;
            let c = ground_case(&mut rng, kind, k, alpha);
            let (hits, cards) = build(&c, Some(true));
            let bucket = (((alpha - 0.3) / 0.1) as usize).min(6);
            for t in &c.truths {
                let band = cards
                    .iter()
                    .flat_map(|cd| cd.bands.iter())
                    .find(|b| b.row == t.row && (f64::from(b.tag_rect.y) - t.y0).abs() <= 1.5);
                if alpha < ALPHA_SURE && band.is_none() {
                    continue; // at the fade limit: hidden or not, both are right
                }
                kr += 1;
                by_alpha[bucket].0 += 1;
                let Some(b) = band else {
                    if kr - kf <= 3 {
                        let hit = hits.iter().any(|h| h.row == t.row);
                        eprintln!("{} case {i}: row {} alpha {alpha:.2} not shown (tag found: {hit})", kind.name(), t.row);
                    }
                    continue;
                };
                kf += 1;
                by_alpha[bucket].1 += 1;
                let plate_end = t.plate.unwrap()[2].min(f64::from(c.view.w));
                let right = f64::from(b.text_rect.x + b.text_rect.w);
                let step = end_step(&c, t);
                let clear = alpha >= 0.45 && step >= 16.0;
                clear_rows += usize::from(clear);
                clear_bad += usize::from(clear && (right - plate_end).abs() > 2.0);
                if right < plate_end - 2.0 {
                    cut_short += 1;
                    eprintln!(
                        "{} case {i}: row {} alpha {alpha:.2}: text cut at {right}, plate ends at {plate_end:.1} (step {step:.1})",
                        kind.name(),
                        t.row
                    );
                }
                if step < STEP_SEEN {
                    // the plate's end does not show against this ground: the text may run on
                    kamb += 1;
                    amb_over.push(right - plate_end);
                    continue;
                }
                if (right - plate_end).abs() > 2.0 {
                    kext += 1;
                    if kext <= 3 {
                        eprintln!(
                            "{} case {i}: row {} alpha {alpha:.2}: text ends at {right}, plate at {plate_end:.1} (step {step:.1})",
                            kind.name(),
                            t.row
                        );
                    }
                }
                let m = b.mask.as_ref().unwrap();
                for y in m.rect.y..m.rect.y + m.rect.h {
                    for x in m.rect.x..m.rect.x + m.rect.w {
                        let Some(tr) = truth_px(t, &[], c.canvas.aa, x, y) else { continue };
                        let a = m.get(x, y);
                        iou_i += u64::from(a && tr);
                        iou_u += u64::from(a || tr);
                    }
                }
            }
        }
        let shares: Vec<String> =
            by_alpha.iter().enumerate().map(|(j, (n, f))| format!("{:.1}: {f}/{n}", 0.3 + 0.1 * j as f64)).collect();
        println!(
            "fade over {:<6} rows {kr}, shown {kf}; end not visible {kamb}; text end off by > 2 px {kext}; by alpha {}",
            kind.name(),
            shares.join(", ")
        );
        rows_all += kr;
        rows_found += kf;
        ext_bad += kext;
        amb += kamb;
    }
    let iou = iou_i as f64 / iou_u.max(1) as f64;
    amb_over.sort_by(|a, b| a.total_cmp(b));
    let q = |f: f64| amb_over.get(((amb_over.len() as f64 - 1.0) * f) as usize).copied().unwrap_or(0.0);
    let seen = rows_found - amb;
    println!(
        "fade over coloured grounds: rows {rows_all}, shown {rows_found}; visible plate ends {seen}: text end off {ext_bad}, \
         mask IoU {iou:.4}; of which alpha >= 0.45 and step >= 16: {clear_rows}, text end off {clear_bad}; \
         invisible plate ends {amb}: text overrun median {:.0} px, P90 {:.0} px; cut short {cut_short}",
        q(0.5),
        q(0.9)
    );
    assert_eq!(rows_found, rows_all, "every row with alpha >= 0.33 is shown");
    // A textured ground can look like the plate's end, or like more plate after it (mostly under
    // alpha 0.45): bounded rates, not zero (see docs/BRIDGE.md).
    assert!(clear_bad * 200 <= clear_rows, "clear plate ends: text end off on {clear_bad} of {clear_rows} (limit 0.5%)");
    assert!(ext_bad * 100 <= seen, "text end off on {ext_bad} of {seen} visible ends (limit 1%)");
    assert!(cut_short * 1000 <= rows_found, "text cut short on {cut_short} of {rows_found} rows (limit 0.1%)");
    assert!(iou >= 0.98, "mask IoU {iou}");
}

/// Kinds of covering UI drawn over the start of a plate.
#[derive(Clone, Copy, Debug)]
enum CoverKind {
    /// Default tooltip: (23,23,48) at 0.95 with a 2 px light border.
    Tooltip,
    /// The E2E rig tooltip: (16,16,24) at 0.94 with a 2 px (190,190,190) border.
    RigTooltip,
    /// A borderless near-black menu backdrop.
    Menu,
    /// Opaque random colour.
    Random,
    /// Opaque random dark blue-ish colour (close to the plate hue).
    Bluish,
}

fn draw_cover_kind(cv: &mut Canvas, rng: &mut Rng, kind: CoverKind, r: [f64; 4]) {
    let border = |cv: &mut Canvas, c: [f32; 3]| {
        cv.rect(r[0], r[1], r[2], r[1] + 2.0, c, 1.0);
        cv.rect(r[0], r[3] - 2.0, r[2], r[3], c, 1.0);
        cv.rect(r[0], r[1] + 2.0, r[0] + 2.0, r[3] - 2.0, c, 1.0);
        cv.rect(r[2] - 2.0, r[1] + 2.0, r[2], r[3] - 2.0, c, 1.0);
    };
    match kind {
        CoverKind::Tooltip => synth::draw_cover(cv, r),
        CoverKind::RigTooltip => {
            cv.rect(r[0] + 2.0, r[1] + 2.0, r[2] - 2.0, r[3] - 2.0, [16.0, 16.0, 24.0], 0.94);
            border(cv, [190.0, 190.0, 190.0]);
        }
        CoverKind::Menu => {
            let v = rng.uniform(0.0, 20.0) as f32;
            cv.rect(r[0], r[1], r[2], r[3], [v, v, v], rng.uniform(0.85, 1.0) as f32);
        }
        CoverKind::Random => {
            let c = [rng.uniform(0.0, 255.0) as f32, rng.uniform(0.0, 255.0) as f32, rng.uniform(0.0, 255.0) as f32];
            cv.rect(r[0], r[1], r[2], r[3], c, 1.0);
        }
        CoverKind::Bluish => {
            let c = [rng.uniform(0.0, 50.0) as f32, rng.uniform(0.0, 60.0) as f32, rng.uniform(30.0, 110.0) as f32];
            cv.rect(r[0], r[1], r[2], r[3], c, 1.0);
        }
    }
}

#[test]
fn covers_over_the_plate_start_are_never_plates() {
    let mut rng = Rng::new(0xC0FE);
    let per = ((400.0 * scale()) as usize).max(10);
    let kinds = [CoverKind::Tooltip, CoverKind::RigTooltip, CoverKind::Menu, CoverKind::Random, CoverKind::Bluish];
    let (mut all_rows, mut all_false) = (0usize, 0usize);
    for kind in kinds {
        let (mut covered_rows, mut false_plates) = (0usize, 0usize);
        for i in 0..per {
            let alpha = rng.uniform(0.3, 1.0) as f32;
            let k = rng.int(1, 3) as u8;
            let ground = *rng.pick(&Ground::ALL);
            let mut c = ground_case(&mut rng, ground, k, alpha);
            // a cover whose left edge falls between the tag end and the plate sample: it hides the
            // plate start of one or more rows and spans more than one row vertically
            let t0 = c.truths[0].clone();
            let tl = c.truths[c.truths.len() - 1].clone();
            let bw = t0.block_w;
            let left = t0.x1 + rng.uniform(-0.5, 1.0 + bw * 0.4);
            let top = rng.uniform(t0.y0 - 30.0, tl.y1 - 6.0).max(0.0);
            let bottom = (top + rng.uniform(24.0, 90.0)).min(f64::from(c.view.h));
            let r = [left, top, left + rng.uniform(60.0, 200.0), bottom];
            let aa = c.canvas.aa;
            c.canvas.aa = rng.chance(0.3);
            draw_cover_kind(&mut c.canvas, &mut rng, kind, r);
            c.canvas.aa = aa;
            let (_, cards) = build(&c, Some(true));
            // the plate sample window, [x1 + 2, x1 + 2 + bw] x the middle half of the rows the
            // tag was found on (a cover over the plate start can shorten the tag's rows)
            let window = |x1: f64, y0: f64, y1: f64| {
                let (sx0, sx1) = (x1 + 2.0, x1 + 2.0 + bw.round().max(3.0) + 1.0);
                let (sy0, sy1) = (y0 + 0.25 * (y1 - y0), y1 - 0.25 * (y1 - y0));
                let cov_w = (sx1.min(r[2]) - sx0.max(r[0])).max(0.0) / (sx1 - sx0);
                let cov_h = (sy1.min(r[3]) - sy0.max(r[1])).max(0.0) / (sy1 - sy0);
                cov_w >= 0.75 && cov_h >= 0.99
            };
            covered_rows += c.truths.iter().filter(|t| window(t.x1, t.y0, t.y1)).count();
            for b in cards.iter().flat_map(|cd| cd.bands.iter()) {
                let tr = b.tag_rect;
                if window(f64::from(tr.x + tr.w), f64::from(tr.y), f64::from(tr.y + tr.h)) {
                    false_plates += 1;
                    if false_plates <= 3 {
                        eprintln!("{kind:?} case {i}: row {} over {} at alpha {alpha:.2} taken for a plate", b.row, ground.name());
                    }
                }
            }
        }
        println!("covers ({kind:?}) over the plate start: covered rows {covered_rows}, taken for a plate {false_plates}");
        all_rows += covered_rows;
        all_false += false_plates;
        // A cover of exactly the faded plate's colour is a plate as far as colour goes: only its
        // extent above or below the row gives it away, and a cover that ends within the card's
        // rows does not show one. Bounded here, not zero (see docs/BRIDGE.md).
        let limit = if matches!(kind, CoverKind::Bluish) { 50 } else { 100 };
        assert!(false_plates * limit <= covered_rows, "{kind:?}: {false_plates} of {covered_rows}");
    }
    println!("covers over the plate start: {all_false} of {all_rows} covered rows taken for a plate");
}

#[test]
fn tag_rect_hides_the_blended_tag_edges() {
    // Review regression (A2 slivers): with fractional block edges and heights, the columns and
    // rows that blend a tag block with its neighbour must be inside tag_rect, so painting
    // tag_rect and the mask leaves only ground, plate and ground-plate blends visible.
    let ground = [20.0f32, 20.0, 30.0];
    let plate = [32.0f32, 40.0, 64.0];
    // distance from the ground-to-plate blend line (WoW's own plate edges are such blends)
    let off_line = |p: [u8; 3]| {
        (0..=20)
            .map(|k| {
                let f = k as f32 / 20.0;
                (0..3).map(|c| (f32::from(p[c]) - (ground[c] + f * (plate[c] - ground[c]))).abs()).fold(0.0, f32::max)
            })
            .fold(f32::MAX, f32::min)
    };
    let mut worst = 0.0f32;
    let mut cases = 0;
    for (bw, bh) in [(5.6, 14.0), (7.3, 13.5), (4.6, 16.25)] {
        for ox in 0..5 {
            for oy in 0..5 {
                let mut cv = Canvas::new(420, 50, ground);
                cv.aa = true;
                let t = synth::draw_tag(&mut cv, &TagDraw {
                    slot: 33,
                    row: 1,
                    sess_bit: 1,
                    x: 30.0 + f64::from(ox) * 0.2,
                    y: 12.0 + f64::from(oy) * 0.2,
                    block_w: bw,
                    block_h: bh,
                    gap: 0.0,
                    alpha: 1.0,
                    plate_w: 200.0,
                    colors: None,
                });
                let cap = cv.capture();
                let img = cap.bgra();
                let mut hits = Vec::new();
                tags::scan(&img, image::Rect { x: 0, y: 0, w: 420, h: 50 }, None, &mut hits);
                let view = View { id: 1, x: 0, y: 0, w: 420, h: 50, font_px: 14, row_px: 16 };
                let cs = cards::build(&img, &hits, &[view], &|_| Some(true), Some(1));
                assert_eq!(cs.len(), 1, "bw {bw} bh {bh} offset ({ox}, {oy})");
                let b = &cs[0].bands[0];
                let r = b.tag_rect;
                let painted = |x: i32, y: i32| {
                    (x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h) || b.mask.as_ref().is_some_and(|m| m.get(x, y))
                };
                let (y0, y1) = (t.y0.floor() as i32 - 1, t.y1.ceil() as i32 + 1);
                for y in y0..=y1 {
                    for x in (t.x0.floor() as i32 - 2)..=(t.x1.ceil() as i32 + 2) {
                        if !painted(x, y) {
                            let d = off_line(img.rgb(x, y).unwrap());
                            assert!(d <= 20.0, "bw {bw} bh {bh} offset ({ox}, {oy}): unpainted tag colour at ({x}, {y}), {d:.0} off");
                            worst = worst.max(d);
                        }
                    }
                }
                cases += 1;
            }
        }
    }
    println!("tag edge cases {cases}, largest unpainted colour step {worst:.1}");
}
