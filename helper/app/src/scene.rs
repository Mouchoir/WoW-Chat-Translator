//! Scene state machine (design D.3), pure and testable: bridge messages and found cards in, the
//! cards to draw out. The capture thread feeds it; the UI thread draws the published [`Scene`].
//!
//! - TRANSLATE/CANCEL/CLEAR/TEST/VIEW update the slot map and produce [`Action`]s for the engine
//!   and the UI.
//! - A card whose slot is unknown in this session shows "Not received" once its plate has been
//!   seen for 1.5 s.
//! - With the game in front and no new frame for 500 ms, every card is hidden; nothing is shown
//!   while the game is minimized or gone; covered cards are kept but flagged.
//! - Each known slot's row count (TRANSLATE rows, 1..6) goes to `cards::build_with_rows`
//!   ([`SceneMachine::rows_expected`]): a tag carries its row modulo 4, and the cards are
//!   numbered with it.

use std::collections::{HashMap, VecDeque};

use tp_bridge::cards::{Card, TAG_ROWS};
use tp_bridge::image::Rect;
use tp_bridge::msg::{Message, View};
use tp_bridge::strip::Mode;
use tp_bridge::tags::{TagHit, TEST_SLOT};
use tp_engine::{Job, JobKey};

use crate::backdrop::CardBg;

pub const MISSING_AFTER_MS: u64 = 1500;
pub const STALE_AFTER_MS: u64 = 500;
/// Cards fainter than this (chat faded out) are hidden.
pub const MIN_ALPHA: f32 = 0.3;
/// The overlay never covers client rows 0..15 (the strip lives there).
pub const OVERLAY_TOP: i32 = 16;
/// `STATUS.addonBuild` this helper speaks (2: TRANSLATE rows 1..6, 2026-10-18).
pub const ADDON_BUILD: u16 = 2;
/// Weak bridge signal: average calibration gap under this for 10 s.
pub const WEAK_GAP: f32 = 16.0;
pub const WEAK_FOR_MS: u64 = 10_000;

#[derive(Clone, Debug, PartialEq)]
pub struct SlotReq {
    pub key: JobKey,
    pub rows: u8,
    pub plate: bool,
    pub target: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ToastKind {
    BridgeOk { mode: Mode, margin: u32 },
    AddonOld,
    HelperOld,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BalloonKind {
    WeakSignal,
    SwitchedCpu,
    ServerFailed,
    Fullscreen,
    NotInstalled,
}

impl BalloonKind {
    pub fn name(self) -> &'static str {
        match self {
            BalloonKind::WeakSignal => "weak_signal",
            BalloonKind::SwitchedCpu => "switched_cpu",
            BalloonKind::ServerFailed => "server_failed",
            BalloonKind::Fullscreen => "fullscreen",
            BalloonKind::NotInstalled => "notinstalled",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            BalloonKind::WeakSignal => "balloon.weak_signal",
            BalloonKind::SwitchedCpu => "balloon.switched_cpu",
            BalloonKind::ServerFailed => "balloon.server_failed",
            BalloonKind::Fullscreen => "balloon.fullscreen",
            BalloonKind::NotInstalled => "balloon.notinstalled",
        }
    }
}

/// Side effects requested by the state machine.
pub enum Action {
    Submit(Job),
    Cancel(JobKey),
    ClearSession(u16),
    Toast(ToastKind),
    Balloon(BalloonKind),
}

#[derive(Clone, Debug, PartialEq)]
pub enum CardKind {
    Request { key: JobKey, rows: u8, target: String },
    /// Slot 127 (`/tpr test rows`): "Overlay OK (row N, block P px)".
    Test,
    /// Unknown slot whose plate has been seen for 1.5 s: "Not received - click [T] again".
    Missing,
    /// Unknown slot seen for less than 1.5 s: only the tags are painted over.
    Pending,
}

impl CardKind {
    pub fn name(&self) -> &'static str {
        match self {
            CardKind::Request { .. } => "request",
            CardKind::Test => "test",
            CardKind::Missing => "missing",
            CardKind::Pending => "pending",
        }
    }
}

#[derive(Clone, Debug)]
pub struct SceneCard {
    pub card: Card,
    pub kind: CardKind,
    pub covered: bool,
    /// Tag block size of the card's first row (for the slot-127 test text).
    pub block_px: f32,
    /// The chat background around each band, sampled in the same capture (native look); empty
    /// when unknown (the overlay then covers the tags with the plate colour).
    pub bg: CardBg,
}

