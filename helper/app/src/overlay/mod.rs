//! The overlay: one layered window per chat view, drawn from the published scene and the job
//! statuses. With `--no-overlay` (amendment A1) the same rendering goes into in-memory DIBs and no
//! window is ever created.
//!
//! Native look: each card's reserved rows are painted over with the chat background sampled
//! around them (`backdrop`), and the text is drawn like chat text: the chat font size, a 1 px
//! black shadow, greyscale antialiasing; statuses in a muted grey.
//!
//! CPU (P4b): each view keeps its canvas between renders (only the dirty part is cleared and
//! copied), and wrapped lines and their rasterized coverage are cached, so a render after a small
//! change (alpha, a mask, a status) does not measure or rasterize text again.

pub mod fit;
pub mod fonts;
pub mod render;
pub mod window;

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use tp_bridge::cards::Card;
use tp_bridge::image::Rect;
use tp_engine::{JobKey, Status};

use crate::i18n::{self, Lang};
use crate::scene::{CardKind, Scene, SceneCard, SceneView};
use fit::Fitted;
use fonts::{face_for_target, Face, Fonts, SS};
use render::{paint_card, Canvas, Coverage, PaintStats, TextDraw};
use window::{changed_region, Dib, LayeredWindow, OVERLAY_CLASS};

/// Status lines ("Translating..." and the like): a muted grey, distinct from the translation.
pub const STATUS_RGB: [u8; 3] = [0xA0, 0xA0, 0xA0];
pub const FAILURE_RGB: [u8; 3] = [0xE0, 0x80, 0x80];
/// The font is capped so that its cell (ascent + descent) is at most the band height plus this
/// many px: text is drawn vertically centred in the band, and a character height equal to the
/// band clipped accents and descenders (2.1-2.6% of a French line's ink, measured 2026-10-18).
pub const FONT_CELL_SLACK: f32 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Text,
    Status,
    Failure,
}

/// What one card shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    /// Nothing but the tag cover (no status yet, or a pending unknown slot).
    Empty,
    /// The translation, wrapped into the request's rows: line i goes to the band of row i.
    Lines { text: String, target: String, rows: usize },
    /// One status line in the first visible band.
    Status { text: String, tone: Tone, animated: bool },
    /// One line per band (slot 127 test rows): (band index, text).
    PerBand(Vec<(usize, String)>),
}

/// Engine status name for logs and `--diag-json`.
pub fn status_name(s: &Status) -> &'static str {
    match s {
        Status::Queued { .. } => "queued",
        Status::Loading { .. } => "loading",
        Status::Translating => "translating",
        Status::Partial(_) => "partial",
        Status::Done(_) => "done",
        Status::Same => "same",
        Status::Nothing => "nothing",
        Status::Failed(_) => "failed",
        Status::LowMemory => "lowmem",
        Status::NotInstalled => "notinstalled",
    }
}

/// Decides what a card shows. `fallback` is the language for cards without a request.
pub fn card_content(sc: &SceneCard, status: Option<&Status>, fallback: Lang) -> Content {
    match &sc.kind {
        CardKind::Pending => Content::Empty,
        CardKind::Missing => Content::Status { text: i18n::tr(fallback, "status.missing").to_string(), tone: Tone::Status, animated: false },
        CardKind::Test => {
            let block = format!("{}", sc.block_px.round() as i32);
            Content::PerBand(
                sc.card
                    .bands
                    .iter()
                    .enumerate()
                    .map(|(i, b)| (i, i18n::tr_fmt(fallback, "status.overlay_ok", &[("row", &b.row.to_string()), ("block", &block)])))
                    .collect(),
            )
        }
        CardKind::Request { rows, target, .. } => {
            let lang = i18n::for_target(target);
            let st = |key: &str, tone: Tone, animated: bool| Content::Status { text: i18n::tr(lang, key).to_string(), tone, animated };
            match status {
                None => Content::Empty,
                Some(Status::Queued { ahead }) => Content::Status {
                    text: i18n::tr_fmt(lang, "status.queued", &[("n", &ahead.to_string())]),
                    tone: Tone::Status,
                    animated: true,
                },
                Some(Status::Loading { first_gpu_use: false }) => st("status.loading", Tone::Status, true),
                Some(Status::Loading { first_gpu_use: true }) => st("status.loading_gpu_first", Tone::Status, true),
                Some(Status::Translating) => st("status.translating", Tone::Status, true),
                Some(Status::Partial(t)) | Some(Status::Done(t)) => {
                    Content::Lines { text: t.as_str().to_string(), target: target.clone(), rows: usize::from(*rows).max(1) }
                }
                Some(Status::Same) => Content::Status {
                    text: i18n::tr_fmt(lang, "status.same", &[("language", i18n::language_name(lang, target))]),
                    tone: Tone::Status,
                    animated: false,
                },
                Some(Status::Nothing) => st("status.nothing", Tone::Status, false),
                Some(Status::Failed(_)) => st("status.failed", Tone::Failure, false),
                Some(Status::LowMemory) => st("status.lowmem", Tone::Failure, false),
                Some(Status::NotInstalled) => st("status.notinstalled", Tone::Failure, false),
            }
        }
    }
}

