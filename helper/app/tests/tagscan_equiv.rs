//! The Helper's tag search (`tp_helper::tagscan`: candidate row bands, then `tags::scan` on those
//! bands only, and reuse of unchanged tags) must find exactly what `tags::scan` finds on the whole
//! area, on the bridge's synthetic chat panels (worlds, dark chat backgrounds, coloured text, raid
//! icons, faded tags, covers, clipped tops).

use tp_bridge::image::Rect;
use tp_bridge::synth::{self, Canvas, PanelOpts, Rng};
use tp_bridge::tags::{self, TagHit};
use tp_helper::tagscan::TagScanner;

fn key(h: &TagHit) -> (u8, u8, u8, i32, i32, i32, i32) {
    (h.slot, h.row, h.sess_bit, h.x0, h.x1, h.y_top, h.y_bot)
}

fn sorted(v: &[TagHit]) -> Vec<(u8, u8, u8, i32, i32, i32, i32)> {
    let mut k: Vec<_> = v.iter().map(key).collect();
    k.sort_unstable();
    k
}

#[test]
fn same_hits_as_a_full_scan() {
    let mut rng = Rng::new(0x5CA7);
    let (mut panels, mut tags_found, mut area_rows, mut scanned_rows) = (0usize, 0usize, 0u64, 0u64);
    for i in 0..400 {
        let opts = PanelOpts {
            tags: i % 5 != 4,
            clip_top: i % 4 == 1,
            icons_p: if i % 3 == 0 { 0.2 } else { 0.05 },
            covers: (i % 3) as usize,
            ..PanelOpts::default()
        };
        let p = synth::chat_panel(&mut rng, 8, 30, &opts);
        let mut cv = Canvas::new(opts.w + 40, opts.h + 60, [10.0, 10.0, 10.0]);
        cv.blit(&p.canvas);
        let cap = cv.capture();
        let img = cap.bgra();
        let wb = if i % 2 == 0 { None } else { Some(([250.0, 248.0, 252.0], [3.0, 2.0, 4.0])) };
        let mut full = Vec::new();
        tags::scan(&img, p.view, wb, &mut full);
        let mut scanner = TagScanner::new();
        let mut mine = Vec::new();
        let st = scanner.scan(&img, &[p.view], wb, &mut mine);
        assert_eq!(sorted(&mine), sorted(&full), "panel {i}");
        // The same picture again: same hits, read from memory where the tags did not change.
        let mut again = Vec::new();
        let st2 = scanner.scan(&img, &[p.view], wb, &mut again);
        assert_eq!(sorted(&again), sorted(&full), "panel {i}, second tick");
        if !full.is_empty() {
            assert!(st2.reused_bands > 0, "panel {i}: unchanged tags are reused");
        }
        panels += 1;
        tags_found += full.len();
        area_rows += u64::from(st.area_rows);
        scanned_rows += u64::from(st.scanned_rows);
    }
    assert!(tags_found > 500, "the panels hold tags: {tags_found}");
    eprintln!("{panels} panels, {tags_found} tags, scanned {scanned_rows} of {area_rows} rows");
    assert!(scanned_rows * 10 < area_rows * 9, "the bands skip rows: {scanned_rows} of {area_rows}");
}

#[test]
fn a_changed_tag_is_read_again() {
    let mut rng = Rng::new(0xC4A9);
    let mut checked = 0;
    for _ in 0..60 {
        let opts = PanelOpts { alpha: (0.9, 1.0), ..PanelOpts::default() };
        let p = synth::chat_panel(&mut rng, 0, 0, &opts);
        let cap = p.canvas.capture();
        let mut full = Vec::new();
        tags::scan(&cap.bgra(), p.view, None, &mut full);
        let Some(first) = full.first().cloned() else { continue };
        let mut scanner = TagScanner::new();
        let mut hits = Vec::new();
        scanner.scan(&cap.bgra(), &[p.view], None, &mut hits);
        // Blacken the tag's data blocks: the tag is gone from the next capture.
        let mut buf = cap.buf.clone();
        let stride = cap.w as usize * 4;
        for y in first.y_top..=first.y_bot {
            for x in (first.x0 + (2.0 * first.pitch_px) as i32)..first.x1 {
                let i = (y - cap.oy) as usize * stride + (x - cap.ox) as usize * 4;
                buf[i..i + 3].copy_from_slice(&[0, 0, 0]);
            }
        }
        let img = tp_bridge::image::Bgra { px: &buf, width: cap.w, height: cap.h, stride: cap.w * 4, origin_x: cap.ox, origin_y: cap.oy };
        let mut expect = Vec::new();
        tags::scan(&img, p.view, None, &mut expect);
        let mut got = Vec::new();
        scanner.scan(&img, &[p.view], None, &mut got);
        assert_eq!(sorted(&got), sorted(&expect));
        assert!(!got.iter().any(|h| key(h) == key(&first)), "the changed tag is not reused");
        checked += 1;
    }
    assert!(checked > 20, "{checked}");
}