#[derive(Clone, Debug)]
pub struct SceneView {
    pub id: u8,
    /// Overlay area in client coordinates (never above row 16).
    pub rect: Rect,
    /// Chat font size from VIEW (0 when no VIEW is known).
    pub font_px: u8,
    pub cards: Vec<SceneCard>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hidden {
    NoWow,
    Minimized,
    Stale,
}

impl Hidden {
    pub fn name(self) -> &'static str {
        match self {
            Hidden::NoWow => "no_wow",
            Hidden::Minimized => "minimized",
            Hidden::Stale => "stale",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WowGeom {
    pub hwnd: isize,
    pub origin_x: i32,
    pub origin_y: i32,
    pub client_w: i32,
    pub client_h: i32,
}

#[derive(Clone, Debug)]
pub struct Scene {
    pub wow: Option<WowGeom>,
    pub views: Vec<SceneView>,
    pub hidden: Option<Hidden>,
    /// Target of the latest request this session (language of the Missing and Test strings).
    pub lang_target: Option<String>,
    /// Capture runs through GDI (the overlay must then be excluded from capture).
    pub gdi: bool,
}

impl Scene {
    pub fn empty(hidden: Option<Hidden>) -> Scene {
        Scene { wow: None, views: Vec::new(), hidden, lang_target: None, gdi: false }
    }

    pub fn card_count(&self) -> usize {
        self.views.iter().map(|v| v.cards.len()).sum()
    }

    /// True when at least one uncovered card would be drawn.
    pub fn any_visible(&self) -> bool {
        self.hidden.is_none() && self.views.iter().any(|v| v.cards.iter().any(|c| !c.covered))
    }

    /// FNV-1a over everything the overlay draws from, for "publish when changed".
    pub fn fingerprint(&self) -> u64 {
        let mut h = Fnv::new();
        h.i(self.hidden.map(|x| x as i64).unwrap_or(-1));
        h.i(i64::from(self.gdi));
        if let Some(w) = self.wow {
            h.i(w.hwnd as i64);
            h.i(w.origin_x.into());
            h.i(w.origin_y.into());
            h.i(w.client_w.into());
            h.i(w.client_h.into());
        }
        h.bytes(self.lang_target.as_deref().unwrap_or("").as_bytes());
        for v in &self.views {
            h.i(v.id.into());
            h.rect(&v.rect);
            h.i(v.font_px.into());
            for sc in &v.cards {
                h.i(i64::from(sc.covered));
                match &sc.kind {
                    CardKind::Request { key, rows, .. } => {
                        h.i(1);
                        h.i(key.session.into());
                        h.i(key.req_id.into());
                        h.i((*rows).into());
                    }
                    k => h.i(match k {
                        CardKind::Test => 2,
                        CardKind::Missing => 3,
                        _ => 4,
                    }),
                }
                h.i((sc.block_px * 4.0) as i64);
                for w in sc.bg.hash_words() {
                    h.i(w);
                }
                let c = &sc.card;
                h.i(c.slot.into());
                h.i(c.view_id.into());
                h.i((c.alpha * 64.0).round() as i64);
                h.i(i64::from(c.plate));
                h.bytes(&c.plate_rgb);
                h.i(c.text_width_px.into());
                for b in &c.bands {
                    h.i(b.row.into());
                    h.rect(&b.text_rect);
                    h.rect(&b.tag_rect);
                    if let Some(m) = &b.mask {
                        h.rect(&m.rect);
                        h.bytes(&m.bits);
                    }
                }
                for g in &c.gaps {
                    h.rect(g);
                }
            }
        }
        h.0
    }
}

struct Fnv(u64);

impl Fnv {
    fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    /// FNV-1a over 8-byte words (then the tail bytes): masks are hashed every tick, so this is
    /// 8 times fewer multiplications than byte by byte.
    fn bytes(&mut self, b: &[u8]) {
        let words = b.chunks_exact(8);
        let tail = words.remainder();
        for w in words {
            self.0 ^= u64::from_le_bytes([w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7]]);
            self.0 = self.0.wrapping_mul(0x100_0000_01b3);
        }
        for &x in tail {
            self.0 ^= u64::from(x);
            self.0 = self.0.wrapping_mul(0x100_0000_01b3);
        }
        self.0 ^= b.len() as u64;
        self.0 = self.0.wrapping_mul(0x100_0000_01b3);
    }
    fn i(&mut self, v: i64) {
        self.bytes(&v.to_le_bytes());
    }
    fn rect(&mut self, r: &Rect) {
        self.i(r.x.into());
        self.i(r.y.into());
        self.i(r.w.into());
        self.i(r.h.into());
    }
}

/// The game window as seen in one capture tick.
#[derive(Clone, Copy, Debug)]
pub struct WowTick {
    pub geom: WowGeom,
    pub minimized: bool,
    /// Foreground (or assumed foreground in tests).
    pub foreground: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct TickInput {
    pub now_ms: u64,
    pub wow: Option<WowTick>,
    /// A new frame was captured this tick.
    pub fresh: bool,
}

/// What a decoded strip frame says about the bridge (no payload).
#[derive(Clone, Copy, Debug)]
pub struct FrameInfo {
    pub session: u16,
    pub mode: Mode,
    pub addon_build: u16,
    pub min_gap: f32,
    pub white: [f32; 3],
    pub black: [f32; 3],
}

pub struct SceneMachine {
    session: Option<u16>,
    slots: HashMap<u8, SlotReq>,
    views: Vec<View>,
    last_target: Option<String>,
    unknown_seen: HashMap<(u8, u8), u64>,
    last_fresh_ms: Option<u64>,
    last_mode: Mode,
    last_margin: u32,
    white_black: Option<([f32; 3], [f32; 3])>,
    build_warned: Option<u16>,
    gaps: VecDeque<f32>,
    weak_since: Option<u64>,
    weak_warned: bool,
}

impl Default for SceneMachine {
    fn default() -> Self {
        SceneMachine::new()
    }
}

impl SceneMachine {
    pub fn new() -> SceneMachine {
        SceneMachine {
            session: None,
            slots: HashMap::new(),
            views: Vec::new(),
            last_target: None,
            unknown_seen: HashMap::new(),
            last_fresh_ms: None,
            last_mode: Mode::Dense,
            last_margin: 0,
            white_black: None,
            build_warned: None,
            gaps: VecDeque::new(),
            weak_since: None,
            weak_warned: false,
        }
    }