/// Status pulse (0.6..1.0) for the "Translating..." animation, `t` in seconds.
pub fn pulse(t: f64) -> f32 {
    (0.6 + 0.4 * (0.5 + 0.5 * (t * std::f64::consts::TAU / 1.2).cos())) as f32
}

enum Surface {
    Window(LayeredWindow),
    /// `--no-overlay`: the last rendering, kept in memory only, with the dirty rectangle
    /// (canvas-local) of the canvas it was copied from.
    Memory(Option<(Dib, Option<Rect>)>),
}

/// What happened to one view in a render pass (for `--diag-json`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewReport {
    pub id: u8,
    pub shown: bool,
    /// Client coordinates.
    pub rect: [i32; 4],
    pub px: usize,
    pub stats: PaintStats,
    /// A window is on screen (false under `--no-overlay`).
    pub window: bool,
}

/// Cache key of a rasterized line or a fitted text: the text's hash plus the parameters (font
/// sizes in supersampled pixels); the cached entry keeps the text itself to rule out hash
/// collisions.
type CovKey = (Face, i32, i32, i32, u64);
type FitKey = (Face, i32, i32, usize, u64);
const CACHE_MAX: usize = 256;

fn text_hash(s: &str) -> u64 {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

pub struct OverlayManager {
    no_overlay: bool,
    diag_outlines: bool,
    exclude: bool,
    text_rgb: [u8; 3],
    font_scale: f32,
    surfaces: HashMap<u8, Surface>,
    /// One canvas per view, reused between renders.
    canvases: HashMap<u8, Canvas>,
    fonts: Fonts,
    cov_cache: HashMap<CovKey, (String, Rc<Coverage>)>,
    fit_cache: HashMap<FitKey, (String, Fitted)>,
    /// Per view: (shown, rect, painted px, text px) of the last report. The text count is part of
    /// it: with the native look the plate is painted before any text, so a status or a
    /// translation appearing leaves the painted count unchanged.
    last: HashMap<u8, (bool, [i32; 4], usize, usize)>,
    /// A visible card shows an animated status (the UI runs a 10 Hz timer while true).
    pub animating: bool,
}

impl OverlayManager {
    pub fn new(no_overlay: bool, diag_outlines: bool, text_rgb: [u8; 3], font_scale: f32) -> OverlayManager {
        OverlayManager {
            no_overlay,
            diag_outlines,
            exclude: false,
            text_rgb,
            font_scale,
            surfaces: HashMap::new(),
            canvases: HashMap::new(),
            fonts: Fonts::new(),
            cov_cache: HashMap::new(),
            fit_cache: HashMap::new(),
            last: HashMap::new(),
            animating: false,
        }
    }

    /// Display affinity of every overlay window (`WDA_EXCLUDEFROMCAPTURE` when true).
    /// Returns false when a window refused it.
    pub fn set_exclude(&mut self, on: bool) -> bool {
        self.exclude = on;
        let mut ok = true;
        for s in self.surfaces.values_mut() {
            if let Surface::Window(w) = s {
                ok &= w.set_exclude(on);
            }
        }
        ok
    }

    /// Handles of the overlay windows (the probe checks hit testing against them).
    pub fn hwnds(&self) -> Vec<isize> {
        self.surfaces
            .values()
            .filter_map(|s| match s {
                Surface::Window(w) => Some(w.hwnd()),
                Surface::Memory(_) => None,
            })
            .collect()
    }

    /// True when a window is on screen.
    pub fn any_shown(&self) -> bool {
        self.surfaces.values().any(|s| matches!(s, Surface::Window(w) if w.shown()))
    }

    /// Coverage of one line at `size` supersampled pixels.
    fn coverage(&mut self, face: Face, size: i32, text: &str, w: i32, h: i32) -> Rc<Coverage> {
        let key = (face, size, w, h, text_hash(text));
        if let Some((t, c)) = self.cov_cache.get(&key) {
            if t == text {
                return c.clone();
            }
        }
        if self.cov_cache.len() >= CACHE_MAX {
            self.cov_cache.clear();
        }
        let c = Rc::new(self.fonts.coverage_ss(face, size, text, w, h));
        self.cov_cache.insert(key, (text.to_string(), c.clone()));
        c
    }

    /// `fit::fit` at `size` supersampled pixels, with the result cached (the wrap measures every
    /// candidate line with GDI).
    fn fitted(&mut self, face: Face, size: i32, text: &str, rows: usize, width: i32) -> Fitted {
        let key = (face, size, width, rows, text_hash(text));
        if let Some((t, f)) = self.fit_cache.get(&key) {
            if t == text {
                return f.clone();
            }
        }
        if self.fit_cache.len() >= CACHE_MAX {
            self.fit_cache.clear();
        }
        let fonts = &mut self.fonts;
        let f = fit::fit(text, rows, width, size, &mut |s, sz| fonts.measure_ss(face, sz, s));
        self.fit_cache.insert(key, (text.to_string(), f.clone()));
        f
    }

    /// Font size for a card in supersampled pixels: VIEW.fontPx (the chat font) x fontScale,
    /// capped so that the cell of `face` fits the band height plus `FONT_CELL_SLACK`.
    fn font_size(&mut self, view: &SceneView, card: &Card, face: Face) -> i32 {
        let band_h = card.bands.iter().map(|b| b.text_rect.h).min().unwrap_or(0) as f32;
        let base = if view.font_px > 0 { f32::from(view.font_px) * self.font_scale } else { band_h * self.font_scale };
        let cap = ((band_h + FONT_CELL_SLACK) / self.fonts.cell_ratio(face)).min(band_h);
        (base.min(cap) * SS as f32).round() as i32
    }

    fn paint(&mut self, canvas: &mut Canvas, view: &SceneView, sc: &SceneCard, results: &HashMap<JobKey, Status>, fallback: Lang, pulse_mul: f32) -> PaintStats {
        let card = &sc.card;
        let status = match &sc.kind {
            CardKind::Request { key, .. } => results.get(key),
            _ => None,
        };
        let content = card_content(sc, status, fallback);
        let size = self.font_size(view, card, Face::Latin);
        let width = if card.text_width_px > 0 { card.text_width_px } else { card.bands.iter().map(|b| b.text_rect.w).max().unwrap_or(0) };
        // (band index, line, face, supersampled size, colour, alpha multiplier)
        let mut lines: Vec<(usize, String, Face, i32, [u8; 3], f32)> = Vec::new();
        if size >= 6 * SS && width > 0 {
            match content {
                Content::Empty => {}
                Content::Lines { text, target, rows } => {
                    let face = face_for_target(&target);
                    let size = self.font_size(view, card, face);
                    let fitted = self.fitted(face, size, &text, rows, width);
                    for (i, line) in fitted.lines.into_iter().enumerate() {
                        if let Some(bi) = card.bands.iter().position(|b| usize::from(b.row) == i) {
                            lines.push((bi, line, face, fitted.size, self.text_rgb, 1.0));
                        }
                    }
                }
                Content::Status { text, tone, animated } => {
                    let rgb = match tone {
                        Tone::Text => self.text_rgb,
                        Tone::Status => STATUS_RGB,
                        Tone::Failure => FAILURE_RGB,
                    };
                    if animated {
                        self.animating = true;
                    }
                    let fitted = self.fitted(Face::Latin, size, &text, 1, width);
                    let first = card.bands.iter().enumerate().min_by_key(|(_, b)| b.row).map(|(i, _)| i);
                    if let (Some(bi), Some(line)) = (first, fitted.lines.into_iter().next()) {
                        lines.push((bi, line, Face::Latin, fitted.size, rgb, if animated { pulse_mul } else { 1.0 }));
                    }
                }
                Content::PerBand(list) => {
                    for (bi, text) in list {
                        let fitted = self.fitted(Face::Latin, size, &text, 1, width);
                        if let Some(line) = fitted.lines.into_iter().next() {
                            lines.push((bi, line, Face::Latin, fitted.size, STATUS_RGB, 1.0));
                        }
                    }
                }
            }
        }
        let covs: Vec<(usize, Rc<Coverage>, [u8; 3], f32)> = lines
            .into_iter()
            .map(|(bi, line, face, lsize, rgb, mul)| {
                let tr = card.bands[bi].text_rect;
                (bi, self.coverage(face, lsize, &line, tr.w, tr.h), rgb, mul)
            })
            .collect();
        let texts: Vec<TextDraw> = covs.iter().map(|(bi, cov, rgb, mul)| TextDraw { band: *bi, cov: cov.as_ref(), rgb: *rgb, alpha_mul: *mul }).collect();
        paint_card(canvas, card, &sc.bg, &texts, self.diag_outlines)
    }

    /// Draws the scene. Returns the views whose state changed (for `--diag-json`).
    pub fn render(&mut self, scene: &Scene, results: &HashMap<JobKey, Status>, pulse_mul: f32, ui_lang: Lang) -> Vec<ViewReport> {
        self.animating = false;
        let fallback = scene.lang_target.as_deref().map(i18n::for_target).unwrap_or(ui_lang);
        let mut reports = Vec::new();
        let mut live: Vec<u8> = Vec::new();
        if let (None, Some(wow)) = (scene.hidden, scene.wow) {
            for view in &scene.views {
                if view.rect.w <= 0 || view.rect.h <= 0 {
                    continue;
                }
                let mut canvas = self.canvases.remove(&view.id).unwrap_or_else(|| Canvas::new(view.rect));
                canvas.reset(view.rect);
                let mut stats = PaintStats::default();
                for sc in view.cards.iter().filter(|c| !c.covered) {
                    let s = self.paint(&mut canvas, view, sc, results, fallback, pulse_mul);
                    stats.tag_px += s.tag_px;
                    stats.fill_px += s.fill_px;
                    stats.text_px += s.text_px;
                }
                let px = canvas.painted();
                let r = view.rect;
                if px == 0 {
                    self.canvases.insert(view.id, canvas);
                    continue;
                }
                let shown = self.present(view.id, wow.origin_x + r.x, wow.origin_y + r.y, &canvas);
                self.canvases.insert(view.id, canvas);
                live.push(view.id);
                reports.push(ViewReport { id: view.id, shown, rect: [r.x, r.y, r.w, r.h], px, stats, window: !self.no_overlay });
            }
        }
        let ids: Vec<u8> = self.surfaces.keys().copied().collect();
        for id in ids {
            if !live.contains(&id) {
                self.hide(id);
            }
        }
        // Canvases of views that left the scene are not kept.
        self.canvases.retain(|id, _| scene.views.iter().any(|v| v.id == *id));
        let prev: Vec<u8> = self.last.keys().copied().collect();
        for id in prev {
            if !live.contains(&id) {
                if let Some((true, rect, _, _)) = self.last.get(&id).copied() {
                    reports.push(ViewReport { id, shown: false, rect, px: 0, stats: PaintStats::default(), window: !self.no_overlay });
                }
            }
        }
        // Keep only changes.
        let mut changed = Vec::new();
        for r in reports {
            let state = (r.shown, r.rect, r.px, r.stats.text_px);
            if self.last.get(&r.id) != Some(&state) {
                self.last.insert(r.id, state);
                changed.push(r);
            }
        }
        changed
    }

    fn present(&mut self, id: u8, x: i32, y: i32, canvas: &Canvas) -> bool {
        if self.no_overlay {
            let s = self.surfaces.entry(id).or_insert(Surface::Memory(None));
            if let Surface::Memory(mem) = s {
                let local = canvas.dirty().map(|r| Rect { x: r.x - canvas.rect.x, y: r.y - canvas.rect.y, w: r.w, h: r.h });
                // Same size: only the changed part; else a new DIB and a full copy.
                if let Some((d, prev)) = mem.as_mut().filter(|(d, _)| d.w == canvas.rect.w && d.h == canvas.rect.h) {
                    let ok = changed_region(canvas, *prev).is_none_or(|r| d.copy_region(canvas, r));
                    *prev = local;
                    return ok;
                }
                *mem = Dib::new(canvas.rect.w, canvas.rect.h).map(|d| (d, local));
                return mem.as_mut().is_some_and(|(d, _)| d.copy_from(canvas));
            }
            return false;
        }
        if !self.surfaces.contains_key(&id) {
            let Some(mut w) = LayeredWindow::create(OVERLAY_CLASS) else { return false };
            w.set_exclude(self.exclude);
            self.surfaces.insert(id, Surface::Window(w));
        }
        match self.surfaces.get_mut(&id) {
            Some(Surface::Window(w)) => w.present(x, y, canvas),
            _ => false,
        }
    }

    fn hide(&mut self, id: u8) {
        match self.surfaces.get_mut(&id) {
            Some(Surface::Window(w)) => w.hide(),
            Some(Surface::Memory(d)) => *d = None,
            None => {}
        }
    }

    /// Hides every view (game gone, exit).
    pub fn hide_all(&mut self) {
        let ids: Vec<u8> = self.surfaces.keys().copied().collect();
        for id in ids {
            self.hide(id);
        }
        self.last.clear();
    }

    /// Bytes of the in-memory rendering of a view (`--no-overlay`), for tests.
    pub fn memory_pixels(&self, id: u8) -> Option<Vec<u8>> {
        match self.surfaces.get(&id) {
            Some(Surface::Memory(Some((d, _)))) => Some(d.bits().to_vec()),
            _ => None,
        }
    }
}

/// Union of a card's band and tag rectangles (client coordinates).
pub fn card_bounds(card: &Card) -> Option<Rect> {
    let mut it = card.bands.iter().flat_map(|b| [b.text_rect, b.tag_rect]).filter(|r| r.w > 0 && r.h > 0);
    let first = it.next()?;
    Some(it.fold(first, crate::capture::union))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::tests::card;
    use crate::scene::{Hidden, WowGeom};
    use tp_engine::ChatText;

    fn scene_with(cards: Vec<SceneCard>) -> Scene {
        Scene {
            wow: Some(WowGeom { hwnd: 1, origin_x: 0, origin_y: 0, client_w: 1920, client_h: 1080 }),
            views: vec![SceneView { id: 0, rect: Rect { x: 0, y: 16, w: 400, h: 1000 }, font_px: 14, cards }],
            hidden: None,
            lang_target: None,
            gdi: false,
        }
    }

    fn req(slot: u8, req_id: u16, target: &str) -> SceneCard {
        SceneCard {
            card: card(slot, 0, 900, 2, 1.0),
            kind: CardKind::Request { key: JobKey { session: 9, req_id }, rows: 2, target: target.into() },
            covered: false,
            block_px: 5.0,
            bg: crate::backdrop::CardBg::default(),
        }
    }

    #[test]
    fn content_per_status() {
        let sc = req(3, 1, "fr");
        assert_eq!(card_content(&sc, None, Lang::En), Content::Empty);
        assert_eq!(
            card_content(&sc, Some(&Status::Queued { ahead: 2 }), Lang::En),
            Content::Status { text: "En attente (2 avant)...".into(), tone: Tone::Status, animated: true }
        );
        assert_eq!(
            card_content(&sc, Some(&Status::Same), Lang::En),
            Content::Status { text: "Déjà en français".into(), tone: Tone::Status, animated: false }
        );
        let de = req(3, 1, "de");
        assert_eq!(
            card_content(&de, Some(&Status::Failed(tp_engine::FailKind::Timeout)), Lang::Fr),
            Content::Status { text: "Translation failed - Shift-click [T] to retry".into(), tone: Tone::Failure, animated: false }
        );
        match card_content(&de, Some(&Status::Done(ChatText::new("HALLO WELT".into()))), Lang::En) {
            Content::Lines { text, target, rows } => assert_eq!((text.as_str(), target.as_str(), rows), ("HALLO WELT", "de", 2)),
            other => panic!("{other:?}"),
        }
        let mut missing = req(4, 0, "en");
        missing.kind = CardKind::Missing;
        assert_eq!(
            card_content(&missing, None, Lang::En),
            Content::Status { text: "Not received - click [T] again".into(), tone: Tone::Status, animated: false }
        );
        let mut test = req(127, 0, "en");
        test.kind = CardKind::Test;
        test.block_px = 6.4;
        assert_eq!(
            card_content(&test, None, Lang::En),
            Content::PerBand(vec![(0, "Overlay OK (row 0, block 6 px)".into()), (1, "Overlay OK (row 1, block 6 px)".into())])
        );
    }

    #[test]
    fn renders_into_memory_without_windows() {
        let mut m = OverlayManager::new(true, false, [0x9F, 0xD4, 0xFF], 1.0);
        let mut results = HashMap::new();
        results.insert(JobKey { session: 9, req_id: 1 }, Status::Done(ChatText::new("Bonjour tout le monde".into())));
        let reports = m.render(&scene_with(vec![req(3, 1, "fr")]), &results, 1.0, Lang::En);
        assert!(m.hwnds().is_empty(), "no window under --no-overlay");
        assert!(!m.any_shown());
        assert_eq!(reports.len(), 1);
        let r = reports[0];
        assert!(r.shown && !r.window);
        assert!(r.stats.text_px > 20, "{:?}", r.stats);
        assert_eq!(r.stats.tag_px, 2 * 47 * 14);
        let bits = m.memory_pixels(0).expect("DIB kept in memory");
        assert_eq!(bits.len(), 400 * 1000 * 4);
        assert!(bits.chunks_exact(4).any(|p| p[3] != 0));
        // Unchanged scene: no new report; then the card goes away: one "hidden" report.
        assert!(m.render(&scene_with(vec![req(3, 1, "fr")]), &results, 1.0, Lang::En).is_empty());
        let gone = m.render(&scene_with(vec![]), &results, 1.0, Lang::En);
        assert_eq!(gone.len(), 1);
        assert!(!gone[0].shown);
        assert!(m.memory_pixels(0).is_none());
    }

    #[test]
    fn covered_and_hidden_scenes_draw_nothing() {
        let mut m = OverlayManager::new(true, false, [255, 255, 255], 1.0);
        let mut c = req(3, 1, "fr");
        c.covered = true;
        assert!(m.render(&scene_with(vec![c]), &HashMap::new(), 1.0, Lang::En).is_empty());
        let mut s = scene_with(vec![req(3, 1, "fr")]);
        s.hidden = Some(Hidden::Stale);
        assert!(m.render(&s, &HashMap::new(), 1.0, Lang::En).is_empty());
        assert!(m.memory_pixels(0).is_none());
    }

    #[test]
    fn animated_status_sets_the_flag() {
        let mut m = OverlayManager::new(true, false, [255, 255, 255], 1.0);
        let mut results = HashMap::new();
        results.insert(JobKey { session: 9, req_id: 1 }, Status::Translating);
        m.render(&scene_with(vec![req(3, 1, "en")]), &results, pulse(0.0), Lang::En);
        assert!(m.animating);
        results.insert(JobKey { session: 9, req_id: 1 }, Status::Nothing);
        m.render(&scene_with(vec![req(3, 1, "en")]), &results, 1.0, Lang::En);
        assert!(!m.animating);
        assert!((pulse(0.0) - 1.0).abs() < 1e-6);
        assert!((pulse(0.6) - 0.6).abs() < 1e-6);
    }

    #[test]
    fn long_translation_uses_every_reserved_row() {
        let mut m = OverlayManager::new(true, false, [255, 255, 255], 1.0);
        let mut results = HashMap::new();
        let long = "word ".repeat(60);
        results.insert(JobKey { session: 9, req_id: 1 }, Status::Done(ChatText::new(long)));
        let r = m.render(&scene_with(vec![req(3, 1, "en")]), &results, 1.0, Lang::En);
        let bits = m.memory_pixels(0).unwrap();
        // Both bands (y 900..913 and 916..929, x 60..259) received text pixels.
        let row_has_text = |y0: i32| (y0..y0 + 14).any(|y| (60..260).any(|x| bits[(((y - 16) * 400 + x) * 4 + 3) as usize] != 0));
        assert!(row_has_text(900) && row_has_text(916), "{r:?}");
    }

    #[test]
    fn dib_respects_mask_and_card_alpha() {
        // Right half of each plate covered (mask 0 from x 160), chat faded to 0.5.
        let mut sc = req(3, 1, "en");
        sc.card.alpha = 0.5;
        for b in sc.card.bands.iter_mut() {
            let m = b.mask.as_mut().unwrap();
            for y in 0..m.rect.h {
                for x in 0..m.rect.w {
                    if m.rect.x + x >= 160 {
                        m.bits[(y * m.rect.w + x) as usize] = 0;
                    }
                }
            }
        }
        let mut results = HashMap::new();
        results.insert(JobKey { session: 9, req_id: 1 }, Status::Done(ChatText::new("WWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWW".into())));
        let mut m = OverlayManager::new(true, false, [255, 255, 255], 1.0);
        m.render(&scene_with(vec![sc]), &results, 1.0, Lang::En);
        let bits = m.memory_pixels(0).unwrap();
        let px = |x: i32, y: i32| {
            let i = (((y - 16) * 400 + x) * 4) as usize;
            [bits[i], bits[i + 1], bits[i + 2], bits[i + 3]]
        };
        let mut left = 0;
        for y in 900..914 {
            for x in 60..260 {
                let p = px(x, y);
                if x >= 160 {
                    assert_eq!(p[3], 0, "covered pixel painted at ({x},{y})");
                } else if p[3] != 0 {
                    left += 1;
                    assert!(p[3] <= 128, "alpha at most 0.5: {}", p[3]);
                    assert!(p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3], "premultiplied");
                }
            }
        }
        assert!(left > 50, "text drawn on the visible half: {left}");
        // The tag cover is opaque: the observed plate colour already includes the fade.
        assert_eq!(px(20, 905), [64, 40, 32, 255]);
    }

    /// Render cost of a typical scene (3 finished cards with 3, 2 and 4 rows in a 645 x 433 chat
    /// view, native look: tags and plates painted with a background gradient). Run:
    /// `cargo test --release --lib render_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn render_cost() {
        let mut cards = Vec::new();
        let mut results = HashMap::new();
        for (i, rows) in [3u8, 2, 4].into_iter().enumerate() {
            let mut sc = req(3 + i as u8, 10 + i as u16, "fr");
            sc.card = card(3 + i as u8, 0, 650 + 100 * i as i32, rows, 1.0);
            sc.kind = CardKind::Request { key: JobKey { session: 9, req_id: 10 + i as u16 }, rows, target: "fr".into() };
            for b in sc.card.bands.iter_mut() {
                b.text_rect.w = 520;
                b.mask = Some(tp_bridge::cards::Mask { rect: Rect { x: 57, y: b.text_rect.y, w: 530, h: 14 }, bits: vec![255; 530 * 14] });
            }
            sc.card.text_width_px = 520;
            // the native look: the plate is painted with a background gradient
            let bg = crate::backdrop::BandBg {
                left: Some(crate::backdrop::Sample { rgb: [30, 70, 40], x: 30 }),
                right: Some(crate::backdrop::Sample { rgb: [100, 55, 30], x: 640 }),
            };
            sc.bg = crate::backdrop::CardBg { bands: vec![bg; usize::from(rows)], edges: Vec::new() };
            cards.push(sc);
            results.insert(JobKey { session: 9, req_id: 10 + i as u16 }, Status::Done(ChatText::new("QUELQU'UN POUR FINIR LA QUETE DE LA MINE CE SOIR ? ON A BESOIN D'UN SOIGNEUR ET D'UN TANK, ON PART DANS DIX MINUTES".into())));
        }
        let mut s = scene_with(cards);
        s.views[0].rect = Rect { x: 20, y: 600, w: 645, h: 433 };
        let mut m = OverlayManager::new(true, false, [0x9F, 0xD4, 0xFF], 1.0);
        m.render(&s, &results, 1.0, Lang::En);
        let n = 200;
        let t0 = std::time::Instant::now();
        for i in 0..n {
            // A different alpha each time, like a fading chat.
            for c in s.views[0].cards.iter_mut() {
                c.card.alpha = 0.5 + (i % 50) as f32 / 100.0;
            }
            m.render(&s, &results, 1.0, Lang::En);
        }
        let per = t0.elapsed().as_secs_f64() * 1000.0 / f64::from(n);
        eprintln!("render: {per:.3} ms per frame");
    }

    /// A 2-row card over a sampled background: green on the left, brown on the right.
    fn native_card(text_rows: u8) -> SceneCard {
        use crate::backdrop::{BandBg, CardBg, Sample};
        let mut sc = req(3, 1, "fr");
        sc.card = card(3, 0, 900, text_rows, 1.0);
        sc.kind = CardKind::Request { key: JobKey { session: 9, req_id: 1 }, rows: text_rows, target: "fr".into() };
        let b = BandBg { left: Some(Sample { rgb: [30, 70, 40], x: 4 }), right: Some(Sample { rgb: [100, 55, 30], x: 380 }) };
        sc.bg = CardBg { bands: vec![b; usize::from(text_rows)], edges: Vec::new() };
        sc
    }

    #[test]
    fn native_look_paints_the_background_and_no_outline() {
        let sc = native_card(2);
        let want = |x: i32| sc.bg.bands[0].at(x).unwrap();
        let mut results = HashMap::new();
        results.insert(JobKey { session: 9, req_id: 1 }, Status::Done(ChatText::new("Bonjour".into())));
        let mut m = OverlayManager::new(true, false, [0xA8, 0xD8, 0xFF], 1.0);
        m.render(&scene_with(vec![sc.clone()]), &results, 1.0, Lang::En);
        let bits = m.memory_pixels(0).unwrap();
        // canvas pixel (x, y) as (RGB, alpha); the view starts at client y 16
        let px = |x: i32, y: i32| {
            let i = (((y - 16) * 400 + x) * 4) as usize;
            ([bits[i + 2], bits[i + 1], bits[i]], bits[i + 3])
        };
        // the tag: the background gradient, opaque (no dark square)
        assert_eq!(px(20, 905), (want(20), 255));
        // plate pixels right of the text (x 200..259 of row 1 holds no text): the gradient
        for x in [210, 240, 258] {
            assert_eq!(px(x, 920), (want(x), 255), "row 1 at x {x}");
        }
        let (mut text, mut plate, mut outline) = (0, 0, 0);
        for y in 880..960 {
            for x in 0..400 {
                let (rgb, a) = px(x, y);
                if a == 0 {
                    continue;
                }
                text += usize::from(rgb[2] > 200);
                plate += usize::from(rgb == [32, 40, 64]);
                outline += usize::from(rgb == [255, 255, 0] || rgb == [0, 255, 255]);
            }
        }
        assert!(text > 10, "the translation is drawn");
        assert_eq!(plate, 0, "no plate colour (no slab)");
        assert_eq!(outline, 0, "no outline without --diag");
        // --diag adds the outlines
        let mut d = OverlayManager::new(true, true, [0xA8, 0xD8, 0xFF], 1.0);
        d.render(&scene_with(vec![sc]), &results, 1.0, Lang::En);
        let dbits = d.memory_pixels(0).unwrap();
        assert!(dbits.chunks_exact(4).any(|p| p[..3] == [0, 255, 255] && p[3] == 255), "yellow outline (BGRA)");
    }

    #[test]
    fn shrinks_to_80_percent_before_cutting_with_real_fonts() {
        let mut m = OverlayManager::new(true, false, [255, 255, 255], 1.0);
        let (size, width, rows) = (14 * SS, 200, 2);
        let min = fit::min_size(size);
        let words = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda omicron sigma upsilon omega";
        let all: Vec<&str> = words.split(' ').collect();
        let mut fonts = Fonts::new();
        let lines_at = |fonts: &mut Fonts, text: &str, sz: i32| fit::wrap(text, width, &mut |s| fonts.measure_ss(Face::Latin, sz, s)).len();
        let (mut shrunk, mut cut) = (None, None);
        for n in 1..=all.len() {
            let text = all[..n].join(" ");
            let (full, small) = (lines_at(&mut fonts, &text, size), lines_at(&mut fonts, &text, min));
            if full > rows && small <= rows && shrunk.is_none() {
                shrunk = Some(text.clone());
            }
            if small > rows && cut.is_none() {
                cut = Some(text);
            }
        }
        let shrunk = shrunk.expect("a text that fits only below the full size");
        let f = m.fitted(Face::Latin, size, &shrunk, rows, width);
        assert!(!f.cut && f.size < size && f.size >= min, "{f:?}");
        assert!(f.lines.len() <= rows);
        assert!(f.lines.iter().all(|l| fonts.measure_ss(Face::Latin, f.size, l) <= width), "{f:?}");
        let cut = cut.expect("a text too long even at 80%");
        let g = m.fitted(Face::Latin, size, &cut, rows, width);
        assert!(g.cut && g.size == min, "{g:?}");
        assert_eq!(g.lines.len(), rows);
        let last = g.lines.last().unwrap();
        assert!(last.ends_with("...") && !last.contains('\u{2026}'), "{last}");
        assert!(fonts.measure_ss(Face::Latin, g.size, last) <= width);
        // pixels: the shrunk text is drawn in both rows, never right of the text width
        let mut sc = native_card(2);
        sc.bg = crate::backdrop::CardBg::default();
        let mut results = HashMap::new();
        results.insert(JobKey { session: 9, req_id: 1 }, Status::Done(ChatText::new(shrunk)));
        m.render(&scene_with(vec![sc]), &results, 1.0, Lang::En);
        let bits = m.memory_pixels(0).unwrap();
        let glyph = |x: i32, y: i32| {
            let i = (((y - 16) * 400 + x) * 4) as usize;
            bits[i] > 128 && bits[i + 3] > 128
        };
        let row_has_text = |y0: i32| (y0..y0 + 14).any(|y| (60..260).any(|x| glyph(x, y)));
        assert!(row_has_text(900) && row_has_text(916));
        assert!((900..930).all(|y| (261..400).all(|x| !glyph(x, y))), "text past the text width");
    }

    #[test]
    fn card_bounds_union() {
        let b = card_bounds(&card(1, 0, 100, 2, 1.0)).unwrap();
        assert_eq!((b.x, b.y, b.w, b.h), (10, 100, 250, 30));
    }
}