#[test]
fn several_areas_and_an_offset_image() {
    // Two panels side by side in one capture whose origin is not (0, 0), scanned as two areas.
    let mut rng = Rng::new(0x2A2A);
    for _ in 0..40 {
        let opts = PanelOpts::default();
        let a = synth::chat_panel(&mut rng, 100, 500, &opts);
        let b = synth::chat_panel(&mut rng, 100 + opts.w + 24, 540, &opts);
        let mut cv = Canvas::new(2 * opts.w + 60, opts.h + 120, [10.0, 10.0, 10.0]);
        cv.ox = 90;
        cv.oy = 480;
        cv.blit(&a.canvas);
        cv.blit(&b.canvas);
        let cap = cv.capture();
        let img = cap.bgra();
        let mut full = Vec::new();
        tags::scan(&img, a.view, None, &mut full);
        tags::scan(&img, b.view, None, &mut full);
        let mut mine = Vec::new();
        TagScanner::new().scan(&img, &[a.view, b.view], None, &mut mine);
        assert_eq!(sorted(&mine), sorted(&full));
        // An area partly outside the image is clipped like tags::scan clips it.
        let wide = Rect { x: a.view.x - 50, y: a.view.y - 30, w: a.view.w + 100, h: a.view.h + 400 };
        let mut f2 = Vec::new();
        tags::scan(&img, wide, None, &mut f2);
        let mut m2 = Vec::new();
        TagScanner::new().scan(&img, &[wide], None, &mut m2);
        assert_eq!(sorted(&m2), sorted(&f2));
    }
}

/// A crop of `img` to `r` (client coordinates, inside the image): what a capture region of that
/// rectangle holds.
fn crop<'a>(img: &tp_bridge::image::Bgra<'a>, r: Rect) -> tp_bridge::image::Bgra<'a> {
    let (lx, ly) = ((r.x - img.origin_x) as usize, (r.y - img.origin_y) as usize);
    let start = ly * img.stride as usize + lx * 4;
    let len = (r.h as usize - 1) * img.stride as usize + r.w as usize * 4;
    tp_bridge::image::Bgra { px: &img.px[start..start + len], width: r.w as u32, height: r.h as u32, stride: img.stride, origin_x: r.x, origin_y: r.y }
}

/// Design B.3 H7 and H8: the conversation window's view over a chat frame's view. Each view is
/// its own capture region with its own tag search; the tags both regions read are kept once, with
/// the window's reading (scanned first), and that is exactly what a full scan of the window finds.
#[test]
fn overlapping_views_keep_each_tag_once() {
    use std::collections::HashSet;
    use tp_helper::app::dedup_hits;
    let mut rng = Rng::new(0xD0D0);
    let (mut dropped, mut kept_own, mut window_tags) = (0usize, 0usize, 0usize);
    for i in 0..120 {
        let opts = PanelOpts { covers: (i % 2) as usize, ..PanelOpts::default() };
        let p = synth::chat_panel(&mut rng, 40, 60, &opts);
        let mut cv = Canvas::new(opts.w + 120, opts.h + 140, [10.0, 10.0, 10.0]);
        cv.blit(&p.canvas);
        let cap = cv.capture();
        let img = cap.bgra();
        let window = p.view;
        // the chat frame under it is larger on every side
        let chat = Rect { x: window.x - 30, y: window.y - 40, w: window.w + 60, h: window.h + 80 };
        let mut full = Vec::new();
        tags::scan(&img, window, None, &mut full);
        let (img_w, img_c) = (crop(&img, window), crop(&img, chat));
        let mut seen = HashSet::new();
        let mut from_window = Vec::new();
        TagScanner::new().scan(&img_w, &[window], None, &mut from_window);
        dedup_hits(&mut from_window, &mut seen);
        let mut from_chat = Vec::new();
        TagScanner::new().scan(&img_c, &[chat], None, &mut from_chat);
        let before = from_chat.len();
        dedup_hits(&mut from_chat, &mut seen);
        assert_eq!(sorted(&from_window), sorted(&full), "panel {i}: the window region reads what a full scan reads");
        let mut all = from_window.clone();
        all.extend(from_chat.iter().cloned());
        let keys: Vec<_> = all.iter().map(|h| (h.slot, h.row, h.x0, h.y_top)).collect();
        let unique: HashSet<_> = keys.iter().copied().collect();
        assert_eq!(keys.len(), unique.len(), "panel {i}: no tag twice");
        assert!(from_chat.iter().all(|h| !from_window.iter().any(|w| (w.slot, w.row, w.x0, w.y_top) == (h.slot, h.row, h.x0, h.y_top))));
        dropped += before - from_chat.len();
        kept_own += from_chat.len();
        window_tags += from_window.len();
    }
    assert!(window_tags > 100, "the panels hold tags: {window_tags}");
    assert!(dropped * 10 >= window_tags * 9, "the chat region read the window's tags again: {dropped} of {window_tags} dropped");
    eprintln!("{window_tags} window tags, {dropped} duplicate readings dropped, {kept_own} kept from the chat region");
}