    pub fn session(&self) -> Option<u16> {
        self.session
    }

    /// Current session's bit, for `cards::build` (None before the first frame).
    pub fn sess_bit(&self) -> Option<u8> {
        self.session.map(|s| (s % 2) as u8)
    }

    /// From the TRANSLATE flags: Some(plate) for a known slot, None otherwise.
    pub fn plate_expected(&self, slot: u8) -> Option<bool> {
        self.slots.get(&slot).map(|s| s.plate)
    }

    /// From the TRANSLATE: Some(rows) (1..6) for a known slot, None otherwise.
    pub fn rows_expected(&self, slot: u8) -> Option<u8> {
        self.slots.get(&slot).map(|s| s.rows)
    }

    pub fn slot(&self, slot: u8) -> Option<&SlotReq> {
        self.slots.get(&slot)
    }

    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    /// The sticky VIEW of this session.
    pub fn views(&self) -> &[View] {
        &self.views
    }

    /// No VIEW known: the capture thread runs the discovery scan.
    pub fn discovery(&self) -> bool {
        self.views.is_empty()
    }

    /// Latest strip calibration as (white, black), for the tag alpha.
    pub fn white_black(&self) -> Option<([f32; 3], [f32; 3])> {
        self.white_black
    }

    pub fn last_mode(&self) -> Mode {
        self.last_mode
    }

    pub fn last_margin(&self) -> u32 {
        self.last_margin
    }

    /// A strip frame was decoded (before its messages are handled).
    pub fn on_frame(&mut self, f: &FrameInfo, now_ms: u64) -> Vec<Action> {
        let mut out = Vec::new();
        if self.session.is_none() {
            self.session = Some(f.session);
        }
        self.last_mode = f.mode;
        self.last_margin = f.min_gap.max(0.0).round() as u32;
        self.white_black = Some((f.white, f.black));
        if f.addon_build != ADDON_BUILD && self.build_warned != Some(f.session) {
            self.build_warned = Some(f.session);
            out.push(Action::Toast(if f.addon_build < ADDON_BUILD { ToastKind::AddonOld } else { ToastKind::HelperOld }));
        }
        self.gaps.push_back(f.min_gap);
        while self.gaps.len() > 16 {
            self.gaps.pop_front();
        }
        let avg = self.gaps.iter().sum::<f32>() / self.gaps.len() as f32;
        if avg < WEAK_GAP {
            let since = *self.weak_since.get_or_insert(now_ms);
            if !self.weak_warned && now_ms.saturating_sub(since) >= WEAK_FOR_MS {
                self.weak_warned = true;
                out.push(Action::Balloon(BalloonKind::WeakSignal));
            }
        } else {
            self.weak_since = None;
        }
        out
    }

