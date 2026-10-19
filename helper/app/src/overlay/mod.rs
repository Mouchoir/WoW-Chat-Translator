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
//!
//! Conversation window (VIEW ids 200..255, design A.4 and A.6.4):
//! - the bubbles' tags are painted over, so a line starts at the tag's left edge + 2 px and may
//!   use the tag's area as well as the plate's visible pixels;
//! - a reply card shows the Helper's status on row 0 (grey while waiting, green once the
//!   clipboard holds the translation, amber when it was not copied, red on failure) and the
//!   translation on rows 1..K-1; its hint lane is painted over with the bubble fill;
//! - "Not received" and "Translation failed" name the bubble or the Translate button instead of
//!   [T].
//!
//! Z-order (decision D12): with the game window in front, the overlay windows are topmost; when
//! another window is in front, they sit right above the game window, so that window (and any
//! other above the game) hides them, and no card is ever drawn over another application.

pub mod fit;
pub mod fonts;
pub mod render;
pub mod window;

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use tp_bridge::cards::{Card, Mask};
use tp_bridge::image::Rect;
use tp_engine::{JobKey, Status};

use crate::clip::{one_line_255, ClipOutcome};
use crate::i18n::{self, Lang};
use crate::reply::ReplyBook;
use crate::scene::{CardKind, Scene, SceneCard, SceneView, ViewKind, REPLY_SLOT};
use fit::Fitted;
use fonts::{face_for_target, Face, Fonts, SS};
use render::{paint_card, Canvas, Coverage, PaintStats, TextDraw};
use window::{changed_region, Dib, LayeredWindow, ZMode, OVERLAY_CLASS};

/// Status lines ("Translating..." and the like): a muted grey, distinct from the translation.
pub const STATUS_RGB: [u8; 3] = [0xA0, 0xA0, 0xA0];
pub const FAILURE_RGB: [u8; 3] = [0xE0, 0x80, 0x80];
/// Reply card row 0 (design A.6.4): grey 0.6 while waiting, WoW's GREEN_FONT_COLOR when the
/// clipboard holds the translation, NORMAL_FONT_COLOR (amber) when it was not copied,
/// RED_FONT_COLOR on failure.
pub const REPLY_GREY: [u8; 3] = [153, 153, 153];
pub const REPLY_GREEN: [u8; 3] = [25, 255, 25];
pub const REPLY_AMBER: [u8; 3] = [255, 209, 0];
pub const REPLY_RED: [u8; 3] = [255, 25, 25];
/// In a window view a line starts this far right of the tag's left edge (the tag is painted
/// over).
pub const WINDOW_TEXT_INSET: i32 = 2;
/// The font is capped so that its cell (ascent + descent) is at most the band height plus this
/// many px: text is drawn vertically centred in the band, and a character height equal to the
/// band clipped accents and descenders (2.1-2.6% of a French line's ink, measured 2026-10-18).
pub const FONT_CELL_SLACK: f32 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Text,
    Status,
    Failure,
    /// Reply row 0: waiting.
    ReplyGrey,
    /// Reply row 0: ready, in the clipboard.
    ReplyGreen,
    /// Reply row 0: ready, not copied.
    ReplyAmber,
    /// Reply row 0: failed or not received.
    ReplyRed,
}