type CardKey = (u8, u8, u32, bool, [u8; 3], i32, Vec<(u8, [i32; 4], [i32; 4], Option<([i32; 4], Vec<u8>)>)>, Vec<[i32; 4]>);

fn r4(r: &Rect) -> [i32; 4] {
    [r.x, r.y, r.w, r.h]
}

fn card_keys(cards: &[tp_bridge::cards::Card]) -> Vec<CardKey> {
    cards
        .iter()
        .map(|c| {
            (
                c.slot,
                c.view_id,
                c.alpha.to_bits(),
                c.plate,
                c.plate_rgb,
                c.text_width_px,
                c.bands.iter().map(|b| (b.row, r4(&b.text_rect), r4(&b.tag_rect), b.mask.as_ref().map(|m| (r4(&m.rect), m.bits.clone())))).collect(),
                c.gaps.iter().map(r4).collect(),
            )
        })
        .collect()
}

#[test]
fn kept_cards_equal_a_new_build() {
    use tp_bridge::cards;
    use tp_bridge::msg::View;
    use tp_helper::tagscan::CardCache;
    let mut rng = Rng::new(0xCA4D);
    let (mut kept, mut rebuilt) = (0, 0);
    for i in 0..150 {
        let opts = PanelOpts { covers: (i % 2) as usize, ..PanelOpts::default() };
        let p = synth::chat_panel(&mut rng, 0, 0, &opts);
        let cap = p.canvas.capture();
        let img = cap.bgra();
        let mut hits = Vec::new();
        tags::scan(&img, p.view, None, &mut hits);
        if hits.is_empty() {
            continue;
        }
        let views = [View { id: 1, x: p.view.x as u16, y: p.view.y as u16, w: p.view.w as u16, h: p.view.h as u16, font_px: 14, row_px: 18 }];
        let plate = |slot: u8| if slot % 5 == 0 { None } else { Some(slot % 7 != 0) };
        let mut cache = CardCache::new();
        let (a, k1) = cache.build(&img, 1, None, &hits, &views, &plate, None);
        assert!(!k1);
        assert_eq!(card_keys(&a), card_keys(&cards::build(&img, &hits, &views, &plate, None)));
        // The next capture holds the same pixels: kept, and equal.
        let same = cap.bgra();
        let (b, k2) = cache.build(&same, 2, Some((&img, 1)), &hits, &views, &plate, None);
        assert!(k2, "panel {i}");
        assert_eq!(card_keys(&b), card_keys(&a));
        // A previous capture that is not the one the cards came from: built again.
        assert!(!cache.build(&same, 4, Some((&img, 3)), &hits, &views, &plate, None).1);
        assert!(cache.build(&same, 5, Some((&img, 4)), &hits, &views, &plate, None).1);
        kept += 1;
        // One pixel changes anywhere (here the far right of a tag row): built again.
        let h = &hits[0];
        let mut buf = cap.buf.clone();
        let x = cap.w as usize - 1;
        let i0 = (h.y_top + (h.y_bot - h.y_top) / 2 - cap.oy) as usize * cap.w as usize * 4 + x * 4;
        buf[i0] = buf[i0].wrapping_add(97);
        let img2 = tp_bridge::image::Bgra { px: &buf, width: cap.w, height: cap.h, stride: cap.w * 4, origin_x: cap.ox, origin_y: cap.oy };
        let (c, k3) = cache.build(&img2, 6, Some((&img, 5)), &hits, &views, &plate, None);
        assert!(!k3, "panel {i}: a changed picture is built again");
        assert_eq!(card_keys(&c), card_keys(&cards::build(&img2, &hits, &views, &plate, None)));
        // A pixel far from every tag row also counts (the build may look around the rows).
        let mut buf3 = buf.clone();
        buf3[0] = buf3[0].wrapping_add(1);
        let img3 = tp_bridge::image::Bgra { px: &buf3, width: cap.w, height: cap.h, stride: cap.w * 4, origin_x: cap.ox, origin_y: cap.oy };
        assert!(!cache.build(&img3, 7, Some((&img2, 6)), &hits, &views, &plate, None).1);
        // Another plate flag or session bit: built again.
        let other = |slot: u8| Some(plate(slot) != Some(true));
        assert!(!cache.build(&img3, 8, Some((&img3, 7)), &hits, &views, &other, None).1);
        assert!(!cache.build(&img3, 9, Some((&img3, 8)), &hits, &views, &other, Some(1)).1);
        assert!(cache.build(&img3, 10, Some((&img3, 9)), &hits, &views, &other, Some(1)).1);
        rebuilt += 1;
    }
    assert!(kept > 50 && rebuilt > 50, "{kept} {rebuilt}");
}