    /// The addon session changed (from `Reassembler::take_session_change`): forget its requests
    /// and VIEW, and cancel its jobs.
    pub fn on_session_change(&mut self, old: u16, new: u16) -> Vec<Action> {
        self.session = Some(new);
        self.slots.clear();
        self.views.clear();
        self.unknown_seen.clear();
        self.last_target = None;
        vec![Action::ClearSession(old)]
    }

    pub fn on_message(&mut self, session: u16, m: Message) -> Vec<Action> {
        self.session = Some(session);
        let mut out = Vec::new();
        match m {
            Message::Translate(t) => {
                let key = JobKey { session, req_id: t.req_id };
                if let Some(old) = self.slots.get(&t.slot) {
                    if old.key != key {
                        out.push(Action::Cancel(old.key));
                    }
                }
                self.slots.insert(t.slot, SlotReq { key, rows: t.rows, plate: t.plate, target: t.target.clone() });
                self.unknown_seen.retain(|(_, s), _| *s != t.slot);
                self.last_target = Some(t.target.clone());
                out.push(Action::Submit(Job { key, target: t.target, text: t.text, pairs: t.pairs, no_cache: t.no_cache }));
            }
            Message::Cancel { req_id, slot } => {
                if self.slots.get(&slot).map(|s| s.key.req_id) == Some(req_id) {
                    self.slots.remove(&slot);
                }
                out.push(Action::Cancel(JobKey { session, req_id }));
            }
            Message::Clear => {
                self.slots.clear();
                out.push(Action::ClearSession(session));
            }
            Message::Test { .. } => {
                out.push(Action::Toast(ToastKind::BridgeOk { mode: self.last_mode, margin: self.last_margin }));
            }
            Message::View(v) => {
                // An empty VIEW (every chat rectangle secret) keeps the previous one.
                if !v.is_empty() {
                    self.views = v;
                }
            }
        }
        out
    }

    /// Overlay areas: one per VIEW, or id 0 = the whole client below row 16 when none is known.
    pub fn view_areas(&self, client_w: i32, client_h: i32) -> Vec<(u8, Rect, u8)> {
        let cut = |r: Rect| -> Rect {
            let y0 = r.y.max(OVERLAY_TOP);
            let y1 = (r.y + r.h).min(client_h);
            let x0 = r.x.max(0);
            let x1 = (r.x + r.w).min(client_w);
            Rect { x: x0, y: y0, w: (x1 - x0).max(0), h: (y1 - y0).max(0) }
        };
        if self.views.is_empty() {
            return vec![(0, cut(Rect { x: 0, y: 0, w: client_w, h: client_h }), 0)];
        }
        self.views
            .iter()
            .map(|v| (v.id, cut(Rect { x: v.x.into(), y: v.y.into(), w: v.w.into(), h: v.h.into() }), v.font_px))
            .collect()
    }

    /// Builds the scene for one tick from the cards found in this capture (without background
    /// samples).
    pub fn update(&mut self, input: &TickInput, cards: Vec<Card>, hits: &[TagHit], covered: &mut dyn FnMut(&Card) -> bool) -> Scene {
        self.update_with_bg(input, cards.into_iter().map(|c| (c, CardBg::default())).collect(), hits, covered)
    }