/// One status line.
#[derive(Clone, Debug, PartialEq)]
pub struct StatusLine {
    pub text: String,
    pub tone: Tone,
    pub animated: bool,
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
    /// A reply card: the status on row 0, the text wrapped into rows 1..K-1 (`rows` = K - 1).
    Reply { status: StatusLine, text: Option<String>, target: String, rows: usize },
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

/// Decides what a chat card shows. `fallback` is the language for cards without a request.
pub fn card_content(sc: &SceneCard, status: Option<&Status>, fallback: Lang) -> Content {
    card_content_in(sc, status, None, ViewKind::Chat, fallback)
}

/// The row 0 status of a reply whose final result is in (green when copied, amber otherwise).
/// `ready` / `not_copied`: the strings for a copied and a not copied result.
fn ready_status(outcome: Option<ClipOutcome>, ready: String, not_copied: String, lang: Lang) -> StatusLine {
    let (text, tone) = match outcome {
        Some(ClipOutcome::Copied { cut: false }) => (ready, Tone::ReplyGreen),
        Some(ClipOutcome::Copied { cut: true }) => (i18n::tr(lang, "reply.ready_cut").to_string(), Tone::ReplyGreen),
        Some(ClipOutcome::Off) => (i18n::tr(lang, "reply.off").to_string(), Tone::ReplyAmber),
        Some(ClipOutcome::Changed) => (i18n::tr(lang, "reply.changed").to_string(), Tone::ReplyAmber),
        Some(ClipOutcome::Busy) => (i18n::tr(lang, "reply.busy").to_string(), Tone::ReplyAmber),
        Some(ClipOutcome::NotAllowed) | None => (not_copied, Tone::ReplyAmber),
    };
    StatusLine { text, tone, animated: false }
}

/// A reply card (design A.6.4): the status in the player's language (`lang`; the reply's target
/// is the other player's language) and the text that is (or would be) in the clipboard.
fn reply_content(status: Option<&Status>, book: Option<&ReplyBook>, rows: u8, target: &str, lang: Lang) -> Content {
    let line = |key: &str, tone: Tone, animated: bool| StatusLine { text: i18n::tr(lang, key).to_string(), tone, animated };
    let outcome = book.and_then(|b| b.outcome());
    // the text as it went to the clipboard (shortened to 255 bytes when it was cut)
    let shown = |t: &str| -> String {
        match outcome {
            Some(ClipOutcome::Copied { cut: true }) => one_line_255(t).0,
            _ => t.to_string(),
        }
    };
    let original = book.map(|b| b.original.as_str()).unwrap_or("");
    let language = i18n::language_name(lang, target);
    let (status, text) = match status {
        None | Some(Status::Translating) => (line("status.translating", Tone::ReplyGrey, true), None),
        Some(Status::Queued { ahead }) => (
            StatusLine { text: i18n::tr_fmt(lang, "status.queued", &[("n", &ahead.to_string())]), tone: Tone::ReplyGrey, animated: true },
            None,
        ),
        Some(Status::Loading { first_gpu_use: false }) => (line("status.loading", Tone::ReplyGrey, true), None),
        Some(Status::Loading { first_gpu_use: true }) => (line("status.loading_gpu_first", Tone::ReplyGrey, true), None),
        // shown while it streams, never copied
        Some(Status::Partial(t)) => (line("status.translating", Tone::ReplyGrey, true), Some(t.as_str().to_string())),
        Some(Status::Done(t)) => (
            ready_status(outcome, i18n::tr(lang, "reply.ready").to_string(), i18n::tr(lang, "reply.not_copied").to_string(), lang),
            Some(shown(t.as_str())),
        ),
        Some(Status::Same) => (
            ready_status(
                outcome,
                i18n::tr_fmt(lang, "reply.same", &[("language", language)]),
                i18n::tr_fmt(lang, "reply.same_not_copied", &[("language", language)]),
                lang,
            ),
            Some(shown(&one_line_255(original).0)),
        ),
        Some(Status::Nothing) => (
            ready_status(outcome, i18n::tr(lang, "reply.nothing").to_string(), i18n::tr(lang, "reply.nothing_not_copied").to_string(), lang),
            Some(shown(&one_line_255(original).0)),
        ),
        Some(Status::Failed(_)) => (line("reply.failed", Tone::ReplyRed, false), None),
        Some(Status::LowMemory) => (line("status.lowmem", Tone::ReplyRed, false), None),
        Some(Status::NotInstalled) => (line("status.notinstalled", Tone::ReplyRed, false), None),
    };
    let text = text.filter(|t| !t.is_empty());
    Content::Reply { status, text, target: target.to_string(), rows: usize::from(rows.saturating_sub(1)).max(1) }
}

/// Decides what a card shows in a view of kind `kind`. `reply` is the UI thread's state of a
/// reply request; `fallback` is the player's language (the latest chat request's target, else
/// the Windows UI language).
pub fn card_content_in(sc: &SceneCard, status: Option<&Status>, reply: Option<&ReplyBook>, kind: ViewKind, fallback: Lang) -> Content {
    let window = kind == ViewKind::Window;
    match &sc.kind {
        CardKind::Pending => Content::Empty,
        CardKind::Missing if window && sc.card.slot == REPLY_SLOT => {
            Content::Status { text: i18n::tr(fallback, "reply.missing").to_string(), tone: Tone::ReplyRed, animated: false }
        }
        CardKind::Missing => {
            let key = if window { "window.missing" } else { "status.missing" };
            Content::Status { text: i18n::tr(fallback, key).to_string(), tone: Tone::Status, animated: false }
        }
        CardKind::Request { reply: true, rows, target, .. } => reply_content(status, reply, *rows, target, fallback),
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
                Some(Status::Failed(_)) => st(if window { "window.failed" } else { "status.failed" }, Tone::Failure, false),
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
    /// Where the overlay windows sit in the z-order (decision D12).
    z: ZMode,
}

/// No reply state (chat-only callers and tests).
fn no_replies() -> &'static HashMap<JobKey, ReplyBook> {
    static EMPTY: std::sync::OnceLock<HashMap<JobKey, ReplyBook>> = std::sync::OnceLock::new();
    EMPTY.get_or_init(HashMap::new)
}

/// A card as it is drawn in a window view: each band's text starts at the tag's left edge + 2 px
/// and may use the tag's pixels as well as the visible plate pixels (the tag is painted over).
/// The second value is the extra width the text gains.
pub fn window_card(card: &Card) -> (Card, i32) {
    let mut c = card.clone();
    let mut gain = 0;
    for b in c.bands.iter_mut() {
        // tag_rect starts one column before the tag's x0 (its blended edge)
        let x = b.tag_rect.x + 1 + WINDOW_TEXT_INSET;
        let right = b.text_rect.x + b.text_rect.w;
        if b.tag_rect.w <= 0 || x >= b.text_rect.x || right <= x {
            continue;
        }
        gain = gain.max(b.text_rect.x - x);
        b.text_rect = Rect { x, y: b.text_rect.y, w: right - x, h: b.text_rect.h };
        // the drawable area: the tag's rectangle and the plate mask
        if let Some(m) = &b.mask {
            let u = crate::capture::union(m.rect, b.tag_rect);
            let mut bits = vec![0u8; (u.w.max(0) * u.h.max(0)) as usize];
            for y in 0..u.h {
                for x in 0..u.w {
                    let (cx, cy) = (u.x + x, u.y + y);
                    let in_tag = cx >= b.tag_rect.x && cx < b.tag_rect.x + b.tag_rect.w && cy >= b.tag_rect.y && cy < b.tag_rect.y + b.tag_rect.h;
                    if in_tag || m.get(cx, cy) {
                        bits[(y * u.w + x) as usize] = 255;
                    }
                }
            }
            b.mask = Some(Mask { rect: u, bits });
        }
    }
    c.text_width_px += gain;
    (c, gain)
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
            z: ZMode::Topmost,
        }
    }

    /// The z-order mode in use.
    pub fn z_mode(&self) -> ZMode {
        self.z
    }

    /// Puts every overlay window topmost (the game window is in front) or right above the game
    /// window `wow` (it is not), now and for the windows shown later (decision D12). Applied again
    /// when the window right above the game is not ours any more. Returns true when a window was
    /// moved.
    pub fn set_z(&mut self, z: ZMode) -> bool {
        self.z = z;
        let mut moved = false;
        for s in self.surfaces.values_mut() {
            if let Surface::Window(w) = s {
                moved |= w.apply_z(z);
            }
        }
        moved
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

    fn tone_rgb(&self, tone: Tone) -> [u8; 3] {
        match tone {
            Tone::Text => self.text_rgb,
            Tone::Status => STATUS_RGB,
            Tone::Failure => FAILURE_RGB,
            Tone::ReplyGrey => REPLY_GREY,
            Tone::ReplyGreen => REPLY_GREEN,
            Tone::ReplyAmber => REPLY_AMBER,
            Tone::ReplyRed => REPLY_RED,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint(
        &mut self,
        canvas: &mut Canvas,
        view: &SceneView,
        sc: &SceneCard,
        results: &HashMap<JobKey, Status>,
        replies: &HashMap<JobKey, ReplyBook>,
        fallback: Lang,
        pulse_mul: f32,
    ) -> PaintStats {
        // a window view draws its text from the tag on (the tag is painted over)
        let window_copy = (view.kind == ViewKind::Window).then(|| window_card(&sc.card).0);
        let card = window_copy.as_ref().unwrap_or(&sc.card);
        let (status, reply) = match &sc.kind {
            CardKind::Request { key, .. } => (results.get(key), replies.get(key)),
            _ => (None, None),
        };
        let content = card_content_in(sc, status, reply, view.kind, fallback);
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
                    let rgb = self.tone_rgb(tone);
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
                Content::Reply { status, text, target, rows } => {
                    // row 0: the status, in the player's language
                    if let Some(bi) = card.bands.iter().position(|b| b.row == 0) {
                        if status.animated {
                            self.animating = true;
                        }
                        let fitted = self.fitted(Face::Latin, size, &status.text, 1, width);
                        if let Some(line) = fitted.lines.into_iter().next() {
                            lines.push((bi, line, Face::Latin, fitted.size, self.tone_rgb(status.tone), if status.animated { pulse_mul } else { 1.0 }));
                        }
                    }
                    // rows 1..K-1: the text, in the reply's script
                    if let Some(text) = text {
                        let face = face_for_target(&target);
                        let fsize = self.font_size(view, card, face);
                        let fitted = self.fitted(face, fsize, &text, rows, width);
                        for (i, line) in fitted.lines.into_iter().enumerate() {
                            if let Some(bi) = card.bands.iter().position(|b| usize::from(b.row) == i + 1) {
                                lines.push((bi, line, face, fitted.size, self.text_rgb, 1.0));
                            }
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
        let mut st = paint_card(canvas, card, &sc.bg, &texts, self.diag_outlines);
        // the reply card's hint lane: its passing columns get the bubble fill, flat
        if let Some(lane) = &sc.lane {
            let mut x = 0usize;
            while x < lane.cols.len() {
                if !lane.cols[x] {
                    x += 1;
                    continue;
                }
                let start = x;
                while x < lane.cols.len() && lane.cols[x] {
                    x += 1;
                }
                let r = Rect { x: lane.rect.x + start as i32, y: lane.rect.y, w: (x - start) as i32, h: lane.rect.h };
                st.lane_px += canvas.fill(r, lane.rgb, 1.0);
            }
        }
        st
    }

    /// Draws the scene (chat requests only, no reply state). Returns the views whose state
    /// changed (for `--diag-json`).
    pub fn render(&mut self, scene: &Scene, results: &HashMap<JobKey, Status>, pulse_mul: f32, ui_lang: Lang) -> Vec<ViewReport> {
        self.render_with(scene, results, no_replies(), pulse_mul, ui_lang)
    }

    /// Draws the scene with the UI thread's reply state. Returns the views whose state changed
    /// (for `--diag-json`).
    pub fn render_with(&mut self, scene: &Scene, results: &HashMap<JobKey, Status>, replies: &HashMap<JobKey, ReplyBook>, pulse_mul: f32, ui_lang: Lang) -> Vec<ViewReport> {
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
                    let s = self.paint(&mut canvas, view, sc, results, replies, fallback, pulse_mul);
                    stats.tag_px += s.tag_px;
                    stats.fill_px += s.fill_px;
                    stats.text_px += s.text_px;
                    stats.lane_px += s.lane_px;
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
        let z = self.z;
        match self.surfaces.get_mut(&id) {
            Some(Surface::Window(w)) => w.present_z(x, y, canvas, z),
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
            views: vec![SceneView { id: 0, kind: ViewKind::Chat, rect: Rect { x: 0, y: 16, w: 400, h: 1000 }, font_px: 14, cards }],
            hidden: None,
            lang_target: None,
            gdi: false,
            foreground: true,
        }
    }

    fn req(slot: u8, req_id: u16, target: &str) -> SceneCard {
        SceneCard {
            card: card(slot, 0, 900, 2, 1.0),
            kind: CardKind::Request { key: JobKey { session: 9, req_id }, rows: 2, target: target.into(), reply: false },
            covered: false,
            block_px: 5.0,
            bg: crate::backdrop::CardBg::default(),
            lane: None,
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
            sc.kind = CardKind::Request { key: JobKey { session: 9, req_id: 10 + i as u16 }, rows, target: "fr".into(), reply: false };
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
        sc.kind = CardKind::Request { key: JobKey { session: 9, req_id: 1 }, rows: text_rows, target: "fr".into(), reply: false };
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

    use crate::clip::FakeClip;
    use crate::reply::Replies;

    /// A 3-row reply card (row 0 status, rows 1-2 text) in a window view.
    fn reply_card() -> SceneCard {
        let mut sc = req(126, 40, "de");
        sc.card = card(126, 200, 900, 3, 1.0);
        sc.kind = CardKind::Request { key: JobKey { session: 9, req_id: 40 }, rows: 3, target: "de".into(), reply: true };
        sc
    }

    fn window_scene(cards: Vec<SceneCard>) -> Scene {
        let mut s = scene_with(cards);
        s.views[0].id = 200;
        s.views[0].kind = ViewKind::Window;
        s
    }

    fn status_of(c: &Content) -> (String, Tone) {
        match c {
            Content::Reply { status, .. } => (status.text.clone(), status.tone),
            Content::Status { text, tone, .. } => (text.clone(), *tone),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn reply_row_0_says_when_ctrl_v_can_be_used() {
        let sc = reply_card();
        let key = JobKey { session: 9, req_id: 40 };
        let done = Status::Done(ChatText::new("Brechen wir in 5 Minuten auf?".into()));
        let st = |c: Content| status_of(&c);
        // waiting: grey, before any copy
        let mut os = FakeClip::new();
        let mut r = Replies::new();
        r.start(&mut os, key, true, true, ChatText::new("on part dans 5 min ?".into()));
        let book = r.get(&key);
        assert_eq!(st(card_content_in(&sc, None, book, ViewKind::Window, Lang::En)), ("Translating...".into(), Tone::ReplyGrey));
        assert_eq!(st(card_content_in(&sc, Some(&Status::Queued { ahead: 1 }), book, ViewKind::Window, Lang::En)).1, Tone::ReplyGrey);
        assert_eq!(st(card_content_in(&sc, Some(&Status::Loading { first_gpu_use: false }), book, ViewKind::Window, Lang::En)).0, "Starting the translator...");
        match card_content_in(&sc, Some(&Status::Partial(ChatText::new("Brechen wir".into()))), book, ViewKind::Window, Lang::En) {
            Content::Reply { status, text, rows, .. } => {
                assert_eq!((status.tone, text.as_deref(), rows), (Tone::ReplyGrey, Some("Brechen wir"), 2), "partial shown, not ready");
            }
            other => panic!("{other:?}"),
        }
        // green only once the clipboard holds it
        r.on_status(&mut os, &key, &done);
        assert_eq!(st(card_content_in(&sc, Some(&done), r.get(&key), ViewKind::Window, Lang::En)), ("Ready - Ctrl+V to paste".into(), Tone::ReplyGreen));
        assert_eq!(st(card_content_in(&sc, Some(&done), r.get(&key), ViewKind::Window, Lang::Fr)).0, "Prêt - Ctrl+V pour coller");
        // amber with the reason
        let cases: [(bool, bool, Option<&str>, &str); 4] = [
            (false, true, None, "Ready (not copied)"),
            (true, false, None, "Ready (not copied - off in the Helper menu)"),
            (true, true, Some("copied meanwhile"), "Not copied - you copied something else. Click Translate again"),
            (true, true, None, "Not copied - clipboard busy. Click Translate again"),
        ];
        for (i, (copy, switch, user_copy, want)) in cases.into_iter().enumerate() {
            let mut os = FakeClip::new();
            let mut r = Replies::new();
            r.start(&mut os, key, copy, switch, ChatText::new("x".into()));
            if let Some(t) = user_copy {
                os.user_copy(t);
            }
            if i == 3 {
                os.busy_opens = 3;
            }
            r.on_status(&mut os, &key, &done);
            assert_eq!(st(card_content_in(&sc, Some(&done), r.get(&key), ViewKind::Window, Lang::En)), (want.into(), Tone::ReplyAmber));
        }
        // shortened to 255 bytes: still green, says so, and shows what is in the clipboard
        let long = Status::Done(ChatText::new("Wort ".repeat(80)));
        let mut os = FakeClip::new();
        let mut r = Replies::new();
        r.start(&mut os, key, true, true, ChatText::new("x".into()));
        r.on_status(&mut os, &key, &long);
        match card_content_in(&sc, Some(&long), r.get(&key), ViewKind::Window, Lang::En) {
            Content::Reply { status, text, .. } => {
                assert_eq!((status.text.as_str(), status.tone), ("Ready - Ctrl+V to paste (shortened to 255 bytes)", Tone::ReplyGreen));
                assert_eq!(text.as_deref(), os.text.as_deref());
            }
            other => panic!("{other:?}"),
        }
        // already in the target language: the player's own text was copied
        let mut os = FakeClip::new();
        let mut r = Replies::new();
        r.start(&mut os, key, true, true, ChatText::new("Ja, ich komme".into()));
        r.on_status(&mut os, &key, &Status::Same);
        match card_content_in(&sc, Some(&Status::Same), r.get(&key), ViewKind::Window, Lang::En) {
            Content::Reply { status, text, .. } => {
                assert_eq!((status.text.as_str(), status.tone, text.as_deref()), ("Already in German - Ctrl+V to paste", Tone::ReplyGreen, Some("Ja, ich komme")));
            }
            other => panic!("{other:?}"),
        }
        // red
        assert_eq!(
            st(card_content_in(&sc, Some(&Status::Failed(tp_engine::FailKind::Timeout)), r.get(&key), ViewKind::Window, Lang::En)),
            ("Translation failed - click Translate again".into(), Tone::ReplyRed)
        );
    }

    #[test]
    fn window_wording_for_missing_and_failed() {
        let mut missing = reply_card();
        missing.kind = CardKind::Missing;
        assert_eq!(status_of(&card_content_in(&missing, None, None, ViewKind::Window, Lang::En)), ("Not received - click Translate again".into(), Tone::ReplyRed));
        let mut bubble = req(101, 0, "fr");
        bubble.kind = CardKind::Missing;
        assert_eq!(status_of(&card_content_in(&bubble, None, None, ViewKind::Window, Lang::En)).0, "Not received - click the bubble");
        assert_eq!(status_of(&card_content_in(&bubble, None, None, ViewKind::Chat, Lang::En)).0, "Not received - click [T] again");
        let sc = req(101, 1, "en");
        let failed = Status::Failed(tp_engine::FailKind::Http);
        assert_eq!(status_of(&card_content_in(&sc, Some(&failed), None, ViewKind::Window, Lang::En)).0, "Translation failed - Shift-click the bubble");
        assert_eq!(status_of(&card_content_in(&sc, Some(&failed), None, ViewKind::Chat, Lang::En)).0, "Translation failed - Shift-click [T] to retry");
    }

    #[test]
    fn reply_card_paints_the_status_on_row_0_and_the_text_below() {
        let key = JobKey { session: 9, req_id: 40 };
        let mut os = FakeClip::new();
        let mut r = Replies::new();
        r.start(&mut os, key, true, true, ChatText::new("x".into()));
        let done = Status::Done(ChatText::new("Brechen wir auf".into()));
        r.on_status(&mut os, &key, &done);
        let mut results = HashMap::new();
        results.insert(key, done);
        let mut m = OverlayManager::new(true, false, [0xA8, 0xD8, 0xFF], 1.0);
        m.render_with(&window_scene(vec![reply_card()]), &results, r.books(), 1.0, Lang::En);
        let bits = m.memory_pixels(200).unwrap();
        let px = |x: i32, y: i32| {
            let i = (((y - 16) * 400 + x) * 4) as usize;
            [i32::from(bits[i + 2]), i32::from(bits[i + 1]), i32::from(bits[i])]
        };
        // premultiplied text over a transparent canvas: green (status) or light blue (text)
        let green = |p: [i32; 3]| p[1] > 120 && p[1] > p[0] + 60 && p[1] > p[2] + 60;
        let blue = |p: [i32; 3]| p[2] > 120 && p[2] >= p[1] && p[2] > p[0] + 30;
        // rows at y 900, 916, 932 (14 px tall)
        let count = |y0: i32, f: &dyn Fn([i32; 3]) -> bool| (y0..y0 + 14).flat_map(|y| (0..400).map(move |x| (x, y))).filter(|&(x, y)| f(px(x, y))).count();
        assert!(count(900, &green) > 10, "green status on row 0");
        assert_eq!(count(900, &blue), 0, "no translation on row 0");
        assert!(count(916, &blue) > 10, "the translation from row 1");
        assert_eq!(count(916, &green) + count(932, &green), 0);
    }

    #[test]
    fn window_views_start_the_text_at_the_tag() {
        // the scene card's tag is x 10..56 (tag_rect 10, 47 wide), its text rect from x 60
        let (w, gain) = window_card(&card(3, 200, 900, 1, 1.0));
        assert_eq!(w.bands[0].text_rect.x, 10 + 1 + WINDOW_TEXT_INSET);
        assert_eq!(gain, 60 - 13);
        assert_eq!(w.bands[0].text_rect.x + w.bands[0].text_rect.w, 260, "same right end");
        assert_eq!(w.text_width_px, 200 + gain);
        let m = w.bands[0].mask.as_ref().unwrap();
        assert!(m.get(20, 905) && m.get(100, 905), "the tag and the plate are drawable");
        let mut results = HashMap::new();
        results.insert(JobKey { session: 9, req_id: 1 }, Status::Done(ChatText::new("WWWWWWWWWW".into())));
        let mut sc = req(3, 1, "fr");
        sc.card = card(3, 200, 900, 1, 1.0);
        let mut m = OverlayManager::new(true, false, [0xA8, 0xD8, 0xFF], 1.0);
        m.render(&window_scene(vec![sc.clone()]), &results, 1.0, Lang::En);
        // a text pixel over the tag cover (plate colour 32, 40, 64): blue and red raised
        let is_text = |b: &[u8], x: i32, y: i32| {
            let i = (((y - 16) * 400 + x) * 4) as usize;
            b[i] > 150 && b[i + 2] > 100
        };
        let bits = m.memory_pixels(200).unwrap();
        assert!((900..914).any(|y| (13..56).any(|x| is_text(&bits, x, y))), "text over the painted tag");
        // the same card in a chat view keeps its text right of the tag
        let mut c = OverlayManager::new(true, false, [0xA8, 0xD8, 0xFF], 1.0);
        c.render(&scene_with(vec![sc]), &results, 1.0, Lang::En);
        let cb = c.memory_pixels(0).unwrap();
        assert!(!(900..914).any(|y| (13..56).any(|x| is_text(&cb, x, y))));
        assert!((900..914).any(|y| (60..200).any(|x| is_text(&cb, x, y))));
    }

    #[test]
    fn the_lane_is_painted_on_passing_columns_only() {
        let mut sc = reply_card();
        let mut cols = vec![true; 250];
        for c in cols.iter_mut().skip(200) {
            *c = false;
        }
        sc.lane = Some(crate::lane::Lane { rect: Rect { x: 10, y: 948, w: 250, h: 16 }, rgb: [36, 30, 24], cols });
        let mut m = OverlayManager::new(true, false, [0xA8, 0xD8, 0xFF], 1.0);
        let reports = m.render(&window_scene(vec![sc]), &HashMap::new(), 1.0, Lang::En);
        assert_eq!(reports[0].stats.lane_px, 200 * 16);
        let bits = m.memory_pixels(200).unwrap();
        let px = |x: i32, y: i32| {
            let i = (((y - 16) * 400 + x) * 4) as usize;
            ([bits[i + 2], bits[i + 1], bits[i]], bits[i + 3])
        };
        assert_eq!(px(15, 950), ([36, 30, 24], 255));
        assert_eq!(px(209, 963), ([36, 30, 24], 255));
        assert_eq!(px(210, 955).1, 0, "a failing column (under a tooltip) is left alone");
    }
}