    /// Builds the scene for one tick from the cards found in this capture, each with the chat
    /// background sampled around its rows.
    pub fn update_with_bg(&mut self, input: &TickInput, cards: Vec<(Card, CardBg)>, hits: &[TagHit], covered: &mut dyn FnMut(&Card) -> bool) -> Scene {
        if input.fresh {
            self.last_fresh_ms = Some(input.now_ms);
        }
        let lang_target = self.last_target.clone();
        let Some(w) = input.wow else {
            self.unknown_seen.clear();
            return Scene { lang_target, ..Scene::empty(Some(Hidden::NoWow)) };
        };
        if w.minimized {
            self.unknown_seen.clear();
            return Scene { wow: Some(w.geom), lang_target, ..Scene::empty(Some(Hidden::Minimized)) };
        }
        let stale = match self.last_fresh_ms {
            Some(t) => input.now_ms.saturating_sub(t) > STALE_AFTER_MS,
            None => true,
        };
        if w.foreground && stale {
            return Scene { wow: Some(w.geom), lang_target, ..Scene::empty(Some(Hidden::Stale)) };
        }
        let mut views: Vec<SceneView> = self
            .view_areas(w.geom.client_w, w.geom.client_h)
            .into_iter()
            .map(|(id, rect, font_px)| SceneView { id, rect, font_px, cards: Vec::new() })
            .collect();
        let mut seen_unknown: Vec<(u8, u8)> = Vec::new();
        for (card, bg) in cards {
            if card.alpha < MIN_ALPHA || card.bands.is_empty() {
                continue;
            }
            let Some(view) = views.iter_mut().find(|v| v.id == card.view_id) else { continue };
            let kind = if card.slot == TEST_SLOT {
                CardKind::Test
            } else if let Some(s) = self.slots.get(&card.slot) {
                CardKind::Request { key: s.key, rows: s.rows, target: s.target.clone() }
            } else {
                let k = (card.view_id, card.slot);
                seen_unknown.push(k);
                let since = *self.unknown_seen.entry(k).or_insert(input.now_ms);
                if card.plate && input.now_ms.saturating_sub(since) >= MISSING_AFTER_MS {
                    CardKind::Missing
                } else {
                    CardKind::Pending
                }
            };
            let first = &card.bands[0];
            let block_px = hits
                .iter()
                .find(|h| h.slot == card.slot && h.row % TAG_ROWS == first.row % TAG_ROWS && h.y_top <= first.tag_rect.y + first.tag_rect.h && h.y_bot >= first.tag_rect.y)
                .map(|h| h.block_px)
                .unwrap_or(0.0);
            let is_covered = covered(&card);
            view.cards.push(SceneCard { card, kind, covered: is_covered, block_px, bg });
        }
        self.unknown_seen.retain(|k, _| seen_unknown.contains(k));
        Scene { wow: Some(w.geom), views, hidden: None, lang_target, gdi: false }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use tp_bridge::cards::{Mask, RowBand};
    use tp_bridge::msg::{ChatText, Translate};

    pub fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn card(slot: u8, view_id: u8, y: i32, rows: u8, alpha: f32) -> Card {
        let bands = (0..rows)
            .map(|r| {
                let by = y + i32::from(r) * 16;
                RowBand {
                    row: r,
                    text_rect: rect(60, by, 200, 14),
                    mask: Some(Mask { rect: rect(57, by, 210, 14), bits: vec![255; 210 * 14] }),
                    tag_rect: rect(10, by, 47, 14),
                }
            })
            .collect();
        Card { slot, view_id, bands, gaps: Vec::new(), alpha, plate: true, plate_rgb: [32, 40, 64], text_width_px: 200 }
    }

    fn geom() -> WowGeom {
        WowGeom { hwnd: 77, origin_x: 0, origin_y: 0, client_w: 1920, client_h: 1080 }
    }

    fn tick(now_ms: u64, fresh: bool, foreground: bool) -> TickInput {
        TickInput { now_ms, wow: Some(WowTick { geom: geom(), minimized: false, foreground }), fresh }
    }

    fn translate(req_id: u16, slot: u8, target: &str) -> Message {
        Message::Translate(Translate {
            req_id,
            slot,
            rows: 2,
            no_cache: false,
            plate: true,
            target: target.into(),
            text: ChatText::new("synthetic".into()),
            pairs: Vec::new(),
        })
    }

    fn view(id: u8) -> View {
        View { id, x: 20, y: 700, w: 600, h: 300, font_px: 14, row_px: 16 }
    }

    fn no_cover(_: &Card) -> bool {
        false
    }

    fn kinds(s: &Scene) -> Vec<CardKind> {
        s.views.iter().flat_map(|v| v.cards.iter().map(|c| c.kind.clone())).collect()
    }

    #[test]
    fn unknown_slot_turns_missing_after_1500_ms() {
        let mut m = SceneMachine::new();
        let s = m.update(&tick(1000, true, true), vec![card(5, 0, 900, 2, 1.0)], &[], &mut no_cover);
        assert_eq!(kinds(&s), vec![CardKind::Pending]);
        let s = m.update(&tick(2499, true, true), vec![card(5, 0, 900, 2, 1.0)], &[], &mut no_cover);
        assert_eq!(kinds(&s), vec![CardKind::Pending]);
        let s = m.update(&tick(2500, true, true), vec![card(5, 0, 900, 2, 1.0)], &[], &mut no_cover);
        assert_eq!(kinds(&s), vec![CardKind::Missing]);
        // The plate disappears: the timer restarts.
        let s = m.update(&tick(2600, true, true), vec![], &[], &mut no_cover);
        assert_eq!(s.card_count(), 0);
        let s = m.update(&tick(2700, true, true), vec![card(5, 0, 900, 2, 1.0)], &[], &mut no_cover);
        assert_eq!(kinds(&s), vec![CardKind::Pending]);
        // A card without a plate never claims "missing".
        let mut c = card(6, 0, 600, 1, 1.0);
        c.plate = false;
        let s = m.update(&tick(9000, true, true), vec![c.clone()], &[], &mut no_cover);
        let s2 = m.update(&tick(12000, true, true), vec![c], &[], &mut no_cover);
        assert_eq!(kinds(&s), vec![CardKind::Pending]);
        assert_eq!(kinds(&s2), vec![CardKind::Pending]);
    }

    #[test]
    fn translate_makes_the_slot_known() {
        let mut m = SceneMachine::new();
        let acts = m.on_message(42, translate(7, 5, "fr"));
        assert_eq!(acts.len(), 1);
        match &acts[0] {
            Action::Submit(j) => {
                assert_eq!(j.key, JobKey { session: 42, req_id: 7 });
                assert_eq!(j.target, "fr");
                assert_eq!(j.text.as_str(), "synthetic");
            }
            _ => panic!("expected Submit"),
        }
        assert_eq!(m.plate_expected(5), Some(true));
        assert_eq!(m.plate_expected(6), None);
        let s = m.update(&tick(10, true, true), vec![card(5, 0, 900, 2, 1.0)], &[], &mut no_cover);
        assert_eq!(kinds(&s), vec![CardKind::Request { key: JobKey { session: 42, req_id: 7 }, rows: 2, target: "fr".into() }]);
        assert_eq!(s.lang_target.as_deref(), Some("fr"));
    }

    #[test]
    fn new_request_on_a_slot_cancels_the_old_one() {
        let mut m = SceneMachine::new();
        m.on_message(42, translate(7, 5, "fr"));
        let acts = m.on_message(42, translate(7, 5, "fr"));
        assert!(matches!(acts.as_slice(), [Action::Submit(_)]), "same reqId: resubmit only");
        let acts = m.on_message(42, translate(8, 5, "fr"));
        assert!(matches!(acts.as_slice(), [Action::Cancel(k), Action::Submit(_)] if *k == JobKey { session: 42, req_id: 7 }));
    }

    #[test]
    fn cancel_clear_test_view() {
        let mut m = SceneMachine::new();
        m.on_message(42, translate(7, 5, "fr"));
        let acts = m.on_message(42, Message::Cancel { req_id: 9, slot: 5 });
        assert!(matches!(acts.as_slice(), [Action::Cancel(k)] if k.req_id == 9));
        assert!(m.slot(5).is_some(), "a cancel for another reqId keeps the slot");
        m.on_message(42, Message::Cancel { req_id: 7, slot: 5 });
        assert!(m.slot(5).is_none());
        m.on_message(42, translate(8, 3, "de"));
        let acts = m.on_message(42, Message::Clear);
        assert!(matches!(acts.as_slice(), [Action::ClearSession(42)]));
        assert_eq!(m.slot_count(), 0);
        m.on_frame(&FrameInfo { session: 42, mode: Mode::Robust, addon_build: ADDON_BUILD, min_gap: 41.6, white: [250.0; 3], black: [3.0; 3] }, 0);
        let acts = m.on_message(42, Message::Test { nonce: 3 });
        assert!(matches!(acts.as_slice(), [Action::Toast(ToastKind::BridgeOk { mode: Mode::Robust, margin: 42 })]));
        assert!(m.discovery());
        m.on_message(42, Message::View(vec![view(1)]));
        assert!(!m.discovery());
        m.on_message(42, Message::View(vec![]));
        assert_eq!(m.views().len(), 1, "an empty VIEW keeps the sticky one");
        assert_eq!(m.white_black(), Some(([250.0; 3], [3.0; 3])));
    }

    #[test]
    fn session_change_forgets_everything() {
        let mut m = SceneMachine::new();
        m.on_message(42, translate(7, 5, "fr"));
        m.on_message(42, Message::View(vec![view(1)]));
        let acts = m.on_session_change(42, 43);
        assert!(matches!(acts.as_slice(), [Action::ClearSession(42)]));
        assert_eq!(m.slot_count(), 0);
        assert!(m.discovery());
        assert_eq!(m.sess_bit(), Some(1));
        let s = m.update(&tick(10, true, true), vec![card(5, 0, 900, 1, 1.0)], &[], &mut no_cover);
        assert_eq!(kinds(&s), vec![CardKind::Pending]);
    }

    #[test]
    fn stale_frames_hide_everything_in_front_only() {
        let mut m = SceneMachine::new();
        m.on_message(1, translate(1, 2, "en"));
        let s = m.update(&tick(1000, true, true), vec![card(2, 0, 500, 1, 1.0)], &[], &mut no_cover);
        assert!(s.any_visible());
        let s = m.update(&tick(1500, false, true), vec![card(2, 0, 500, 1, 1.0)], &[], &mut no_cover);
        assert!(s.any_visible(), "500 ms is still fresh enough");
        let s = m.update(&tick(1501, false, true), vec![card(2, 0, 500, 1, 1.0)], &[], &mut no_cover);
        assert_eq!(s.hidden, Some(Hidden::Stale));
        assert_eq!(s.card_count(), 0);
        let s = m.update(&tick(5000, false, false), vec![card(2, 0, 500, 1, 1.0)], &[], &mut no_cover);
        assert!(s.any_visible(), "in the background a still image is normal");
    }

    #[test]
    fn minimized_and_gone() {
        let mut m = SceneMachine::new();
        let mut t = tick(10, true, true);
        t.wow.as_mut().unwrap().minimized = true;
        let s = m.update(&t, vec![card(2, 0, 500, 1, 1.0)], &[], &mut no_cover);
        assert_eq!(s.hidden, Some(Hidden::Minimized));
        assert_eq!(s.card_count(), 0);
        let s = m.update(&TickInput { now_ms: 20, wow: None, fresh: false }, vec![], &[], &mut no_cover);
        assert_eq!(s.hidden, Some(Hidden::NoWow));
        assert!(s.wow.is_none());
    }

    #[test]
    fn covered_and_faded_cards() {
        let mut m = SceneMachine::new();
        let mut cover = |c: &Card| c.slot == 3;
        let s = m.update(&tick(10, true, true), vec![card(3, 0, 500, 1, 1.0), card(4, 0, 600, 1, 0.29), card(127, 0, 700, 1, 0.3)], &[], &mut cover);
        let cards: Vec<(u8, bool, CardKind)> = s.views[0].cards.iter().map(|c| (c.card.slot, c.covered, c.kind.clone())).collect();
        assert_eq!(cards, vec![(3, true, CardKind::Pending), (127, false, CardKind::Test)]);
        assert!(s.any_visible());
    }

    #[test]
    fn views_and_overlay_areas() {
        let mut m = SceneMachine::new();
        let areas = m.view_areas(1920, 1080);
        assert_eq!(areas.len(), 1);
        let (id, r, f) = areas[0];
        assert_eq!((id, r.x, r.y, r.w, r.h, f), (0, 0, 16, 1920, 1064, 0));
        m.on_message(1, Message::View(vec![View { id: 1, x: 10, y: 4, w: 400, h: 200, font_px: 14, row_px: 16 }, view(3)]));
        let areas = m.view_areas(1920, 1080);
        assert_eq!(areas.len(), 2);
        let (_, r, f) = areas[0];
        assert_eq!((r.x, r.y, r.w, r.h, f), (10, 16, 400, 188, 14), "never above row 16");
        // Cards go to their view; a card for an unknown view id is dropped.
        let s = m.update(&tick(10, true, true), vec![card(9, 3, 800, 1, 1.0), card(9, 2, 800, 1, 1.0)], &[], &mut no_cover);
        assert_eq!(s.views.iter().map(|v| v.cards.len()).collect::<Vec<_>>(), vec![0, 1]);
    }

    #[test]
    fn addon_build_mismatch_toasts_once_per_session() {
        let mut m = SceneMachine::new();
        let f = |session, build| FrameInfo { session, mode: Mode::Dense, addon_build: build, min_gap: 40.0, white: [255.0; 3], black: [0.0; 3] };
        assert!(m.on_frame(&f(5, ADDON_BUILD), 0).is_empty());
        assert!(matches!(m.on_frame(&f(5, ADDON_BUILD + 1), 0).as_slice(), [Action::Toast(ToastKind::HelperOld)]));
        assert!(m.on_frame(&f(5, ADDON_BUILD + 1), 0).is_empty());
        // the build before rows 1..6 (b8a3e23) is told to update
        assert!(matches!(m.on_frame(&f(6, 1), 0).as_slice(), [Action::Toast(ToastKind::AddonOld)]));
    }

    #[test]
    fn weak_signal_after_10_s() {
        let mut m = SceneMachine::new();
        let f = |gap| FrameInfo { session: 5, mode: Mode::Dense, addon_build: ADDON_BUILD, min_gap: gap, white: [255.0; 3], black: [0.0; 3] };
        for t in 0..10 {
            assert!(m.on_frame(&f(10.0), t * 999).is_empty());
        }
        assert!(matches!(m.on_frame(&f(10.0), 10_000).as_slice(), [Action::Balloon(BalloonKind::WeakSignal)]));
        assert!(m.on_frame(&f(10.0), 20_000).is_empty(), "once");
        let mut good = SceneMachine::new();
        for t in 0..30 {
            assert!(good.on_frame(&f(40.0), t * 1000).is_empty());
        }
    }

    #[test]
    fn block_px_comes_from_the_hit() {
        let mut m = SceneMachine::new();
        let hit = TagHit { slot: 127, row: 0, sess_bit: 0, x0: 10, x1: 57, y_top: 500, y_bot: 513, block_px: 5.5, pitch_px: 6.0, alpha: 1.0 };
        let s = m.update(&tick(10, true, true), vec![card(127, 0, 500, 3, 1.0)], &[hit], &mut no_cover);
        assert_eq!(s.views[0].cards[0].block_px, 5.5);
    }

    #[test]
    fn rows_expected_follow_translate() {
        let mut m = SceneMachine::new();
        let t = Translate { req_id: 7, slot: 5, rows: 6, no_cache: false, plate: true, target: "fr".into(), text: ChatText::new("x".into()), pairs: Vec::new() };
        m.on_message(42, Message::Translate(t));
        assert_eq!(m.rows_expected(5), Some(6));
        assert_eq!(m.rows_expected(6), None);
    }

    #[test]
    fn fingerprint_follows_the_background() {
        use crate::backdrop::{BandBg, Sample};
        let mut m = SceneMachine::new();
        let bg = |r: u8| CardBg { bands: vec![BandBg { left: Some(Sample { rgb: [r, 20, 30], x: 5 }), right: None }], edges: Vec::new() };
        let a = m.update_with_bg(&tick(10, true, true), vec![(card(3, 0, 500, 1, 1.0), bg(100))], &[], &mut no_cover);
        let b = m.update_with_bg(&tick(20, true, true), vec![(card(3, 0, 500, 1, 1.0), bg(101))], &[], &mut no_cover);
        let c = m.update_with_bg(&tick(30, true, true), vec![(card(3, 0, 500, 1, 1.0), bg(120))], &[], &mut no_cover);
        assert_eq!(a.fingerprint(), b.fingerprint(), "a 1-level change does not republish");
        assert_ne!(b.fingerprint(), c.fingerprint());
        assert_eq!(c.views[0].cards[0].bg, bg(120));
    }

    #[test]
    fn fingerprint_follows_changes() {
        let mut m = SceneMachine::new();
        let a = m.update(&tick(10, true, true), vec![card(3, 0, 500, 1, 1.0)], &[], &mut no_cover);
        let b = m.update(&tick(20, true, true), vec![card(3, 0, 500, 1, 1.0)], &[], &mut no_cover);
        let c = m.update(&tick(30, true, true), vec![card(3, 0, 502, 1, 1.0)], &[], &mut no_cover);
        assert_eq!(a.fingerprint(), b.fingerprint());
        assert_ne!(b.fingerprint(), c.fingerprint());
        let mut masked = card(3, 0, 502, 1, 1.0);
        masked.bands[0].mask.as_mut().unwrap().bits[5] = 0;
        let d = m.update(&tick(40, true, true), vec![masked], &[], &mut no_cover);
        assert_ne!(c.fingerprint(), d.fingerprint());
    }
}
