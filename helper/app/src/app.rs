//! The Helper process (design D.3): main/UI thread, capture thread and engine wiring.
//!
//! - UI thread: hidden window `TPHelperMain`, tray icon, overlay windows, toast. It handles
//!   `WM_APP_SCENE` (newest scene), `WM_APP_RESULT` (engine updates), `WM_APP_STATE` and
//!   `WM_APP_EVENT` (toasts, balloons), and renders only on change.
//! - Capture thread: finds the game window, grabs the strip and the chat areas (one region per
//!   VIEW rectangle, two merged only when their union is at most 1.3 times their total area),
//!   decodes the bridge, feeds the engine, finds the cards and publishes a scene when it changed.
//!   A tag seen in two overlapping views is kept once, in the first (window views come first).
//! - Replies (conversation window): the UI thread owns the clipboard flow (`reply`, `clip`):
//!   it empties the clipboard when a consented reply request arrives, writes the final
//!   translation, and paints the reply card's status from the outcome.
//! - Status file (2026-10-19, user request R1): a thread writes `CompanionStatus.lua` into the
//!   deployed addon folders at start, every 60 s, and "stopped" on every clean exit
//!   (`status_file`), so the addon knows at login or `/reload` that the app runs.
//! - Presence card (slot 125, the addon's Translate Companion popup): found in a VIEW of its own,
//!   or, before any VIEW (an app started while the popup is open), by the discovery scan, which
//!   also searches the middle of the client for it (`discovery_areas`).
//! - Remote translation services (2026-10-20): the tray's "Translation engine..." opens the
//!   settings window (`settings_window`); the saved service, its consent and its key (`keys.bin`,
//!   DPAPI) go to the engine (`EngineHandle::set_remote`). A result the engine translated locally
//!   after a remote failure carries the reason (`Update::fallback`): the overlay ends it with a
//!   warning badge and the UI thread puts a hover window over the badge for its tooltip (`badge`).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tp_bridge::cards::{Card, Mask, RowBand, StickyWidth, TAG_ROWS};
use tp_bridge::image::{Bgra, Rect};
use tp_bridge::msg::{Message, View};
use tp_bridge::reasm::Reassembler;
use tp_bridge::strip::{self, Mode, StripRead};
use tp_bridge::tags::{TagHit, TEST_SLOT};
use tp_bridge::tone::Tone;
use tp_engine::{Engine, EngineConfig, EngineEvent, EngineHandle, EngineState, JobKey, Status, Translator, Update};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::backdrop::{self, CardBg};
use crate::badge::{Badges, WM_APP_BADGE};
use crate::capture::gdi::GdiGrabber;
use crate::capture::wgc::{self, WgcGrabber};
use crate::capture::{self, GrabStatus, Grabber, Method};
use crate::cli::{Backend, CaptureChoice, Cli, TranslatorChoice, USAGE};
use crate::clip::{ClipOs, ClipOutcome, WinClip};
use crate::i18n::{self, Lang};
use crate::log::{AppEvent, DiagSink, Level, Logger};
use crate::overlay::{self, window::hinstance, window::ZMode, OverlayManager};
use crate::perf::{self, Stage, Stages, ThreadClock};
use crate::remote::{self as remote_ui, Choice};
use crate::reply::{ClipEvent, Replies};
use crate::tagscan::{CardCache, TagScanner};
use crate::scene::*;
use crate::settings::{self, LoadOutcome, Settings};
use crate::settings_window::{self, KeyFileBackend, OpenArgs, SettingsEvent, WM_APP_SETTINGS};
use tp_engine::remote::keystore::Keys;
use tp_engine::remote::{Fallback, Provider};
use crate::status_file::{StatusWriter, WriterConfig};
use crate::toast::Toaster;
use crate::tray::{MenuCmd, MenuItem, Tray, WM_APP_TRAY};
use crate::wow::{self, hwnd, WowFinder, WowInfo};
use tp_bridge::msg::ChatText;

pub const WM_APP_SCENE: u32 = WM_APP + 1;
pub const WM_APP_RESULT: u32 = WM_APP + 2;
pub const WM_APP_STATE: u32 = WM_APP + 3;
pub const WM_APP_EVENT: u32 = WM_APP + 4;
const WM_APP_TRAY_READD: u32 = WM_APP + 5;
/// The foreground window changed (WinEvent hook): the overlay's z-order follows at once.
const WM_APP_FOREGROUND: u32 = WM_APP + 6;

const TIMER_EXIT: usize = 1;
const TIMER_ANIM: usize = 2;
const TIMER_TOAST: usize = 3;
const TIMER_PROBE: usize = 4;

pub const MAIN_CLASS: PCWSTR = w!("TPHelperMain");

/// How often the game window is looked for while there is none.
pub const WOW_LOOKUP_EVERY: Duration = Duration::from_secs(2);
/// Capture period with the game in front and nothing pending (no strip seen lately, no card
/// visible): the strip only has to be noticed; a chunk stays on screen for 400 ms.
pub const IDLE_PERIOD_MS: u64 = 100;
/// Capture period while the strip is (or just was) on screen.
pub const BUSY_PERIOD_MS: u64 = 50;
/// Capture period with the game not in front and the strip not seen lately (the strip is still
/// read: a chunk stays on screen for 400 ms; once it is seen the busy period applies).
pub const BACKGROUND_PERIOD_MS: u64 = 200;
/// The strip counts as active this long after it was last seen (any non-absent read).
pub const STRIP_RECENT_MS: u64 = 2000;
/// Tag scans while nothing is known (no request this session, no tag in the last scan, no strip):
/// the discovery cadence of the design (2 Hz).
pub const QUIET_SCAN_MS: u64 = 500;
/// Discovery while the last scan found the presence card (the addon's popup, no VIEW known): 10 Hz,
/// so its "detected" text leaves within about 100 ms of the popup (2 Hz would leave it floating over
/// the game for up to half a second).
pub const PRESENCE_SCAN_MS: u64 = 100;
/// Discovery searches tags whose first block starts in the left `PRESENCE_REACH` of the client: the
/// presence card sits in the addon's popup, in the middle of the screen, out of the left
/// max(200 px, 35%) that `tags::scan` searches in one area.
pub const PRESENCE_REACH: f32 = 0.65;
/// Each extra discovery area starts this far left of where the previous one's search range ends.
pub const DISCOVERY_OVERLAP: i32 = 16;
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Events for the UI thread (no text, except a reply's own text, which stays in `ChatText` and
/// in memory).
pub enum UiEvent {
    Toast { kind: ToastKind, method: Method, hz: u32 },
    Balloon(BalloonKind),
    /// A reply request was accepted: start its clipboard flow (before any engine update for it).
    ReplyStart { key: JobKey, copy: bool, text: ChatText },
    /// The reply ended: release the clipboard if it still holds the Helper's write.
    ReplyEnd { key: JobKey },
}

#[derive(Default)]
struct Inbox {
    scene: Option<Scene>,
    updates: Vec<Update>,
    state: Option<EngineState>,
    events: Vec<UiEvent>,
}

/// State shared by the UI thread, the capture thread and the engine callbacks.
pub struct Shared {
    main: AtomicIsize,
    inbox: Mutex<Inbox>,
    /// Latest status name per job, for the `cards` diag event.
    status_names: Mutex<HashMap<JobKey, &'static str>>,
    /// Clipboard outcome per reply (set by the UI thread), for the `cards` diag event.
    clip_names: Mutex<HashMap<JobKey, &'static str>>,
    submit_times: Mutex<HashMap<JobKey, Instant>>,
    /// UI thread id and render counters, for the `perf` diag record.
    ui_thread: AtomicU32,
    renders: AtomicU64,
    render_ns: AtomicU64,
}

impl Shared {
    pub fn new(main: isize) -> Shared {
        Shared {
            main: AtomicIsize::new(main),
            inbox: Mutex::new(Inbox::default()),
            status_names: Mutex::new(HashMap::new()),
            clip_names: Mutex::new(HashMap::new()),
            submit_times: Mutex::new(HashMap::new()),
            ui_thread: AtomicU32::new(0),
            renders: AtomicU64::new(0),
            render_ns: AtomicU64::new(0),
        }
    }

    fn post(&self, msg: u32) {
        let h = self.main.load(Ordering::Relaxed);
        if h != 0 {
            unsafe {
                let _ = PostMessageW(Some(hwnd(h)), msg, WPARAM(0), LPARAM(0));
            }
        }
    }

    fn publish_scene(&self, s: Scene) {
        if let Ok(mut i) = self.inbox.lock() {
            i.scene = Some(s);
        }
        self.post(WM_APP_SCENE);
    }

    fn push_update(&self, u: Update) {
        if let Ok(mut i) = self.inbox.lock() {
            i.updates.push(u);
        }
        self.post(WM_APP_RESULT);
    }

    fn push_state(&self, s: EngineState) {
        if let Ok(mut i) = self.inbox.lock() {
            i.state = Some(s);
        }
        self.post(WM_APP_STATE);
    }

    fn push_event(&self, e: UiEvent) {
        if let Ok(mut i) = self.inbox.lock() {
            i.events.push(e);
        }
        self.post(WM_APP_EVENT);
    }

    fn note_submit(&self, key: JobKey) {
        if let Ok(mut t) = self.submit_times.lock() {
            if t.len() > 4096 {
                t.clear();
            }
            t.insert(key, Instant::now());
        }
        if let Ok(mut n) = self.status_names.lock() {
            if n.len() > 4096 {
                n.clear();
            }
            n.insert(key, "sent");
        }
    }

    fn status_name(&self, key: &JobKey) -> &'static str {
        self.status_names.lock().ok().and_then(|n| n.get(key).copied()).unwrap_or("sent")
    }

    fn set_clip_name(&self, key: JobKey, name: &'static str) {
        if let Ok(mut n) = self.clip_names.lock() {
            if n.len() > 256 {
                n.clear();
            }
            n.insert(key, name);
        }
    }

    fn clip_name(&self, key: &JobKey) -> Option<&'static str> {
        self.clip_names.lock().ok().and_then(|n| n.get(key).copied())
    }
}

pub fn mode_name(m: Mode) -> &'static str {
    match m {
        Mode::Dense => "dense",
        Mode::Robust => "robust",
        Mode::Mono => "mono",
    }
}

fn state_key(s: EngineState) -> &'static str {
    match s {
        EngineState::NotInstalled => "tray.state.notinstalled",
        EngineState::Unloaded => "tray.state.unloaded",
        EngineState::Loading => "tray.state.loading",
        EngineState::ReadyGpu => "tray.state.ready_gpu",
        EngineState::ReadyCpu => "tray.state.ready_cpu",
        EngineState::Busy => "tray.state.busy",
        EngineState::Error => "tray.state.error",
    }
}

/// The tray tooltip: the version and the translator's state, with the remote service in front
/// when one is used ("Translate Companion 0.7.0 - DeepL (local: translator ready (CPU))").
pub fn tray_tip(lang: Lang, remote: Option<Provider>, state: EngineState) -> String {
    let st = i18n::tr(lang, state_key(state));
    match remote {
        Some(p) => i18n::tr_fmt(lang, "tray.tip_remote", &[("version", VERSION), ("provider", remote_ui::provider_name(lang, p)), ("state", st)]),
        None => i18n::tr_fmt(lang, "tray.tip", &[("version", VERSION), ("state", st)]),
    }
}

fn state_name(s: EngineState) -> &'static str {
    match s {
        EngineState::NotInstalled => "notinstalled",
        EngineState::Unloaded => "unloaded",
        EngineState::Loading => "loading",
        EngineState::ReadyGpu => "ready_gpu",
        EngineState::ReadyCpu => "ready_cpu",
        EngineState::Busy => "busy",
        EngineState::Error => "error",
    }
}

/// Engine configuration from the command line and the settings (command line first).
pub fn engine_config(cli: &Cli, s: &Settings, log: Arc<dyn Fn(EngineEvent) + Send + Sync>) -> EngineConfig {
    let backend = cli.backend.or_else(|| Backend::parse(&s.backend)).unwrap_or(Backend::Auto);
    EngineConfig {
        runtime_dir: cli.runtime.clone().or_else(|| s.runtime_dir_opt()),
        model_path: cli.model.clone().or_else(|| s.model_opt()),
        backend: backend.to_engine(),
        gpu_margin_mb: s.gpu_margin_mb,
        cpu_threads: s.cpu_threads,
        idle_unload: Duration::from_secs(u64::from(s.idle_unload_minutes) * 60),
        keep_loaded: s.keep_loaded,
        translator: match cli.translator {
            TranslatorChoice::Llama => Translator::Llama,
            TranslatorChoice::Echo => Translator::Echo,
        },
        data_dir: settings::data_dir(),
        log,
    }
}

// ---------------------------------------------------------------------------------------------
// --diag-json records (numbers and fixed names only)

fn mask_runs(m: &Mask) -> Vec<[i32; 3]> {
    let mut runs = Vec::new();
    for y in 0..m.rect.h {
        let mut x = 0;
        while x < m.rect.w {
            if m.bits[(y * m.rect.w + x) as usize] != 0 {
                let x0 = x;
                while x < m.rect.w && m.bits[(y * m.rect.w + x) as usize] != 0 {
                    x += 1;
                }
                runs.push([m.rect.y + y, m.rect.x + x0, m.rect.x + x]);
            } else {
                x += 1;
            }
        }
    }
    runs
}

fn r4(r: &Rect) -> [i32; 4] {
    [r.x, r.y, r.w, r.h]
}

fn round3(v: f32) -> f64 {
    (f64::from(v) * 1000.0).round() / 1000.0
}

/// A finite number as JSON (serde_json's form for f64).
fn json_f64(v: f64) -> String {
    Value::from(v).to_string()
}

/// A background sample as `[r, g, b]` or null.
fn sample_json(s: Option<backdrop::Sample>) -> Value {
    s.map_or(Value::Null, |s| json!([s.rgb[0], s.rgb[1], s.rgb[2]]))
}

/// One card of the `cards` diag event. `state` is the engine status name (or the card kind).
pub fn card_json(sc: &SceneCard, state: &str) -> Value {
    card_json_clip(sc, state, None)
}

/// [`card_json`] with a reply's clipboard outcome (`clip`, None for a chat card or before the
/// final result). Also: `reply` and the background samples of each band (`bg`, [left, right]
/// colours, numbers only).
pub fn card_json_clip(sc: &SceneCard, state: &str, clip: Option<&str>) -> Value {
    let c = &sc.card;
    let (req, reply) = match &sc.kind {
        CardKind::Request { key, reply, .. } => (json!(key.req_id), *reply),
        _ => (Value::Null, false),
    };
    let bg: Vec<Value> = (0..c.bands.len()).map(|i| sc.bg.band(i)).map(|b| json!([sample_json(b.left), sample_json(b.right)])).collect();
    let band = |b: &RowBand| r4(&b.text_rect);
    let mask_px: usize = c.bands.iter().filter_map(|b| b.mask.as_ref()).map(|m| m.bits.iter().filter(|v| **v != 0).count()).sum();
    let masks: Vec<Value> = c
        .bands
        .iter()
        .map(|b| match &b.mask {
            Some(m) => json!({"rect": r4(&m.rect), "runs": mask_runs(m)}),
            None => Value::Null,
        })
        .collect();
    json!({
        "slot": c.slot, "req": req, "view": c.view_id, "kind": sc.kind.name(),
        "rows": c.bands.iter().map(|b| b.row).collect::<Vec<_>>(),
        "bands": c.bands.iter().map(band).collect::<Vec<_>>(),
        "tags": c.bands.iter().map(|b| r4(&b.tag_rect)).collect::<Vec<_>>(),
        "gaps": c.gaps.iter().map(r4).collect::<Vec<_>>(),
        "maskPx": mask_px, "masks": masks, "alpha": round3(c.alpha), "plate": c.plate,
        "textWidth": c.text_width_px, "state": state, "covered": sc.covered,
        "reply": reply, "clip": clip, "bg": bg,
    })
}

/// The `msg` diag record and log event of one bridge message (numbers only).
pub fn msg_json(m: &Message) -> (Value, AppEvent) {
    match m {
        Message::Translate(t) => (
            json!({"ev": "msg", "kind": "translate", "req": t.req_id, "slot": t.slot, "rows": t.rows, "bytes": t.text.len(),
                   "pairs": t.pairs.len(), "noCache": t.no_cache, "plate": t.plate, "reply": t.reply, "copy": t.copy}),
            AppEvent::Message { kind: "translate", req: t.req_id, slot: t.slot, rows: t.rows, bytes: t.text.len() as u32, pairs: t.pairs.len() as u8 },
        ),
        Message::Cancel { req_id, slot } => (
            json!({"ev": "msg", "kind": "cancel", "req": req_id, "slot": slot}),
            AppEvent::Message { kind: "cancel", req: *req_id, slot: *slot, rows: 0, bytes: 0, pairs: 0 },
        ),
        Message::Clear => (json!({"ev": "msg", "kind": "clear"}), AppEvent::Message { kind: "clear", req: 0, slot: 0, rows: 0, bytes: 0, pairs: 0 }),
        Message::Test { nonce } => (
            json!({"ev": "msg", "kind": "test", "nonce": nonce}),
            AppEvent::Message { kind: "test", req: *nonce, slot: 0, rows: 0, bytes: 0, pairs: 0 },
        ),
        Message::View(v) => (
            json!({"ev": "msg", "kind": "view", "n": v.len(),
                   "views": v.iter().map(|x| [x.id as i32, x.x as i32, x.y as i32, x.w as i32, x.h as i32, x.font_px as i32, x.row_px as i32]).collect::<Vec<_>>()}),
            AppEvent::Message { kind: "view", req: 0, slot: 0, rows: v.len() as u8, bytes: 0, pairs: 0 },
        ),
        Message::Colours { req_id, spans } => (
            json!({"ev": "msg", "kind": "colours", "req": req_id, "n": spans.len()}),
            AppEvent::Message { kind: "colours", req: *req_id, slot: 0, rows: spans.len().min(255) as u8, bytes: 0, pairs: 0 },
        ),
    }
}

// ---------------------------------------------------------------------------------------------
// Capture thread

pub struct CaptureCtx {
    pub wow_class: Option<String>,
    pub choice: CaptureChoice,
    pub hz_active: u32,
    pub assume_fg: bool,
    /// Hidden `--assume-background`: the game window counts as not in front (E2E of D12).
    pub assume_bg: bool,
    /// `--no-overlay`: skip the WindowFromPoint covering test (nothing is drawn on screen, and the
    /// A1 test rig keeps its window under other windows on purpose).
    pub assume_uncovered: bool,
    pub engine: Option<EngineHandle>,
    pub shared: Arc<Shared>,
    pub stop: Arc<AtomicBool>,
    pub log: Arc<Logger>,
    pub diag: Arc<DiagSink>,
}

#[derive(Default)]
struct Minute {
    ticks: u64,
    fresh: u64,
    stale: u64,
    errors: u64,
    ok: u64,
    rejected: u64,
    absent: u64,
    torn: u64,
    gap_sum: f64,
    messages: u64,
    scans: u64,
    hits: u64,
    cards: u64,
    covered: u64,
}

struct Pipeline {
    ctx: CaptureCtx,
    finder: WowFinder,
    t0: Instant,
    hwnd: Option<isize>,
    last_find: Option<Instant>,
    present: Option<bool>,
    grabber: Option<Box<dyn Grabber>>,
    wgc_target: isize,
    use_gdi: bool,
    wgc_recreated: bool,
    retry_at: Option<Instant>,
    reasm: Reassembler,
    scene: SceneMachine,
    /// Method of the grabber while `step` has taken it out to read its regions.
    taken_method: Option<Method>,
    cards: Vec<Card>,
    /// The chat background around each card of `cards` (same order), for the native look.
    card_bgs: Vec<CardBg>,
    hits: Vec<TagHit>,
    /// The last discovery scan found the presence card (slot 125): discovery runs at
    /// `PRESENCE_SCAN_MS` while it shows.
    presence_seen: bool,
    tick: u64,
    last_fresh: Instant,
    last_fp: Option<u64>,
    last_states: Vec<(&'static str, Option<&'static str>)>,
    last_hit_count: usize,
    visible: bool,
    minimized: bool,
    /// Covering test results: (view id, slot, game window in front) -> (when, covered).
    cover_cache: HashMap<(u8, u8, bool), (Instant, bool)>,
    minute: Minute,
    minute_start: Instant,
    perf: PerfWindow,
    /// Last strip read that was not `Absent` (the strip is on screen, possibly torn).
    strip_seen: Option<Instant>,
    /// Last tag scan.
    last_scan: Option<Instant>,
    /// One per capture region (design B.3 H8): its tag search, card cache and linear copies.
    regions: Vec<Region>,
    /// Design step 7: each card's text width is the widest of the last 2 s, so a short-lived
    /// misread (a cover, a fading plate end) does not re-wrap the text.
    sticky: StickyWidth,
    /// Display curve measured on the strip (sticky like the strip calibration); when it is not
    /// linear, each scan region is linearized before the tag scan and the cards (`tone`).
    tone: Tone,
    frame_cache: wow::FrameBoundsCache,
    /// The D3D11 device of the last WGC session, kept for the next one (creating one costs about
    /// 0.25 s of CPU).
    wgc_device: Option<wgc::WgcDevice>,
}

/// One second of CPU accounting (the `perf` diag record).
struct PerfWindow {
    start: Instant,
    stages: Stages,
    cpu_proc: f64,
    cycles: u64,
    cpu_cap: f64,
    cpu_ui: f64,
    renders: u64,
    render_ns: u64,
    ui_clock: Option<ThreadClock>,
    ticks: u32,
    fresh: u32,
    strip_ok: u32,
    scans: u32,
    pixels: u64,
    area_rows: u64,
    scanned_rows: u64,
    bands: u64,
    reused_bands: u64,
    cards_kept: u64,
    saw_wow: bool,
    saw_min: bool,
    saw_visible: bool,
}

impl PerfWindow {
    fn new() -> PerfWindow {
        PerfWindow {
            start: Instant::now(),
            stages: Stages::default(),
            cpu_proc: perf::process_cpu_ms(),
            cycles: perf::process_cycles(),
            cpu_cap: perf::thread_cpu_ms(),
            cpu_ui: 0.0,
            renders: 0,
            render_ns: 0,
            ui_clock: None,
            ticks: 0,
            fresh: 0,
            strip_ok: 0,
            scans: 0,
            pixels: 0,
            area_rows: 0,
            scanned_rows: 0,
            bands: 0,
            reused_bands: 0,
            cards_kept: 0,
            saw_wow: false,
            saw_min: false,
            saw_visible: false,
        }
    }
}

/// One capture region of the tag scan (one VIEW rectangle, or two merged): its own tag search
/// memory, card cache and linear copies, and the cards it gave on the previous capture.
#[derive(Default)]
struct Region {
    tagscan: TagScanner,
    cards: CardCache,
    /// Linearized current and spare buffers, and the grab number and rectangle of the capture
    /// `lin` holds (it is the previous capture's linear copy on the next tick).
    lin: Vec<u8>,
    lin_spare: Vec<u8>,
    lin_key: Option<(u64, [i32; 4])>,
    prev_cards: Vec<Card>,
    prev_bgs: Vec<CardBg>,
}

impl Region {
    fn reset(&mut self) {
        self.tagscan.reset();
        self.cards.reset();
        self.lin_key = None;
        self.prev_cards.clear();
        self.prev_bgs.clear();
    }
}

/// What one region gave in one capture.
struct RegionOut {
    cards: Vec<Card>,
    bgs: Vec<CardBg>,
    hits: Vec<TagHit>,
}

/// Largest union of two VIEW rectangles grabbed as one region, as a multiple of their total area.
pub const MERGE_MAX: f64 = 1.3;

fn area(r: &Rect) -> f64 {
    f64::from(r.w.max(0)) * f64::from(r.h.max(0))
}

/// The capture regions for these views (clipped to the client): one per view, window views
/// first (the duplicate-tag rule keeps the first reading), two merged while their union is at
/// most `MERGE_MAX` times their total area. Each region lists its views in that order.
pub fn plan_regions(views: &[View], client_w: i32, client_h: i32) -> Vec<(Rect, Vec<View>)> {
    let mut order: Vec<View> = views.to_vec();
    order.sort_by_key(|v| ViewKind::of(v.id) != ViewKind::Window);
    let mut groups: Vec<(Rect, Vec<View>)> = order
        .into_iter()
        .filter_map(|v| capture::clip(Rect { x: v.x.into(), y: v.y.into(), w: v.w.into(), h: v.h.into() }, client_w, client_h).map(|r| (r, vec![v])))
        .collect();
    loop {
        let mut merged = false;
        'outer: for i in 0..groups.len() {
            for j in i + 1..groups.len() {
                let u = capture::union(groups[i].0, groups[j].0);
                if area(&u) <= MERGE_MAX * (area(&groups[i].0) + area(&groups[j].0)) {
                    let (_, vj) = groups.remove(j);
                    groups[i].0 = u;
                    groups[i].1.extend(vj);
                    merged = true;
                    break 'outer;
                }
            }
        }
        if !merged {
            return groups;
        }
    }
}

/// Delay before the next capture with the game window present and not minimized. Not in front:
/// the strip rate while the strip is active (a 400 ms chunk is caught, so a TRANSLATE or CANCEL
/// sent while the game is in the background is not lost), else 5 Hz; the cards stay either way.
pub fn capture_period_ms(fg: bool, visible: bool, strip_recent: bool, hz_active: u32) -> u64 {
    if !fg {
        if strip_recent {
            BUSY_PERIOD_MS
        } else {
            BACKGROUND_PERIOD_MS
        }
    } else if visible {
        u64::from(1000 / hz_active.clamp(20, 60))
    } else if strip_recent {
        BUSY_PERIOD_MS
    } else {
        IDLE_PERIOD_MS
    }
}

/// Keeps the first reading of each tag (slot, row, x0, yTop) across overlapping views and
/// regions (design B.3 H7): `seen` holds the tags already kept this capture.
pub fn dedup_hits(hits: &mut Vec<TagHit>, seen: &mut HashSet<(u8, u8, i32, i32)>) {
    hits.retain(|h| seen.insert((h.slot, h.row, h.x0, h.y_top)));
}

/// Where `tags::scan` looks for a tag's first block in `area`: `[area.x, end)`.
pub fn search_end(area: Rect) -> i32 {
    area.x + area.w.min(200.max((0.35 * area.w as f32) as i32))
}

/// The tag scan areas of a discovery scan (no VIEW) over `area` (the client below row 16): the
/// area itself (chat frames, at the left), then areas that each start `DISCOVERY_OVERLAP` px left
/// of where the previous one's search range ends and reach the area's right edge (a plate is
/// never cut), until the search ranges cover `PRESENCE_REACH` of the width: the presence card
/// (the addon's popup) is in the middle of the screen. Only presence tags are kept from the extra
/// areas ([`keep_discovery_hit`]), so chat cards are found as before.
pub fn discovery_areas(area: Rect) -> Vec<Rect> {
    let mut out = vec![area];
    let right = area.x + area.w;
    let goal = area.x + (area.w as f32 * PRESENCE_REACH) as i32;
    let mut cur = area;
    while cur.w > 0 && search_end(cur) < goal {
        let x = search_end(cur) - DISCOVERY_OVERLAP;
        if x <= cur.x {
            break;
        }
        cur = Rect { x, y: area.y, w: right - x, h: area.h };
        out.push(cur);
    }
    out
}

/// A discovery hit is kept when the first area finds it (any slot) or it is the presence card.
pub fn keep_discovery_hit(h: &TagHit, area: Rect) -> bool {
    h.slot == PRESENCE_SLOT || h.x0 < search_end(area)
}

impl Pipeline {
    fn new(ctx: CaptureCtx) -> Pipeline {
        let finder = WowFinder::new(ctx.wow_class.as_deref());
        let use_gdi = ctx.choice == CaptureChoice::Gdi;
        Pipeline {
            ctx,
            finder,
            t0: Instant::now(),
            hwnd: None,
            last_find: None,
            present: None,
            grabber: None,
            wgc_target: 0,
            use_gdi,
            wgc_recreated: false,
            retry_at: None,
            reasm: Reassembler::new(),
            scene: SceneMachine::new(),
            taken_method: None,
            cards: Vec::new(),
            card_bgs: Vec::new(),
            hits: Vec::new(),
            presence_seen: false,
            tick: 0,
            last_fresh: Instant::now(),
            last_fp: None,
            last_states: Vec::new(),
            last_hit_count: 0,
            visible: false,
            minimized: false,
            cover_cache: HashMap::new(),
            minute: Minute::default(),
            minute_start: Instant::now(),
            perf: PerfWindow::new(),
            strip_seen: None,
            last_scan: None,
            regions: Vec::new(),
            sticky: StickyWidth::new(),
            tone: Tone::identity(),
            frame_cache: wow::FrameBoundsCache::default(),
            wgc_device: None,
        }
    }

    fn strip_recent(&self) -> bool {
        self.strip_seen.is_some_and(|t| t.elapsed() < Duration::from_millis(STRIP_RECENT_MS))
    }

    /// Whether this tick scans for tags. Every tick while a request of this session is known, a
    /// tag was found by the last scan, a card is shown or the strip is active; otherwise only at
    /// the discovery cadence (no card can be on screen that needs a quick reaction).
    fn scan_due(&self) -> bool {
        let active = self.scene.slot_count() > 0 || !self.hits.is_empty() || self.visible || self.strip_recent();
        active || self.last_scan.is_none_or(|t| t.elapsed() >= Duration::from_millis(QUIET_SCAN_MS))
    }

    /// Delay before the next tick with the game window present and not minimized.
    fn period_ms(&self, fg: bool) -> u64 {
        capture_period_ms(fg, self.visible, self.strip_recent(), self.ctx.hz_active)
    }

    /// Writes the `perf` record once per second (only with `--diag-json`).
    fn perf_report(&mut self) {
        let pw = &mut self.perf;
        let wall = pw.start.elapsed();
        if wall < Duration::from_secs(1) || !self.ctx.diag.enabled() {
            return;
        }
        let shared = &self.ctx.shared;
        if pw.ui_clock.is_none() {
            let id = shared.ui_thread.load(Ordering::Relaxed);
            if id != 0 {
                pw.ui_clock = Some(ThreadClock::open(id));
                pw.cpu_ui = pw.ui_clock.as_ref().map(|c| c.cpu_ms()).unwrap_or(0.0);
            }
        }
        let proc_now = perf::process_cpu_ms();
        let cycles_now = perf::process_cycles();
        let cap_now = perf::thread_cpu_ms();
        let ui_now = pw.ui_clock.as_ref().map(|c| c.cpu_ms()).unwrap_or(0.0);
        let renders = shared.renders.load(Ordering::Relaxed);
        let render_ns = shared.render_ns.load(Ordering::Relaxed);
        let state = if !pw.saw_wow {
            "nowow"
        } else if pw.saw_min {
            "minimized"
        } else if pw.strip_ok > 0 {
            "strip"
        } else if pw.saw_visible || self.visible {
            "cards"
        } else {
            "idle"
        };
        let ms = wall.as_secs_f64() * 1000.0;
        self.ctx.diag.emit(json!({
            "ev": "perf", "state": state, "wallMs": perf::round2(ms),
            "cpuMs": perf::round2(proc_now - pw.cpu_proc), "cycles": cycles_now - pw.cycles, "captureCpuMs": perf::round2(cap_now - pw.cpu_cap),
            "uiCpuMs": perf::round2(ui_now - pw.cpu_ui), "renders": renders - pw.renders,
            "renderMs": perf::round2((render_ns - pw.render_ns) as f64 / 1e6),
            "ticks": pw.ticks, "fresh": pw.fresh, "stripOk": pw.strip_ok, "scans": pw.scans, "grabPx": pw.pixels,
            "areaRows": pw.area_rows, "scannedRows": pw.scanned_rows, "bands": pw.bands, "reusedBands": pw.reused_bands, "cardsKept": pw.cards_kept,
            "stages": pw.stages.json(),
        }));
        let ui_clock = pw.ui_clock.take();
        *pw = PerfWindow::new();
        pw.cpu_proc = proc_now;
        pw.cycles = cycles_now;
        pw.cpu_cap = cap_now;
        pw.cpu_ui = ui_now;
        pw.ui_clock = ui_clock;
        pw.renders = renders;
        pw.render_ns = render_ns;
    }

    fn now_ms(&self) -> u64 {
        self.t0.elapsed().as_millis() as u64
    }

    fn method(&self) -> Option<Method> {
        self.grabber.as_ref().map(|g| g.method()).or(self.taken_method)
    }

    fn set_present(&mut self, p: bool) {
        if self.present != Some(p) {
            self.present = Some(p);
            if let Some(e) = &self.ctx.engine {
                e.set_wow_present(p);
            }
        }
    }

    fn exec(&mut self, acts: Vec<Action>) {
        for a in acts {
            match a {
                Action::Submit(job) => {
                    self.ctx.shared.note_submit(job.key);
                    if let Some(e) = &self.ctx.engine {
                        e.submit(job);
                    }
                }
                Action::Cancel(key) => {
                    if let Some(e) = &self.ctx.engine {
                        e.cancel(key);
                    }
                }
                Action::ClearSession(s) => {
                    if let Some(e) = &self.ctx.engine {
                        e.clear_session(s);
                    }
                }
                Action::Toast(kind) => {
                    let method = self.method().unwrap_or(Method::Wgc);
                    self.ctx.shared.push_event(UiEvent::Toast { kind, method, hz: self.ctx.hz_active });
                }
                Action::Balloon(kind) => self.ctx.shared.push_event(UiEvent::Balloon(kind)),
                // pushed before the reply's Submit: the UI thread sees the start before any
                // engine update of the job
                Action::ReplyStart { key, copy, text } => self.ctx.shared.push_event(UiEvent::ReplyStart { key, copy, text }),
                Action::ReplyEnd { key } => self.ctx.shared.push_event(UiEvent::ReplyEnd { key }),
            }
        }
    }

    /// Forgets every region's tag search, cards and linear copies (new window, session, curve).
    fn reset_regions(&mut self) {
        for r in self.regions.iter_mut() {
            r.reset();
        }
    }

    /// Ends the capture session; a WGC device is kept for the next session when `keep_device`.
    fn drop_grabber(&mut self, keep_device: bool) {
        if let Some(g) = self.grabber.take() {
            let dev = g.into_wgc_device();
            if keep_device && dev.is_some() {
                self.wgc_device = dev;
            }
        }
    }

    fn lost(&mut self) {
        if self.hwnd.take().is_some() {
            self.ctx.log.app(Level::Info, AppEvent::WowLost);
            if self.ctx.diag.enabled() {
                self.ctx.diag.emit(json!({"ev": "wow", "found": false}));
            }
        }
        self.drop_grabber(true);
        self.cards.clear();
        self.card_bgs.clear();
        self.hits.clear();
        self.presence_seen = false;
        self.regions.clear();
        self.sticky = StickyWidth::new();
        self.tone = Tone::identity();
        self.set_present(false);
    }

    /// Opens a grabber for the window when needed; false when none is available yet.
    fn ensure_grabber(&mut self, info: &WowInfo) -> bool {
        let need = match &self.grabber {
            None => true,
            Some(g) => g.method() == Method::Wgc && self.wgc_target != info.hwnd,
        };
        if !need {
            return true;
        }
        if self.retry_at.is_some_and(|t| Instant::now() < t) {
            return false;
        }
        self.drop_grabber(true);
        if !self.use_gdi {
            // The first tick sets the cap that matches the capture rate (see `step`).
            let interval = (IDLE_PERIOD_MS * 3 / 4) as u32;
            let dev = self.wgc_device.take();
            let r = if wgc::supported() { WgcGrabber::open_with(info.hwnd, interval, dev).map_err(|e| e.code().0) } else { Err(0) };
            match r {
                Ok(g) => {
                    self.wgc_target = info.hwnd;
                    self.grabber = Some(Box::new(g));
                    self.ctx.log.app(Level::Info, AppEvent::CaptureStarted { method: "wgc", ok: true, hresult: 0 });
                    self.last_fresh = Instant::now();
                    return true;
                }
                Err(code) => {
                    self.ctx.log.app(Level::Warn, AppEvent::CaptureStarted { method: "wgc", ok: false, hresult: code });
                    if self.ctx.choice == CaptureChoice::Wgc {
                        self.retry_at = Some(Instant::now() + Duration::from_secs(2));
                        return false;
                    }
                    self.use_gdi = true;
                    self.ctx.log.app(Level::Warn, AppEvent::CaptureSwitched { from: "wgc", to: "gdi", reason: if code == 0 { "unsupported" } else { "open_failed" } });
                }
            }
        }
        self.grabber = Some(Box::new(GdiGrabber::new()));
        self.ctx.log.app(Level::Info, AppEvent::CaptureStarted { method: "gdi", ok: true, hresult: 0 });
        self.last_fresh = Instant::now();
        true
    }

    /// Regions to grab for the tag scan, each with the views to build cards against.
    fn scan_plan(&self, info: &WowInfo) -> Option<Vec<(Rect, Vec<View>)>> {
        let (cw, ch) = (info.client_w, info.client_h);
        if !self.scan_due() {
            return None;
        }
        if !self.scene.discovery() {
            let regions = plan_regions(self.scene.views(), cw, ch);
            return (!regions.is_empty()).then_some(regions);
        }
        // Discovery runs at 2 Hz whatever the tick rate (10 Hz while the presence card shows).
        let every = if self.presence_seen { PRESENCE_SCAN_MS } else { QUIET_SCAN_MS };
        let discovery_due = self.last_scan.is_none_or(|t| t.elapsed() >= Duration::from_millis(every));
        if !discovery_due || ch <= OVERLAY_TOP {
            return None;
        }
        // Discovery: the whole width below row 16, because a card's plate extends to the right of
        // its tag; `tags::scan` itself searches only the left 35% of the area (`scan_img` adds the
        // areas that find the presence card in the middle of the client).
        let area = Rect { x: 0, y: OVERLAY_TOP, w: cw, h: ch - OVERLAY_TOP };
        let view = View { id: 0, x: 0, y: OVERLAY_TOP as u16, w: cw.clamp(0, 65535) as u16, h: (ch - OVERLAY_TOP).clamp(0, 65535) as u16, font_px: 0, row_px: 0 };
        Some(vec![(area, vec![view])])
    }

    fn decode_strip(&mut self, img: &Bgra) {
        let now = self.now_ms();
        let read = strip::decode_strip(img);
        if !matches!(read, StripRead::Absent) {
            self.strip_seen = Some(Instant::now());
        }
        let diag = self.ctx.diag.clone();
        match read {
            StripRead::Absent => self.minute.absent += 1,
            StripRead::Torn => {
                self.minute.torn += 1;
                if diag.enabled() {
                    diag.emit(json!({"ev": "frame", "ok": false, "why": "torn"}));
                }
            }
            StripRead::Rejected => {
                self.minute.rejected += 1;
                if diag.enabled() {
                    diag.emit(json!({"ev": "frame", "ok": false, "why": "rejected"}));
                }
            }
            StripRead::Frame(f) => {
                self.minute.ok += 1;
                self.perf.strip_ok += 1;
                self.minute.gap_sum += f64::from(f.calib.min_gap);
                if diag.enabled() {
                    diag.emit(json!({"ev": "frame", "ok": true, "mode": mode_name(f.mode), "margin": f.calib.min_gap.max(0.0).round() as u32,
                                     "session": f.session, "seq": f.seq, "chunks": f.chunks.len()}));
                }
                let msgs = self.reasm.push(&f, now);
                if let Some(old) = self.reasm.take_session_change() {
                    let new = self.reasm.session().unwrap_or(f.session);
                    self.ctx.log.app(Level::Info, AppEvent::SessionChange { old, new });
                    if diag.enabled() {
                        diag.emit(json!({"ev": "session", "old": old, "new": new}));
                    }
                    let acts = self.scene.on_session_change(old, new);
                    self.reset_regions();
                    self.sticky = StickyWidth::new();
                    self.exec(acts);
                }
                let tone = Tone::from_levels(&f.calib.levels);
                if !tone.close_to(&self.tone) {
                    if diag.enabled() {
                        let e = tone.exponents();
                        diag.emit(json!({"ev": "tone", "linear": tone.is_identity(), "exp": [round3(e[0]), round3(e[1]), round3(e[2])]}));
                    }
                    self.tone = tone;
                    self.reset_regions();
                }
                let info = FrameInfo {
                    session: f.session,
                    mode: f.mode,
                    addon_build: f.addon_build,
                    min_gap: f.calib.min_gap,
                    white: f.calib.white,
                    black: f.calib.black,
                };
                let acts = self.scene.on_frame(&info, now);
                if !build_served(f.addon_build) {
                    self.ctx.log.app(Level::Warn, AppEvent::AddonBuild { build: f.addon_build, expected: ADDON_BUILD });
                }
                self.exec(acts);
                let session = self.reasm.session().unwrap_or(f.session);
                for m in msgs {
                    self.minute.messages += 1;
                    let (j, ev) = msg_json(&m);
                    if diag.enabled() {
                        diag.emit(j);
                    }
                    self.ctx.log.app(Level::Debug, ev);
                    let acts = self.scene.on_message(session, m);
                    self.exec(acts);
                }
            }
        }
    }

    /// Tag scan of one capture region `ri`: `frame` is its grab number, `raw_prev` the previous
    /// capture of the same rectangle (for keeping unchanged cards). Tags already read in an
    /// earlier region of this capture (`seen`) are dropped.
    #[allow(clippy::too_many_arguments)]
    fn scan_region(&mut self, ri: usize, raw: &Bgra, frame: u64, raw_prev: Option<(Bgra, u64)>, views: Vec<View>, info: &WowInfo, seen: &mut HashSet<(u8, u8, i32, i32)>) -> RegionOut {
        let mut reg = std::mem::take(&mut self.regions[ri]);
        let out = if self.tone.is_identity() {
            reg.lin_key = None;
            self.scan_img(&mut reg, raw, frame, raw_prev, views, info, seen)
        } else {
            self.scan_linear(&mut reg, raw, frame, raw_prev, views, info, seen)
        };
        self.regions[ri] = reg;
        out
    }

    /// A display curve: scan a linear copy. The linear copy of the previous capture is reused as
    /// `prev` (the card cache compares the two) when it came from the same grab, and as the
    /// current copy too when the raw capture did not change (no lookup pass at all).
    #[allow(clippy::too_many_arguments)]
    fn scan_linear(&mut self, reg: &mut Region, raw: &Bgra, frame: u64, raw_prev: Option<(Bgra, u64)>, views: Vec<View>, info: &WowInfo, seen: &mut HashSet<(u8, u8, i32, i32)>) -> RegionOut {
        let prev_lin = std::mem::take(&mut reg.lin);
        let mut cur_lin = std::mem::take(&mut reg.lin_spare);
        let key = |b: &Bgra| [b.origin_x, b.origin_y, b.width as i32, b.height as i32];
        let new_key = (frame, key(raw));
        let prev_valid = match (&raw_prev, reg.lin_key) {
            (Some((p, n)), Some((k, r))) => *n == k && r == key(p) && prev_lin.len() == (p.width * p.height * 4) as usize,
            _ => false,
        };
        let unchanged = prev_valid && raw_prev.as_ref().is_some_and(|(p, _)| same_raw(p, raw));
        fn view<'p>(px: &'p [u8], b: &Bgra) -> Bgra<'p> {
            Bgra { px, width: b.width, height: b.height, stride: b.width * 4, origin_x: b.origin_x, origin_y: b.origin_y }
        }
        if unchanged {
            let n = raw_prev.as_ref().map_or(0, |(_, n)| *n);
            let out = self.scan_img(reg, &view(&prev_lin, raw), frame, Some((view(&prev_lin, raw), n)), views, info, seen);
            reg.lin_key = Some(new_key);
            reg.lin = prev_lin;
            reg.lin_spare = cur_lin;
            return out;
        }
        let out = {
            let cur = self.tone.apply(raw, &mut cur_lin);
            let prev = match &raw_prev {
                Some((p, n)) if prev_valid => Some((view(&prev_lin, p), *n)),
                _ => None,
            };
            self.scan_img(reg, &cur, frame, prev, views, info, seen)
        };
        reg.lin_key = Some(new_key);
        reg.lin = cur_lin;
        reg.lin_spare = prev_lin;
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn scan_img(&mut self, reg: &mut Region, img: &Bgra, frame: u64, prev: Option<(Bgra, u64)>, mut views: Vec<View>, info: &WowInfo, seen: &mut HashSet<(u8, u8, i32, i32)>) -> RegionOut {
        let t_scan = Instant::now();
        let mut hits: Vec<TagHit> = Vec::new();
        {
            // a linearized image reads 0 / 255 for the strip's black and white
            let white_black = if self.tone.is_identity() { self.scene.white_black() } else { None };
            let mut areas: Vec<Rect> = views
                .iter()
                .filter_map(|v| capture::clip(Rect { x: v.x.into(), y: v.y.into(), w: v.w.into(), h: v.h.into() }, info.client_w, info.client_h))
                .collect();
            // discovery (no VIEW): also the middle of the client, for the presence card
            let discovery = views.len() == 1 && views[0].id == 0 && areas.len() == 1;
            if discovery {
                areas = discovery_areas(areas[0]);
            }
            let st = reg.tagscan.scan(img, &areas, white_black, &mut hits);
            if discovery {
                let first = areas[0];
                hits.retain(|h| keep_discovery_hit(h, first));
            }
            self.perf.area_rows += u64::from(st.area_rows);
            self.perf.scanned_rows += u64::from(st.scanned_rows);
            self.perf.reused_bands += u64::from(st.reused_bands);
            self.perf.bands += u64::from(st.bands);
            // H7: a tag in two overlapping views (or regions) keeps its first reading
            dedup_hits(&mut hits, seen);
            // Discovery has no VIEW: estimate the font and row pitch from the tag heights.
            if views.len() == 1 && views[0].font_px == 0 && !hits.is_empty() {
                let mut hs: Vec<i32> = hits.iter().map(|h| h.y_bot - h.y_top + 1).collect();
                hs.sort_unstable();
                let h = hs[hs.len() / 2].clamp(4, 200);
                views[0].font_px = h as u8;
                views[0].row_px = ((h as f32) * 1.15).round().clamp(1.0, 255.0) as u8;
            }
        }
        self.minute.scans += 1;
        self.minute.hits += hits.len() as u64;
        let t_cards = Instant::now();
        self.perf.stages.add(Stage::Scan, t_cards - t_scan);
        let (cards, bgs) = if hits.is_empty() {
            (Vec::new(), Vec::new())
        } else {
            let scene = &self.scene;
            let plate_expected = |slot: u8| scene.plate_expected(slot);
            let rows_expected = |slot: u8| scene.rows_expected(slot);
            let prev = prev.as_ref().map(|(p, n)| (p, *n));
            let (cards, kept) = reg.cards.build_with_rows(img, frame, prev, &hits, &views, &plate_expected, &rows_expected, scene.sess_bit());
            self.perf.cards_kept += u64::from(kept);
            let view_of = |c: &Card| views.iter().find(|v| v.id == c.view_id).copied();
            // The chat background around each card (native look), from the same capture: the
            // same pixels give the same samples, so kept cards keep theirs. Samples that moved by
            // a few levels only keep the previous capture's value (a world moving behind a
            // translucent chat would republish the scene at every capture).
            let discovery = scene.discovery();
            if kept && reg.prev_bgs.len() == cards.len() {
                (cards, reg.prev_bgs.clone())
            } else {
                let mut bgs = Vec::with_capacity(cards.len());
                for c in &cards {
                    let view = view_of(c)
                        .map(|v| Rect { x: v.x.into(), y: v.y.into(), w: v.w.into(), h: v.h.into() })
                        .unwrap_or(Rect { x: img.origin_x, y: img.origin_y, w: img.width as i32, h: img.height as i32 });
                    let mut bg = backdrop::sample_card(img, c, view, discovery);
                    if !self.tone.is_identity() {
                        bg.map_colors(|rgb| self.tone.display(rgb));
                    }
                    if let Some(p) = reg.prev_cards.iter().zip(reg.prev_bgs.iter()).find(|(p, _)| p.slot == c.slot && p.view_id == c.view_id) {
                        if p.0.bands.len() == c.bands.len() {
                            backdrop::settle(&mut bg, p.1);
                        }
                    }
                    bgs.push(bg);
                }
                (cards, bgs)
            }
        };
        self.perf.stages.add(Stage::Cards, t_cards.elapsed());
        reg.prev_cards = cards.clone();
        reg.prev_bgs = bgs.clone();
        RegionOut { cards, bgs, hits }
    }

    /// Tag scan and cards of every region of one capture (`base`: the grabber index of the first
    /// scan region), then the `tags` diag record.
    fn scan_all(&mut self, g: &dyn Grabber, base: usize, plan: Vec<(Rect, Vec<View>)>, info: &WowInfo) {
        let views_all: Vec<View> = plan.iter().flat_map(|(_, v)| v.iter().copied()).collect();
        if self.regions.len() != plan.len() {
            self.regions.resize_with(plan.len(), Region::default);
        }
        let mut seen: HashSet<(u8, u8, i32, i32)> = HashSet::new();
        let (mut cards, mut bgs, mut hits) = (Vec::new(), Vec::new(), Vec::new());
        for (ri, (_, views)) in plan.into_iter().enumerate() {
            // Nothing left of the area after clipping: an empty image finds nothing.
            let empty = Bgra { px: &[], width: 0, height: 0, stride: 0, origin_x: 0, origin_y: 0 };
            let img = g.image(base + ri).unwrap_or(empty);
            let out = self.scan_region(ri, &img, g.frame_number(), g.previous(base + ri), views, info, &mut seen);
            cards.extend(out.cards);
            bgs.extend(out.bgs);
            hits.extend(out.hits);
        }
        let t_diag = Instant::now();
        let now = self.now_ms();
        self.sticky.apply(&mut cards, now);
        // the overlay paints over the game's curved output: back to display colours
        if !self.tone.is_identity() {
            for c in cards.iter_mut() {
                c.plate_rgb = self.tone.display(c.plate_rgb);
            }
        }
        if self.ctx.diag.enabled() && (!hits.is_empty() || self.last_hit_count != 0) {
            // Written every capture while tags are on screen: formatted directly (numbers only).
            for v in &views_all {
                let mut list = String::new();
                for h in hits.iter().filter(|h| {
                    let (vx, vy) = (i32::from(v.x), i32::from(v.y));
                    h.x0 >= vx && h.x0 < vx + i32::from(v.w) && h.y_top >= vy && h.y_top < vy + i32::from(v.h)
                }) {
                    // a band's row is numbered 0..5 in its card, a tag carries it modulo 4
                    let plate = cards.iter().any(|c| {
                        c.slot == h.slot
                            && c.plate
                            && c.bands.iter().any(|b| b.row % TAG_ROWS == h.row % TAG_ROWS && b.tag_rect.y <= h.y_bot && b.tag_rect.y + b.tag_rect.h >= h.y_top)
                    });
                    if !list.is_empty() {
                        list.push(',');
                    }
                    list.push_str(&format!(
                        r#"{{"slot":{},"row":{},"x":{},"y":{},"w":{},"h":{},"alpha":{},"block":{},"plate":{}}}"#,
                        h.slot,
                        h.row,
                        h.x0,
                        h.y_top,
                        h.x1 - h.x0,
                        h.y_bot - h.y_top + 1,
                        json_f64(round3(h.alpha)),
                        json_f64(round3(h.block_px)),
                        plate
                    ));
                }
                self.ctx.diag.emit_raw(&format!(r#""ev":"tags","view":{},"hits":[{}]"#, v.id, list));
            }
        }
        self.perf.stages.add(Stage::Diag, t_diag.elapsed());
        self.last_hit_count = hits.len();
        self.minute.cards += cards.len() as u64;
        self.cards = cards;
        self.card_bgs = bgs;
        self.presence_seen = self.scene.discovery() && hits.iter().any(|h| h.slot == PRESENCE_SLOT);
        self.hits = hits;
    }

    /// Publishes the scene when it changed; the `cards` diag record also follows status changes.
    fn publish(&mut self, mut s: Scene) {
        s.gdi = self.method() == Some(Method::Gdi);
        let fp = s.fingerprint();
        let scene_changed = self.last_fp != Some(fp);
        if self.ctx.diag.enabled() {
            let shared = &self.ctx.shared;
            // (engine status, clipboard outcome of a reply)
            let states: Vec<(&'static str, Option<&'static str>)> = s
                .views
                .iter()
                .flat_map(|v| v.cards.iter())
                .map(|sc| match &sc.kind {
                    CardKind::Request { key, reply, .. } => (shared.status_name(key), if *reply { shared.clip_name(key) } else { None }),
                    k => (k.name(), None),
                })
                .collect();
            if scene_changed || states != self.last_states {
                let list: Vec<Value> =
                    s.views.iter().flat_map(|v| v.cards.iter()).zip(&states).map(|(sc, (st, clip))| card_json_clip(sc, st, *clip)).collect();
                self.ctx.diag.emit(json!({"ev": "cards", "hidden": s.hidden.map(|h| h.name()), "fg": s.foreground, "cards": list}));
                self.last_states = states;
            }
        }
        if !scene_changed {
            return;
        }
        self.last_fp = Some(fp);
        self.visible = s.any_visible();
        self.ctx.shared.publish_scene(s);
    }

    fn minute_report(&mut self) {
        if self.minute_start.elapsed() < Duration::from_secs(60) {
            return;
        }
        self.minute_start = Instant::now();
        let m = std::mem::take(&mut self.minute);
        let log = &self.ctx.log;
        log.app(Level::Info, AppEvent::CaptureMinute { method: self.method().map(|m| m.name()).unwrap_or("none"), ticks: m.ticks, fresh: m.fresh, stale: m.stale, errors: m.errors });
        let margin_avg = if m.ok > 0 { (m.gap_sum / m.ok as f64).round() as u32 } else { 0 };
        log.app(Level::Info, AppEvent::DecodeMinute { ok: m.ok, rejected: m.rejected, absent: m.absent, torn: m.torn, margin_avg, messages: m.messages });
        log.app(Level::Info, AppEvent::TagMinute { scans: m.scans, hits: m.hits, cards: m.cards, covered: m.covered });
        let r = self.reasm.stats();
        log.app(Level::Debug, AppEvent::MessageCounters { frames: r.frames, chunks: r.chunks, messages: r.messages, bad_crc: r.bad_crc, parse_errors: r.parse_errors, expired: r.expired, missed: r.missed });
    }

    /// One capture tick; returns the delay before the next one.
    fn step(&mut self) -> Duration {
        self.tick += 1;
        self.minute.ticks += 1;
        self.minute_report();
        self.perf_report();
        self.perf.ticks += 1;
        let t_win = Instant::now();
        let now = self.now_ms();
        if let Some(h) = self.hwnd {
            if !wow::is_window(h) {
                self.lost();
            }
        }
        if self.hwnd.is_none() {
            if self.last_find.is_none_or(|t| t.elapsed() >= WOW_LOOKUP_EVERY) {
                self.last_find = Some(Instant::now());
                self.hwnd = self.finder.find();
                match self.hwnd.and_then(wow::describe) {
                    Some(i) => {
                        self.ctx.log.app(Level::Info, AppEvent::WowFound { w: i.client_w, h: i.client_h, dpi: i.dpi, foreground: i.foreground });
                        if self.ctx.diag.enabled() {
                            self.ctx.diag.emit(json!({"ev": "wow", "found": true, "w": i.client_w, "h": i.client_h, "x": i.origin_x, "y": i.origin_y}));
                        }
                        self.set_present(true);
                    }
                    None => {
                        self.hwnd = None;
                        self.set_present(false);
                    }
                }
            }
            if self.hwnd.is_none() {
                self.perf.stages.add(Stage::Window, t_win.elapsed());
                let s = self.scene.update(&TickInput { now_ms: now, wow: None, fresh: false }, Vec::new(), &[], &mut |_| false);
                self.publish(s);
                // Nothing to do until the next look-up.
                let next = self.last_find.map_or(Duration::ZERO, |t| WOW_LOOKUP_EVERY.saturating_sub(t.elapsed()));
                return next.max(Duration::from_millis(50));
            }
        }
        let frame_cache = &mut self.frame_cache;
        let Some(info) = self.hwnd.and_then(|h| wow::describe_cached(h, frame_cache)) else {
            self.lost();
            return Duration::from_millis(250);
        };
        self.perf.stages.add(Stage::Window, t_win.elapsed());
        self.perf.saw_wow = true;
        let fg = !self.ctx.assume_bg && (self.ctx.assume_fg || info.foreground);
        let geom = WowGeom { hwnd: info.hwnd, origin_x: info.origin_x, origin_y: info.origin_y, client_w: info.client_w, client_h: info.client_h };
        let minimized = info.minimized || info.client_w <= 0 || info.client_h <= 0;
        if minimized != self.minimized {
            self.minimized = minimized;
            self.ctx.log.app(Level::Info, AppEvent::WowMinimized { minimized });
        }
        if minimized {
            self.perf.saw_min = true;
            let input = TickInput { now_ms: now, wow: Some(WowTick { geom, minimized: true, foreground: fg }), fresh: false };
            let s = self.scene.update(&input, Vec::new(), &[], &mut |_| false);
            self.publish(s);
            return Duration::from_millis(500);
        }
        let t_open = Instant::now();
        let had = self.grabber.is_some();
        let ready = self.ensure_grabber(&info);
        if !had && self.grabber.is_some() {
            self.perf.stages.add(Stage::Open, t_open.elapsed());
        }
        if !ready {
            return Duration::from_millis(500);
        }

        // Regions: the strip and the tag scan regions (one per VIEW rectangle). The strip is read
        // in the background too: the conversation window keeps sending TRANSLATE and CANCEL while
        // the game is not in front (followed whispers, slots given back), and a message missed
        // there would leave a window bubble "Not received", or paint another request's text on a
        // reused slot. It is 8 client rows, so it costs little.
        let mut regions: Vec<Rect> = vec![Rect { x: 0, y: 0, w: info.client_w, h: 8 }];
        const STRIP_REGION: usize = 0;
        let plan = self.scan_plan(&info);
        let scan_index = plan.as_ref().map(|p| {
            let base = regions.len();
            regions.extend(p.iter().map(|(r, _)| *r));
            base
        });
        // the VIEW the regions were planned with (the strip decoded below may bring another one)
        let planned_views: Option<Vec<View>> = plan.as_ref().map(|_| self.scene.views().to_vec());
        // The grabber is taken out while its regions are read (they borrow it).
        let Some(mut g) = self.grabber.take() else { return Duration::from_millis(500) };
        self.taken_method = Some(g.method());
        let status = if regions.is_empty() {
            g.skip();
            GrabStatus::NoNewFrame
        } else {
            g.grab(&info, &regions)
        };
        let fresh = status == GrabStatus::Fresh;
        let t = g.timing();
        self.perf.stages.add(Stage::Acquire, t.acquire);
        if fresh {
            self.perf.stages.add(Stage::Copy, t.copy);
            self.perf.stages.add(Stage::Wait, t.wait);
            self.perf.stages.add(Stage::Read, t.read);
            self.perf.fresh += 1;
            self.perf.pixels += (0..regions.len()).filter_map(|i| g.image(i)).map(|v| u64::from(v.width) * u64::from(v.height)).sum::<u64>();
        }
        match status {
            GrabStatus::Fresh => self.minute.fresh += 1,
            GrabStatus::NoNewFrame => self.minute.stale += 1,
            GrabStatus::Failed => self.minute.errors += 1,
        }
        if fresh {
            self.last_fresh = Instant::now();
            self.wgc_recreated = false;
        }
        let mut view_skip = false;
        if fresh {
            if let Some(img) = g.image(STRIP_REGION) {
                let t = Instant::now();
                self.decode_strip(&img);
                self.perf.stages.add(Stage::Strip, t.elapsed());
            }
            // A VIEW decoded in this tick: the regions were planned with the previous one, so a
            // window's new rows (shown with the VIEW that covers them) would be read against the
            // chat view under them for one tick. This capture's scan is skipped; the next one
            // plans with the new VIEW (the previous cards stay meanwhile).
            view_skip = scan_index.is_some() && planned_views.as_ref().is_some_and(|v| v.as_slice() != self.scene.views());
            if let (Some(base), Some(plan), false) = (scan_index, plan, view_skip) {
                self.perf.scans += 1;
                self.last_scan = Some(Instant::now());
                self.scan_all(g.as_ref(), base, plan, &info);
            }
        }
        if self.ctx.diag.enabled() {
            let t = Instant::now();
            // Written every capture, after the decode: "scan" is true only when the tag scan
            // really ran on this capture (a capture whose VIEW changed skips it, "viewSkip").
            // Formatted directly (numbers and fixed names only).
            let method = self.method().map_or("null".to_string(), |m| format!("\"{}\"", m.name()));
            self.ctx.diag.emit_raw(&format!(
                r#""ev":"capture","fresh":{},"method":{},"fg":{},"strip":true,"scan":{},"viewSkip":{}"#,
                fresh,
                method,
                fg,
                scan_index.is_some() && !view_skip,
                view_skip
            ));
            self.perf.stages.add(Stage::Diag, t.elapsed());
        }
        g.release();
        self.grabber = Some(g);
        self.taken_method = None;
        let t_scene = Instant::now();

        // Covering test, at most 10 Hz per card. In front the overlay is topmost: any cover drops
        // the card. In the background it sits right above the game window (D12), so the windows
        // above the game hide what they cover: only a full cover drops it.
        let target = info.hwnd;
        let (ox, oy) = (info.origin_x, info.origin_y);
        let skip_cover = self.ctx.assume_uncovered;
        let rect_hidden = move |b: Rect| -> bool {
            if fg {
                wow::rect_covered(target, ox + b.x, oy + b.y, b.w, b.h)
            } else {
                wow::rect_fully_covered(target, ox + b.x, oy + b.y, b.w, b.h)
            }
        };
        let found: Vec<FoundCard> =
            self.cards.iter().enumerate().map(|(i, c)| FoundCard { card: c.clone(), bg: self.card_bgs.get(i).cloned().unwrap_or_default() }).collect();
        let cache = &mut self.cover_cache;
        let mut covered_count = 0u64;
        let mut covered = |c: &Card| -> bool {
            if skip_cover {
                return false;
            }
            let key = (c.view_id, c.slot, fg);
            if let Some((t, v)) = cache.get(&key) {
                if t.elapsed() < Duration::from_millis(100) {
                    covered_count += u64::from(*v);
                    return *v;
                }
            }
            let v = overlay::card_bounds(c).is_some_and(rect_hidden);
            cache.insert(key, (Instant::now(), v));
            covered_count += u64::from(v);
            v
        };
        let input = TickInput { now_ms: now, wow: Some(WowTick { geom, minimized: false, foreground: fg }), fresh };
        let hits = std::mem::take(&mut self.hits);
        let s = self.scene.update_found(&input, found, &hits, &mut covered);
        self.hits = hits;
        if self.cover_cache.len() > 256 {
            self.cover_cache.clear();
        }
        self.minute.covered += covered_count;
        let t_pub = self.perf.stages.lap(Stage::Scene, t_scene);
        self.publish(s);
        self.perf.stages.add(Stage::Publish, t_pub.elapsed());
        self.perf.saw_visible |= self.visible;

        // WGC delivering nothing for 3 s in front: recreate once, then fall back to GDI (auto).
        if fg && self.method() == Some(Method::Wgc) && self.last_fresh.elapsed() > Duration::from_secs(3) {
            if !self.wgc_recreated {
                self.wgc_recreated = true;
                // A new device too: the old one may be the reason no frame comes.
                self.drop_grabber(false);
                self.ctx.log.app(Level::Warn, AppEvent::CaptureRecreated { method: "wgc" });
            } else if self.ctx.choice == CaptureChoice::Auto {
                self.use_gdi = true;
                self.drop_grabber(false);
                self.wgc_device = None;
                self.ctx.log.app(Level::Warn, AppEvent::CaptureSwitched { from: "wgc", to: "gdi", reason: "stale" });
                self.ctx.shared.push_event(UiEvent::Balloon(BalloonKind::Fullscreen));
            }
            self.last_fresh = Instant::now();
        }

        let ms = self.period_ms(fg);
        // WGC delivers frames no faster than about the tick rate: a frame nobody reads still costs
        // CPU in this process. 3/4 of the period keeps a fresh frame ready at every tick.
        if let Some(g) = self.grabber.as_mut() {
            g.set_min_interval(((ms * 3 / 4) as u32).max(8));
        }
        Duration::from_millis(ms)
    }
}

fn capture_thread(ctx: CaptureCtx) {
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }
    let stop = ctx.stop.clone();
    let mut p = Pipeline::new(ctx);
    let mut next = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        let period = p.step();
        next += period;
        let now = Instant::now();
        if next < now {
            next = now;
        }
        // Sleep in short slices so that exit is quick.
        while !stop.load(Ordering::Relaxed) {
            let now = Instant::now();
            if now >= next {
                break;
            }
            std::thread::sleep((next - now).min(Duration::from_millis(50)));
        }
    }
    p.lost();
}

// ---------------------------------------------------------------------------------------------
// UI thread

struct Probe {
    fg_before: isize,
    center: (i32, i32),
    rect: [i32; 4],
    samples: u32,
    overlay_hits: u32,
    fg_changed: bool,
    shown_seen: bool,
}

struct Ui {
    settings: Settings,
    log: Arc<Logger>,
    diag: Arc<DiagSink>,
    shared: Arc<Shared>,
    main: HWND,
    engine: Option<EngineHandle>,
    overlay: OverlayManager,
    toaster: Toaster,
    tray: Option<Tray>,
    results: HashMap<JobKey, Status>,
    order: VecDeque<JobKey>,
    scene: Scene,
    ui_lang: Lang,
    exclude: Option<bool>,
    anim_on: bool,
    toast_on: bool,
    start: Instant,
    error_balloon_at: Option<Instant>,
    notinstalled_warned: bool,
    probe: Option<Probe>,
    log_dir: std::path::PathBuf,
    /// A new scene, new statuses or an animation step wait to be drawn (one render for a burst).
    redraw: bool,
    /// The conversation window's replies (memory only) and the clipboard they use.
    replies: Replies,
    clip_os: Box<dyn ClipOs>,
    /// `--no-clipboard` (implied by `--no-overlay`): the clipboard is never touched.
    clipboard_off: bool,
    /// The game window counts as in front whatever Windows says (test flags).
    assume_fg: bool,
    /// The z-order mode last reported (diag `z` record).
    z_reported: Option<bool>,
    settings_path: Option<std::path::PathBuf>,
    /// The fallback badges' hover windows and tooltip.
    badges: Badges,
    /// Why the engine translated a request locally instead of with the remote service (the
    /// badge's tooltip). Memory only, pruned with `results`.
    fallbacks: HashMap<JobKey, Fallback>,
    /// The API keys (the key file, DPAPI); empty when remote services are not allowed.
    keys: Keys,
    /// The remote service the engine uses now (None: local only).
    remote_active: Option<Provider>,
    /// A normal run with the real translator (`Cli::remote_allowed`).
    remote_allowed: bool,
    /// The last engine state (the tray tooltip is rebuilt when the service changes).
    engine_state: EngineState,
    app_icon: Option<HICON>,
}

impl Ui {
    fn lang(&self) -> Lang {
        self.scene.lang_target.as_deref().map(i18n::for_target).unwrap_or(self.ui_lang)
    }

    /// The reply switch: the tray item and not `--no-clipboard`.
    fn reply_copy_on(&self) -> bool {
        self.settings.reply_clipboard && !self.clipboard_off
    }

    /// D12: topmost while the game window is in front, else right above it. Read live (not from
    /// the scene, which may be a few ms old) on every render and on every foreground change.
    fn update_z(&mut self) {
        let Some(w) = self.scene.wow else { return };
        if w.hwnd == 0 || self.probe.is_some() {
            return;
        }
        let fg = self.assume_fg || wow::foreground_root() == w.hwnd;
        let z = if fg { ZMode::Topmost } else { ZMode::Above(w.hwnd) };
        let moved = self.overlay.set_z(z);
        self.badges.set_z(z);
        if self.z_reported != Some(fg) || moved {
            self.z_reported = Some(fg);
            self.log.app(Level::Debug, AppEvent::ZOrder { topmost: fg, moved });
            if self.diag.enabled() {
                self.diag.emit(json!({"ev": "z", "mode": if fg { "topmost" } else { "above" }, "moved": moved}));
            }
        }
    }

    /// The foreground window changed (WinEvent hook): the z-order follows before the capture
    /// thread notices; a minimized game window hides the overlay at once.
    fn on_foreground(&mut self) {
        let Some(w) = self.scene.wow else { return };
        if w.hwnd == 0 || self.probe.is_some() {
            return;
        }
        if wow::is_minimized(w.hwnd) {
            self.overlay.hide_all();
            self.badges.hide_all();
            return;
        }
        self.update_z();
    }

    /// Logs and records one clipboard step of a reply (numbers and fixed names only).
    fn clip_event(&self, key: JobKey, ev: ClipEvent) {
        let cut = matches!(ev, ClipEvent::Finish(ClipOutcome::Copied { cut: true }));
        let excluded = self.replies.get(&key).is_some_and(|b| b.clip.history_excluded);
        self.log.app(Level::Info, AppEvent::Clipboard { what: ev.what(), outcome: ev.outcome(), req: key.req_id, cut, excluded });
        if self.diag.enabled() {
            self.diag.emit(json!({"ev": "clip", "req": key.req_id, "what": ev.what(), "outcome": ev.outcome(), "cut": cut, "excluded": excluded}));
        }
        if let ClipEvent::Finish(o) = ev {
            self.shared.set_clip_name(key, o.name());
        }
    }

    fn apply_affinity(&mut self) {
        let want = self.scene.gdi || self.settings.hide_from_recordings;
        if self.exclude != Some(want) {
            self.exclude = Some(want);
            let ok = self.overlay.set_exclude(want);
            self.toaster.set_exclude(want);
            self.badges.set_exclude(want);
            self.log.app(Level::Info, AppEvent::OverlayAffinity { exclude: want, ok });
        }
    }

    fn render(&mut self) {
        let t0 = Instant::now();
        self.update_z();
        let pulse = overlay::pulse(self.start.elapsed().as_secs_f64());
        let fallbacks = &self.fallbacks;
        let reports = self.overlay.render_all(&self.scene, &self.results, self.replies.books(), &|k| fallbacks.contains_key(k), pulse, self.ui_lang);
        self.sync_badges();
        self.shared.renders.fetch_add(1, Ordering::Relaxed);
        self.shared.render_ns.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
        if self.diag.enabled() {
            for r in reports {
                self.diag.emit(json!({"ev": "overlay", "view": r.id, "shown": r.shown, "rect": r.rect, "window": r.window,
                                      "px": r.px, "textPx": r.stats.text_px, "tagPx": r.stats.tag_px, "fillPx": r.stats.fill_px}));
            }
        }
        let want_anim = self.overlay.animating;
        if want_anim != self.anim_on {
            self.anim_on = want_anim;
            unsafe {
                if want_anim {
                    SetTimer(Some(self.main), TIMER_ANIM, 100, None);
                } else {
                    let _ = KillTimer(Some(self.main), TIMER_ANIM);
                }
            }
        }
    }

    /// The badges' hover windows follow the last render: one over each badge drawn, none while
    /// the cards are hidden (and none in the overlay probe). A change is recorded (`badge` diag
    /// record, client coordinates).
    fn sync_badges(&mut self) {
        let origin = match (self.scene.hidden, self.scene.wow) {
            (None, Some(w)) if self.probe.is_none() => Some((w.origin_x, w.origin_y)),
            _ => None,
        };
        let changed = match origin {
            Some(o) => self.badges.sync(self.overlay.badges(), o, self.overlay.z_mode()),
            None => {
                let had = !self.badges.placed().is_empty();
                self.badges.hide_all();
                had
            }
        };
        if changed && self.diag.enabled() {
            let spots: Vec<Value> = self.overlay.badges().iter().map(|s| json!({"req": s.key.req_id, "session": s.key.session, "view": s.view, "rect": s.rect})).collect();
            self.diag.emit(json!({"ev": "badge", "n": spots.len(), "spots": spots, "windows": self.badges.hwnds().len()}));
        }
    }

    /// The mouse entered (or left) a badge's hover window: its tooltip, in the player's language.
    fn on_badge(&mut self, entered: bool, hwnd: isize) {
        if !entered {
            let had = self.badges.tip_for().is_some();
            self.badges.leave(hwnd);
            if had && self.diag.enabled() {
                self.diag.emit(json!({"ev": "tip", "shown": false}));
            }
            return;
        }
        let Some((key, _)) = self.badges.key_of(hwnd) else { return };
        let Some(f) = self.fallbacks.get(&key) else { return };
        let text = remote_ui::tip_text(f, self.lang(), self.settings.translation.daily_char_cap);
        let shown = self.badges.show_tip(hwnd, &text);
        self.log.app(Level::Debug, AppEvent::FallbackTip { req: key.req_id, kind: f.kind.code(), shown });
        if self.diag.enabled() {
            self.diag.emit(json!({"ev": "tip", "req": key.req_id, "kind": f.kind.code(), "status": f.status, "shown": shown}));
        }
    }

    fn on_scene(&mut self) {
        let s = self.shared.inbox.lock().ok().and_then(|mut i| i.scene.take());
        if let Some(s) = s {
            self.scene = s;
            self.apply_affinity();
            self.redraw = true;
        }
    }

    fn on_results(&mut self) {
        // A reply's start is pushed before its job is submitted: handle the pending events first,
        // so a result is never seen before the start of its reply.
        self.on_events();
        let updates = self.shared.inbox.lock().map(|mut i| std::mem::take(&mut i.updates)).unwrap_or_default();
        if updates.is_empty() {
            return;
        }
        for u in updates {
            if !self.results.contains_key(&u.key) {
                self.order.push_back(u.key);
            }
            // a reply's final result writes the clipboard (when allowed) before it is drawn
            if let Some(ev) = self.replies.on_status(self.clip_os.as_mut(), &u.key, &u.status) {
                self.clip_event(u.key, ev);
                if matches!(ev, ClipEvent::Finish(ClipOutcome::Copied { .. })) && self.settings.reply_sound {
                    message_beep_ok();
                }
            }
            // every update of a job meant for the remote service and translated locally carries
            // the reason; an update without one (a new route for the same request) clears it
            match u.fallback {
                Some(f) => {
                    self.fallbacks.insert(u.key, f);
                }
                None => {
                    self.fallbacks.remove(&u.key);
                }
            }
            self.results.insert(u.key, u.status);
        }
        while self.order.len() > 512 {
            if let Some(k) = self.order.pop_front() {
                self.results.remove(&k);
                self.fallbacks.remove(&k);
            }
        }
        self.redraw = true;
    }

    fn on_state(&mut self) {
        let s = self.shared.inbox.lock().ok().and_then(|mut i| i.state.take());
        let Some(s) = s else { return };
        self.log.app(Level::Info, AppEvent::Engine { state: s });
        if self.diag.enabled() {
            self.diag.emit(json!({"ev": "state", "state": state_name(s)}));
        }
        self.engine_state = s;
        self.update_tray_tip();
        match s {
            // with a remote service in use the local model is only the fallback: no install
            // prompt (the tray tooltip still says "local: translator files not found")
            EngineState::NotInstalled if !self.notinstalled_warned && self.remote_active.is_none() => {
                self.notinstalled_warned = true;
                self.balloon(BalloonKind::NotInstalled);
            }
            EngineState::Error if self.error_balloon_at.is_none_or(|t| t.elapsed() > Duration::from_secs(600)) => {
                self.error_balloon_at = Some(Instant::now());
                self.balloon(BalloonKind::ServerFailed);
            }
            _ => {}
        }
    }

    /// "Translate Companion 0.7.0 - <state>", or with a remote service
    /// "Translate Companion 0.7.0 - DeepL (local: <state>)".
    fn update_tray_tip(&mut self) {
        let l = self.ui_lang;
        let tip = tray_tip(l, self.remote_active, self.engine_state);
        if let Some(t) = self.tray.as_mut() {
            t.set_tip(&tip);
        }
    }

    /// Gives the engine the saved remote service (consent and key checked), or None: local only.
    /// Logged with the provider id and the reason it cannot be used, never the key or the URL.
    fn apply_remote(&mut self) {
        let choice = Choice::of(&self.settings.translation);
        let (cfg, err) = if self.remote_allowed { remote_ui::engine_remote(&self.settings.translation, &self.keys) } else { (None, None) };
        self.remote_active = cfg.as_ref().map(|c| c.provider);
        self.log.app(
            Level::Info,
            AppEvent::RemoteChoice {
                provider: choice.id(),
                active: cfg.is_some(),
                error: err.map(|e| e.code()),
                daily_cap: self.settings.translation.daily_char_cap,
                allowed: self.remote_allowed,
            },
        );
        if let Some(e) = &self.engine {
            e.set_remote(cfg);
        }
        self.update_tray_tip();
    }

    /// The tray's "Translation engine...": opens the settings window (in the Windows UI
    /// language), or brings the open one to the front.
    fn open_engine_settings(&mut self) {
        let keys_path = settings::default_path().map(|p| remote_ui::keys_path(&p)).unwrap_or_else(|| settings::data_dir().join("keys.bin"));
        // the key file as it is now (another instance may not write it, but a fresh read costs
        // little and shows what Save would keep). A file that could not be read, or a run that
        // never touches it (tests, the echo translator), gives a read-only window: Save and
        // Remove must never write a key set that was not read over the real file.
        let keys = if self.remote_allowed {
            match remote_ui::load_keys(settings::default_path().as_deref()) {
                (k, None) => Some(k),
                (_, Some(_)) => None,
            }
        } else {
            None
        };
        let backend = KeyFileBackend::new(keys_path, keys, settings::data_dir());
        let ok = settings_window::open(OpenArgs {
            lang: self.ui_lang,
            saved: self.settings.translation.clone(),
            backend: Box::new(backend),
            notify: self.main.0 as isize,
            test_target: remote_ui::test_target(self.ui_lang, self.scene.lang_target.as_deref()),
            show: true,
            icon: self.app_icon,
        });
        if !ok {
            self.log.app(Level::Warn, AppEvent::EngineSettings { action: "open", provider: Choice::of(&self.settings.translation).id(), ok: false });
        }
    }

    /// What the settings window did: logged, and a Save or a removed key goes to settings.json
    /// (the UI thread's copy: tray switches changed meanwhile are kept) and to the engine.
    fn on_settings_events(&mut self) {
        for ev in settings_window::take_events() {
            match ev {
                SettingsEvent::Log { action, provider, ok } => {
                    self.log.app(if ok { Level::Info } else { Level::Warn }, AppEvent::EngineSettings { action, provider, ok });
                }
                SettingsEvent::KeysChanged(keys) => {
                    self.keys = if self.remote_allowed { keys } else { Keys::default() };
                    self.apply_remote();
                }
                SettingsEvent::Saved { translation, keys } => {
                    self.settings.translation = translation;
                    if self.remote_allowed {
                        self.keys = keys;
                    }
                    let saved = self.settings_path.as_ref().map(|p| self.settings.save(p).is_ok());
                    if saved == Some(false) {
                        self.log.app(Level::Warn, AppEvent::EngineSettings { action: "write_settings", provider: Choice::of(&self.settings.translation).id(), ok: false });
                    }
                    self.apply_remote();
                    self.redraw = true;
                }
                SettingsEvent::Closed => {}
            }
        }
    }

    fn balloon(&mut self, kind: BalloonKind) {
        let shown = match self.tray.as_mut() {
            Some(t) => {
                t.balloon(i18n::tr(self.ui_lang, "balloon.title"), i18n::tr(self.ui_lang, kind.key()));
                true
            }
            None => false,
        };
        self.log.app(Level::Info, AppEvent::Balloon { kind: kind.name() });
        if self.diag.enabled() {
            self.diag.emit(json!({"ev": "balloon", "kind": kind.name(), "shown": shown}));
        }
    }

    fn on_events(&mut self) {
        let events = self.shared.inbox.lock().map(|mut i| std::mem::take(&mut i.events)).unwrap_or_default();
        for e in events {
            match e {
                UiEvent::ReplyStart { key, copy, text } => {
                    let on = self.reply_copy_on();
                    if let Some(ev) = self.replies.start(self.clip_os.as_mut(), key, copy, on, text) {
                        self.clip_event(key, ev);
                    }
                    self.redraw = true;
                }
                UiEvent::ReplyEnd { key } => {
                    if let Some(ev) = self.replies.end(self.clip_os.as_mut(), &key) {
                        self.clip_event(key, ev);
                    }
                    self.redraw = true;
                }
                UiEvent::Balloon(kind) => self.balloon(kind),
                UiEvent::Toast { kind, method, hz } => {
                    let lang = self.lang();
                    let (name, text, fields) = match kind {
                        ToastKind::BridgeOk { mode, margin } => {
                            let text = i18n::tr_fmt(
                                lang,
                                "toast.bridge_ok",
                                &[("mode", mode_name(mode)), ("m", &margin.to_string()), ("capture", method.label()), ("hz", &hz.to_string())],
                            );
                            ("bridge_ok", text, json!({"mode": mode_name(mode), "margin": margin, "capture": method.name(), "hz": hz}))
                        }
                        ToastKind::AddonOld => ("addon_old", i18n::tr(lang, "toast.addon_old").to_string(), json!({})),
                        ToastKind::HelperOld => ("helper_old", i18n::tr(lang, "toast.helper_old").to_string(), json!({})),
                    };
                    let shown = match self.scene.wow {
                        Some(w) => self.toaster.show(&text, w.origin_x + 16, w.origin_y + 24, (w.client_w - 32).max(100)),
                        None => false,
                    };
                    if shown && !self.toast_on {
                        self.toast_on = true;
                        unsafe {
                            SetTimer(Some(self.main), TIMER_TOAST, 250, None);
                        }
                    }
                    self.log.app(Level::Info, AppEvent::Toast { kind: name, shown });
                    if self.diag.enabled() {
                        self.diag.emit(json!({"ev": "toast", "kind": name, "shown": shown, "info": fields}));
                    }
                }
            }
        }
    }

    fn on_timer(&mut self, id: usize) {
        match id {
            TIMER_EXIT => unsafe { PostQuitMessage(0) },
            TIMER_ANIM => self.redraw = true,
            TIMER_TOAST => {
                if !self.toaster.tick() {
                    self.toast_on = false;
                    unsafe {
                        let _ = KillTimer(Some(self.main), TIMER_TOAST);
                    }
                }
            }
            TIMER_PROBE => self.probe_sample(),
            _ => {}
        }
    }

    fn on_tray(&mut self, wparam: usize, lparam: isize) {
        let m = (lparam & 0xFFFF) as u32;
        let Some(v4) = self.tray.as_ref().map(|t| t.version4()) else { return };
        // Version 4: the right-click is WM_CONTEXTMENU only (WM_RBUTTONUP comes too and would
        // open the menu twice), anchored at the point in wParam. Legacy icon: WM_RBUTTONUP.
        let anchor = if v4 {
            if m != WM_CONTEXTMENU {
                return;
            }
            Some(((wparam & 0xFFFF) as u16 as i16 as i32, ((wparam >> 16) & 0xFFFF) as u16 as i16 as i32))
        } else {
            if m != WM_RBUTTONUP {
                return;
            }
            None
        };
        // While the menu is open the UI thread sits in its modal loop, which dispatches (and so
        // drops) the scene, result, state and event messages: the cards would freeze over the
        // game. Hide them (and the badges' hover windows), and drain the inbox again once the
        // menu is closed.
        self.overlay.hide_all();
        self.badges.hide_all();
        let l = self.ui_lang;
        let items = [
            MenuItem { cmd: MenuCmd::EngineSettings, label: i18n::tr(l, "tray.engine_settings"), checked: None, separator_before: false },
            MenuItem { cmd: MenuCmd::Unload, label: i18n::tr(l, "tray.unload"), checked: None, separator_before: true },
            MenuItem { cmd: MenuCmd::Updates, label: i18n::tr(l, "tray.updates"), checked: None, separator_before: false },
            MenuItem { cmd: MenuCmd::OpenLogs, label: i18n::tr(l, "tray.open_logs"), checked: None, separator_before: false },
            MenuItem { cmd: MenuCmd::CopyReplies, label: i18n::tr(l, "tray.copy_replies"), checked: Some(self.settings.reply_clipboard), separator_before: true },
            MenuItem { cmd: MenuCmd::ReplySound, label: i18n::tr(l, "tray.reply_sound"), checked: Some(self.settings.reply_sound), separator_before: false },
            MenuItem { cmd: MenuCmd::Exit, label: i18n::tr(l, "tray.exit"), checked: None, separator_before: true },
        ];
        let chosen = match self.tray.as_ref() {
            Some(t) => t.menu(&items, anchor),
            None => None,
        };
        for m in [WM_APP_SCENE, WM_APP_RESULT, WM_APP_STATE, WM_APP_EVENT, WM_APP_SETTINGS] {
            self.shared.post(m);
        }
        self.redraw = true;
        match chosen {
            // the user's own click: the window may take the focus
            Some(MenuCmd::EngineSettings) => self.open_engine_settings(),
            Some(MenuCmd::Unload) => {
                if let Some(e) = &self.engine {
                    e.unload_now();
                }
            }
            Some(MenuCmd::OpenLogs) => unsafe {
                let dir: Vec<u16> = self.log_dir.as_os_str().to_string_lossy().encode_utf16().chain(std::iter::once(0)).collect();
                let _ = ShellExecuteW(None, w!("open"), PCWSTR(dir.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
            },
            // the user's own click: the releases page in the default browser (nothing is
            // downloaded by the app)
            Some(MenuCmd::Updates) => unsafe {
                let url: Vec<u16> = crate::RELEASES_URL.encode_utf16().chain(std::iter::once(0)).collect();
                let _ = ShellExecuteW(None, w!("open"), PCWSTR(url.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
                self.log.app(Level::Info, AppEvent::TrayUpdates);
            },
            Some(MenuCmd::Exit) => unsafe { PostQuitMessage(0) },
            Some(MenuCmd::CopyReplies) => {
                self.settings.reply_clipboard = !self.settings.reply_clipboard;
                self.save_reply_switches();
            }
            Some(MenuCmd::ReplySound) => {
                self.settings.reply_sound = !self.settings.reply_sound;
                self.save_reply_switches();
            }
            None => {}
        }
    }

    /// The two reply switches changed in the tray menu: logged and saved to settings.json (they
    /// apply to the next reply request).
    fn save_reply_switches(&mut self) {
        self.log.app(Level::Info, AppEvent::ReplySwitches { copy: self.settings.reply_clipboard, sound: self.settings.reply_sound });
        if let Some(p) = &self.settings_path {
            let _ = self.settings.save(p);
        }
    }

    fn probe_sample(&mut self) {
        let hwnds = self.overlay.hwnds();
        let shown = self.overlay.any_shown();
        let Some(p) = self.probe.as_mut() else { return };
        let (cx, cy) = p.center;
        let raw = unsafe { WindowFromPoint(POINT { x: cx, y: cy }) }.0 as isize;
        let root = wow::root_at(cx, cy);
        if hwnds.contains(&raw) || hwnds.contains(&root) {
            p.overlay_hits += 1;
        }
        if unsafe { GetForegroundWindow() }.0 as isize != p.fg_before {
            p.fg_changed = true;
        }
        p.shown_seen |= shown;
        p.samples += 1;
    }

    /// Renders once when something changed and no other scene or status message is queued
    /// (coalesces a burst of updates into one render).
    fn render_if_due(&mut self) {
        if !self.redraw {
            return;
        }
        let mut peek = MSG::default();
        let more = unsafe { PeekMessageW(&mut peek, Some(self.main), WM_APP_SCENE, WM_APP_RESULT, PM_NOREMOVE) }.as_bool();
        if !more {
            self.redraw = false;
            self.render();
        }
    }

    fn message_loop(&mut self) -> i32 {
        let mut msg = MSG::default();
        loop {
            self.render_if_due();
            let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
            if r.0 == 0 || r.0 == -1 {
                break;
            }
            if msg.hwnd == self.main {
                match msg.message {
                    WM_APP_SCENE => {
                        self.on_scene();
                        continue;
                    }
                    WM_APP_RESULT => {
                        self.on_results();
                        continue;
                    }
                    WM_APP_STATE => {
                        self.on_state();
                        continue;
                    }
                    WM_APP_EVENT => {
                        self.on_events();
                        continue;
                    }
                    WM_TIMER => {
                        self.on_timer(msg.wParam.0);
                        continue;
                    }
                    WM_APP_TRAY => {
                        self.on_tray(msg.wParam.0, msg.lParam.0);
                        continue;
                    }
                    WM_APP_TRAY_READD => {
                        if let Some(t) = self.tray.as_mut() {
                            t.readd();
                        }
                        continue;
                    }
                    WM_APP_FOREGROUND => {
                        self.on_foreground();
                        continue;
                    }
                    WM_APP_BADGE => {
                        self.on_badge(msg.wParam.0 != 0, msg.lParam.0);
                        continue;
                    }
                    WM_APP_SETTINGS => {
                        // wParam 1: a modal loop of the settings window (moving it) ended; it
                        // dispatched (and so dropped) the messages that came meanwhile
                        if msg.wParam.0 == 1 {
                            for m in [WM_APP_SCENE, WM_APP_RESULT, WM_APP_STATE, WM_APP_EVENT] {
                                self.shared.post(m);
                            }
                        }
                        self.on_settings_events();
                        continue;
                    }
                    _ => {}
                }
            }
            // Tab, Enter and Esc in the settings window
            if settings_window::dialog_message(&msg) {
                continue;
            }
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        msg.wParam.0 as i32
    }
}

// ---------------------------------------------------------------------------------------------
// Process

static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

// Two user32 calls declared here instead of enabling the large `Win32_UI_Accessibility` and
// `Win32_System_Diagnostics_Debug` features of the windows crate for them.
#[link(name = "user32")]
extern "system" {
    fn SetWinEventHook(event_min: u32, event_max: u32, hmod: isize, proc_: WinEventProc, pid: u32, tid: u32, flags: u32) -> isize;
    fn UnhookWinEvent(hook: isize) -> i32;
    fn MessageBeep(kind: u32) -> i32;
}

#[link(name = "winmm")]
extern "system" {
    #[link_name = "PlaySoundW"]
    fn play_sound(sound: *const u16, module: isize, flags: u32) -> i32;
}
const SND_ASYNC: u32 = 0x0001;
const SND_NODEFAULT: u32 = 0x0002;
const SND_FILENAME: u32 = 0x0002_0000;
/// Asterisk: a sound most Windows schemes keep, used when the file below is missing.
const MB_ICONASTERISK_SOUND: u32 = 0x0000_0040;

type WinEventProc = unsafe extern "system" fn(hook: isize, event: u32, hwnd: isize, id_object: i32, id_child: i32, thread: u32, time: u32);

/// The main window, for the WinEvent callback.
static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);
const EVENT_SYSTEM_FOREGROUND_ID: u32 = 0x0003;
const EVENT_SYSTEM_MINIMIZEEND_ID: u32 = 0x0017;
const WINEVENT_OUTOFCONTEXT_FLAG: u32 = 0x0000;

/// Out-of-context WinEvent callback (runs on the UI thread, inside its message loop): posts the
/// change to the main window, which moves the overlay in the z-order (decision D12).
unsafe extern "system" fn on_win_event(_hook: isize, _event: u32, _hwnd: isize, _id_object: i32, _id_child: i32, _thread: u32, _time: u32) {
    let h = MAIN_HWND.load(Ordering::Relaxed);
    if h != 0 {
        unsafe {
            let _ = PostMessageW(Some(hwnd(h)), WM_APP_FOREGROUND, WPARAM(0), LPARAM(0));
        }
    }
}

/// Foreground and minimize changes of every window (no injection: out of context). 0 when the
/// hook could not be set (the capture thread's foreground check still follows, a tick later).
fn hook_foreground() -> [isize; 2] {
    unsafe {
        [
            SetWinEventHook(EVENT_SYSTEM_FOREGROUND_ID, EVENT_SYSTEM_FOREGROUND_ID, 0, on_win_event, 0, 0, WINEVENT_OUTOFCONTEXT_FLAG),
            // the end of a minimize (restored): EVENT_SYSTEM_MINIMIZESTART (0x16) and MINIMIZEEND
            SetWinEventHook(0x0016, EVENT_SYSTEM_MINIMIZEEND_ID, 0, on_win_event, 0, 0, WINEVENT_OUTOFCONTEXT_FLAG),
        ]
    }
}

fn unhook(hooks: [isize; 2]) {
    for h in hooks {
        if h != 0 {
            unsafe {
                UnhookWinEvent(h);
            }
        }
    }
}

/// The optional sound when a reply was copied (tray item, off by default). MessageBeep(MB_OK)
/// plays the scheme's "Default Beep", which many schemes leave silent (it was not heard in use on
/// 2026-10-20), so a stock Windows notification file is played first.
fn message_beep_ok() {
    let windir = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let path = std::path::Path::new(&windir).join("Media").join("Windows Notify Messaging.wav");
    let played = path.is_file() && {
        let w: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().chain(std::iter::once(0)).collect();
        unsafe { play_sound(w.as_ptr(), 0, SND_FILENAME | SND_ASYNC | SND_NODEFAULT) != 0 }
    };
    if !played {
        unsafe {
            MessageBeep(MB_ICONASTERISK_SOUND);
        }
    }
}

/// True when two captures of the same rectangle hold the same pixels.
fn same_raw(a: &Bgra, b: &Bgra) -> bool {
    if (a.origin_x, a.origin_y, a.width, a.height) != (b.origin_x, b.origin_y, b.width, b.height) {
        return false;
    }
    let row = a.width as usize * 4;
    (0..a.height as usize).all(|y| {
        let (sa, sb) = (y * a.stride as usize, y * b.stride as usize);
        a.px.get(sa..sa + row) == b.px.get(sb..sb + row)
    })
}

/// What a session end must stop: Windows may end the process as soon as WM_ENDSESSION returns,
/// so main_proc runs the shutdown from there, synchronously.
struct EndSession {
    stop: Arc<AtomicBool>,
    capture: Option<JoinHandle<()>>,
    engine: Option<Engine>,
    /// The status file writer: it writes "stopped" first.
    status: Option<StatusWriter>,
    log: Arc<Logger>,
    diag: Arc<DiagSink>,
    started: Instant,
}

static END_SESSION: Mutex<Option<EndSession>> = Mutex::new(None);

/// How long the exit waits for the capture thread (it checks its stop flag at least every 50 ms
/// between captures; a capture waits at most a few hundred ms for the GPU).
const CAPTURE_JOIN_MS: u64 = 3000;

/// Writes "stopped" into the status files, stops and joins the capture thread (never waiting more
/// than `CAPTURE_JOIN_MS`), then shuts the engine down (kills llama-server, joins its threads
/// within 2 s and deletes the key file).
fn stop_workers(es: EndSession) {
    // "stopped" first: a few small file writes, done before anything that could take longer
    if let Some(w) = es.status {
        w.stop();
    }
    es.stop.store(true, Ordering::Relaxed);
    if let Some(j) = es.capture {
        let t0 = Instant::now();
        while !j.is_finished() && t0.elapsed() < Duration::from_millis(CAPTURE_JOIN_MS) {
            std::thread::sleep(Duration::from_millis(10));
        }
        if j.is_finished() {
            let _ = j.join();
        }
    }
    if let Some(e) = es.engine {
        e.shutdown();
    }
}

/// WM_ENDSESSION with wParam TRUE: the session ends; clean up before returning.
fn end_session_now() {
    let es = END_SESSION.lock().ok().and_then(|mut g| g.take());
    if let Some(es) = es {
        let (log, diag, started) = (es.log.clone(), es.diag.clone(), es.started);
        stop_workers(es);
        log.app(Level::Info, AppEvent::Exit { code: 0, uptime_s: started.elapsed().as_secs() });
        diag.flush();
        log.shutdown();
    }
}

unsafe extern "system" fn main_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let tc = TASKBAR_CREATED.load(Ordering::Relaxed);
    if tc != 0 && msg == tc {
        unsafe {
            let _ = PostMessageW(Some(h), WM_APP_TRAY_READD, WPARAM(0), LPARAM(0));
        }
        return LRESULT(0);
    }
    match msg {
        // The shell sends notification-icon callbacks straight to the window procedure (they never
        // reach GetMessage), so re-post them: the message loop opens the menu (`on_tray`). Without
        // this a right-click on the icon did nothing (seen in use on 2026-10-20).
        WM_APP_TRAY => {
            unsafe {
                let _ = PostMessageW(Some(h), WM_APP_TRAY, wp, lp);
            }
            LRESULT(0)
        }
        // Like the tray's Exit: the message loop ends, then the workers stop (llama-server is
        // killed) and the log is flushed. `taskkill` without /F sends WM_CLOSE to one window
        // only; the overlay and toast windows forward it the same way (`overlay::window`).
        WM_CLOSE => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        WM_ENDSESSION => {
            // wParam FALSE: another application cancelled the shutdown or logoff; keep running
            if wp.0 != 0 {
                end_session_now();
                unsafe { PostQuitMessage(0) };
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(h, msg, wp, lp) },
    }
}

fn create_main_window() -> Option<HWND> {
    unsafe {
        TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Relaxed);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(main_proc),
            hInstance: hinstance(),
            lpszClassName: MAIN_CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        // A plain hidden window (never shown): message-only windows miss "TaskbarCreated".
        CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, MAIN_CLASS, w!("Translate Companion"), WS_POPUP, 0, 0, 0, 0, None, None, Some(hinstance()), None).ok()
    }
}

/// The single-instance mutex: `Local\TranslateProbeHelper`, or `Local\TranslateProbeHelper.NAME`
/// with the hidden test flag `--instance-name NAME` (a test Helper next to the user's own).
pub fn mutex_name(instance: Option<&str>) -> String {
    match instance {
        Some(n) => format!("Local\\TranslateProbeHelper.{n}"),
        None => "Local\\TranslateProbeHelper".to_string(),
    }
}

/// Takes the single-instance mutex; None when another instance holds it.
fn single_instance(instance: Option<&str>) -> Option<HANDLE> {
    let name: Vec<u16> = mutex_name(instance).encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let h = CreateMutexW(None, true, PCWSTR(name.as_ptr())).ok()?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(h);
            return None;
        }
        Some(h)
    }
}

/// Lets a GUI-subsystem exe print to the console it was started from (stdout redirected to a
/// file or a pipe works without this).
pub fn attach_console() {
    use windows::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
    use windows::Win32::System::Console::{AttachConsole, GetStdHandle, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE};
    unsafe {
        let have = GetStdHandle(STD_OUTPUT_HANDLE).is_ok_and(|h| !h.is_invalid());
        if have || AttachConsole(ATTACH_PARENT_PROCESS).is_err() {
            return;
        }
        if let Ok(h) = CreateFileW(w!("CONOUT$"), (GENERIC_READ | GENERIC_WRITE).0, FILE_SHARE_READ | FILE_SHARE_WRITE, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None) {
            let _ = SetStdHandle(STD_OUTPUT_HANDLE, h);
            let _ = SetStdHandle(STD_ERROR_HANDLE, h);
        }
    }
}

fn install_panic_hook(log: Arc<Logger>) {
    std::panic::set_hook(Box::new(move |info| {
        // Location only: a panic message is never written anywhere.
        let line = info.location().map(|l| l.line()).unwrap_or(0);
        log.app(Level::Error, AppEvent::Panic { line });
        log.flush();
        eprintln!("WoWTranslateCompanion: internal error (line {line})");
    }));
}

/// The synthetic scene of `--overlay-probe`: one 2-row test card in a 376 x 52 px area at the
/// bottom right of the primary monitor's work area (amendment A1: small, bottom-right).
fn probe_scene() -> (Scene, (i32, i32), [i32; 4]) {
    let wa = wow::primary_work_area();
    let (cw, ch) = (376, 68);
    let (ox, oy) = (wa.right - cw - 24, wa.bottom - ch - 24);
    let band = |row: u8, y: i32| RowBand {
        row,
        text_rect: Rect { x: 60, y, w: 308, h: 16 },
        mask: None,
        tag_rect: Rect { x: 8, y, w: 48, h: 16 },
    };
    let card = Card {
        slot: TEST_SLOT,
        view_id: 0,
        bands: vec![band(0, 20), band(1, 42)],
        gaps: vec![Rect { x: 60, y: 36, w: 308, h: 6 }],
        alpha: 1.0,
        plate: false,
        plate_rgb: [32, 40, 64],
        text_width_px: 308,
    };
    let view = SceneView {
        id: 0,
        kind: ViewKind::Chat,
        rect: Rect { x: 0, y: OVERLAY_TOP, w: cw, h: ch - OVERLAY_TOP },
        font_px: 14,
        cards: vec![SceneCard { card, kind: CardKind::Test, covered: false, block_px: 6.0, bg: CardBg::default() }],
    };
    let scene = Scene {
        wow: Some(WowGeom { hwnd: 0, origin_x: ox, origin_y: oy, client_w: cw, client_h: ch }),
        views: vec![view],
        hidden: None,
        lang_target: None,
        gdi: false,
        foreground: true,
    };
    let center = (ox + (8 + 368) / 2, oy + (20 + 58) / 2);
    (scene, center, [ox, oy + OVERLAY_TOP, cw, ch - OVERLAY_TOP])
}

/// Starts the status file writer: `--status-root` alone when given, else settings.json `wowRoot`,
/// else the Battle.net agent's product.db. Its rounds are logged as counts (never a path): at Info
/// when something changed, else at Debug.
fn start_status_writer(cli: &Cli, settings: &Settings, log: Arc<Logger>) -> Option<StatusWriter> {
    let wow_root = cli.status_root.clone().or_else(|| settings.wow_root_opt());
    let cfg = WriterConfig {
        product_db: if wow_root.is_some() { None } else { Some(crate::status_file::product_db_path()) },
        wow_root,
        version: VERSION.to_string(),
        build: ADDON_BUILD,
        heartbeat: crate::status_file::HEARTBEAT,
        check: crate::status_file::CHECK,
    };
    let report = Arc::new(move |r: crate::status_file::Report| {
        let level = if r.changed { Level::Info } else { Level::Debug };
        log.app(level, AppEvent::StatusFile { state: r.state.name(), roots: r.roots, targets: r.targets, written: r.written, failed: r.failed, skipped: r.skipped });
    });
    StatusWriter::start(cfg, report)
}

/// Runs the Helper; returns the process exit code.
pub fn run(cli: Cli) -> i32 {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    if cli.help || cli.version {
        attach_console();
        if cli.version {
            println!("{} {VERSION}", crate::PRODUCT_NAME);
        } else {
            print!("{USAGE}");
        }
        return 0;
    }
    let (settings, outcome, fixed) = match settings::default_path() {
        Some(p) => Settings::load(&p),
        None => (Settings::default(), LoadOutcome::Unreadable, Vec::new()),
    };
    if cli.selftest {
        attach_console();
        let cfg = engine_config(&cli, &settings, Arc::new(|_| {}));
        let v = tp_engine::selftest::run(&cfg);
        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
        return 0;
    }
    let probe_mode = cli.overlay_probe;
    if probe_mode {
        attach_console();
    }
    let mutex = if probe_mode {
        None
    } else {
        match single_instance(cli.instance_name.as_deref()) {
            Some(m) => Some(m),
            None => return 2,
        }
    };

    let log_dir = cli.log_dir.clone().unwrap_or_else(|| settings::data_dir().join("logs"));
    let log = Logger::start(&log_dir, if cli.diag { Level::Debug } else { Level::parse(&settings.log_level) });
    install_panic_hook(log.clone());
    let capture_choice = cli.capture.or_else(|| CaptureChoice::parse(&settings.capture)).unwrap_or(CaptureChoice::Auto);
    log.app(
        Level::Info,
        AppEvent::Start {
            version: VERSION,
            translator: cli.translator.name(),
            capture: capture_choice.name(),
            overlay: !cli.no_overlay,
            tray: !cli.no_tray,
            diag: cli.diag,
        },
    );
    log.app(Level::Info, AppEvent::Settings { outcome, fixed: fixed.len() as u32 });
    for f in &fixed {
        log.app(Level::Warn, AppEvent::SettingFixed { field: f });
    }
    let diag = cli.diag_json.as_deref().map(DiagSink::open).unwrap_or_else(DiagSink::none);
    let started = Instant::now();

    let Some(main) = create_main_window() else {
        log.app(Level::Error, AppEvent::Exit { code: 1, uptime_s: 0 });
        log.shutdown();
        return 1;
    };
    MAIN_HWND.store(main.0 as isize, Ordering::Relaxed);
    let shared = Arc::new(Shared::new(main.0 as isize));
    shared.ui_thread.store(perf::current_thread_id(), Ordering::Relaxed);

    // Engine: updates and states go to the UI thread; events to the log.
    let engine = if probe_mode {
        None
    } else {
        let l = log.clone();
        let sh = shared.clone();
        let log_fn: Arc<dyn Fn(EngineEvent) + Send + Sync> = Arc::new(move |ev: EngineEvent| {
            if let EngineEvent::VramGuard { switched_to_cpu: true, .. } = ev {
                sh.push_event(UiEvent::Balloon(BalloonKind::SwitchedCpu));
            }
            l.engine(&ev);
        });
        let cfg = engine_config(&cli, &settings, log_fn);
        let sh_u = shared.clone();
        let d = diag.clone();
        let on_update = Box::new(move |u: Update| {
            let name = overlay::status_name(&u.status);
            if d.enabled() {
                let ms = sh_u.submit_times.lock().ok().and_then(|t| t.get(&u.key).map(|s| s.elapsed().as_millis() as u64));
                // "fallback": the kind when the engine translated it locally after a remote
                // failure (never the service's message)
                let fb = u.fallback.as_ref().map(|f| f.kind.code());
                d.emit(json!({"ev": "engine", "req": u.key.req_id, "session": u.key.session, "status": name, "ms": ms, "fallback": fb}));
            }
            if let Ok(mut n) = sh_u.status_names.lock() {
                n.insert(u.key, name);
            }
            sh_u.push_update(u);
        });
        let sh_s = shared.clone();
        let on_state = Box::new(move |s: EngineState| sh_s.push_state(s));
        Some(Engine::start(cfg, on_update, on_state))
    };
    let handle = engine.as_ref().map(|e| e.handle());

    let ui_lang = i18n::ui_lang();
    let tray = if cli.no_tray || probe_mode {
        None
    } else {
        let tip = i18n::tr_fmt(ui_lang, "tray.tip", &[("version", VERSION), ("state", i18n::tr(ui_lang, "tray.state.unloaded"))]);
        let t = Tray::add(main.0 as isize, &tip);
        log.app(Level::Info, AppEvent::TrayIcon { ok: t.as_ref().is_some_and(|t| t.added()) });
        t
    };

    // The API keys of the remote services (DPAPI, next to settings.json): a normal run with the
    // real translator only; a test run never reads them (`Cli::remote_allowed`).
    let remote_allowed = cli.remote_allowed() && !probe_mode;
    let keys = if remote_allowed {
        let (k, err) = remote_ui::load_keys(settings::default_path().as_deref());
        if let Some(e) = err {
            log.app(Level::Warn, AppEvent::KeyFile { ok: false, error: e });
        }
        k
    } else {
        Keys::default()
    };

    let mut ui = Ui {
        badges: Badges::new(cli.no_overlay, main.0 as isize),
        fallbacks: HashMap::new(),
        keys,
        remote_active: None,
        remote_allowed,
        engine_state: EngineState::Unloaded,
        app_icon: crate::tray::make_icon(),
        overlay: OverlayManager::new(cli.no_overlay, cli.diag, settings.text_rgb(), settings.font_scale),
        toaster: Toaster::new(cli.no_overlay),
        settings: settings.clone(),
        log: log.clone(),
        diag: diag.clone(),
        shared: shared.clone(),
        main,
        engine: handle.clone(),
        tray,
        results: HashMap::new(),
        order: VecDeque::new(),
        scene: Scene::empty(Some(Hidden::NoWow)),
        ui_lang,
        exclude: None,
        anim_on: false,
        toast_on: false,
        start: Instant::now(),
        error_balloon_at: None,
        notinstalled_warned: false,
        probe: None,
        log_dir: log_dir.clone(),
        redraw: false,
        replies: Replies::new(),
        clip_os: Box::new(WinClip::new(main.0 as isize)),
        clipboard_off: cli.clipboard_off(),
        assume_fg: cli.foreground_assumed(),
        z_reported: None,
        // an unreadable settings file is left untouched (the tray switches then last this run)
        settings_path: if outcome == LoadOutcome::Unreadable { None } else { settings::default_path() },
    };
    if cli.clipboard_off() {
        log.app(Level::Info, AppEvent::ReplySwitches { copy: false, sound: settings.reply_sound });
    }
    // the saved remote service (or local only) for the engine's first job
    if !probe_mode {
        ui.apply_remote();
    }
    // D12: follow foreground changes at once (overlay windows only)
    let hooks = if cli.no_overlay || probe_mode { [0, 0] } else { hook_foreground() };

    // The toast window exists (hidden) from the start, so `taskkill` without /F always finds a
    // topmost window of ours first and the Helper exits like the tray's Exit.
    if !probe_mode {
        ui.toaster.prepare();
    }

    let stop = Arc::new(AtomicBool::new(false));
    let mut capture: Option<JoinHandle<()>> = None;
    if probe_mode {
        let fg_before = unsafe { GetForegroundWindow() }.0 as isize;
        let (scene, center, rect) = probe_scene();
        ui.probe = Some(Probe { fg_before, center, rect, samples: 0, overlay_hits: 0, fg_changed: false, shown_seen: false });
        shared.publish_scene(scene);
        unsafe {
            SetTimer(Some(main), TIMER_PROBE, 100, None);
        }
    } else {
        let ctx = CaptureCtx {
            wow_class: cli.wow_class.clone(),
            choice: capture_choice,
            hz_active: settings.capture_hz_active,
            assume_fg: cli.foreground_assumed(),
            assume_bg: cli.assume_background,
            assume_uncovered: cli.no_overlay,
            engine: handle.clone(),
            shared: shared.clone(),
            stop: stop.clone(),
            log: log.clone(),
            diag: diag.clone(),
        };
        capture = std::thread::Builder::new().name("tp-capture".into()).spawn(move || capture_thread(ctx)).ok();
    }

    // The probe never shows its window for more than 5 s (amendment A1).
    let exit_after = if probe_mode { Some(cli.exit_after.unwrap_or(5.0).min(5.0)) } else { cli.exit_after };
    if let Some(secs) = exit_after {
        unsafe {
            SetTimer(Some(main), TIMER_EXIT, (secs * 1000.0).round().max(1.0) as u32, None);
        }
    }

    // the status file: "running" now and every 60 s, "stopped" on a clean exit (never for a test
    // instance unless `--status-root` names its own folder, never for the overlay probe)
    let status = if probe_mode || !cli.status_file_on() { None } else { start_status_writer(&cli, &settings, log.clone()) };
    if let Ok(mut g) = END_SESSION.lock() {
        *g = Some(EndSession { stop: stop.clone(), capture: capture.take(), engine, status, log: log.clone(), diag: diag.clone(), started });
    }

    let mut code = ui.message_loop();

    // None: WM_ENDSESSION already stopped everything and closed the log
    let es = END_SESSION.lock().ok().and_then(|mut g| g.take());
    let ended = es.is_none();
    if let Some(es) = es {
        stop_workers(es);
    }
    unhook(hooks);
    // graceful exit: a reply's translation still in the clipboard (the Helper's own write) goes
    for key in ui.replies.books().keys().copied().collect::<Vec<_>>() {
        if let Some(ev) = ui.replies.end(ui.clip_os.as_mut(), &key) {
            ui.clip_event(key, ev);
        }
    }
    settings_window::close();
    ui.overlay.hide_all();
    ui.badges.hide_all();
    ui.toaster.hide();
    ui.tray = None;
    if let Some(i) = ui.app_icon.take() {
        unsafe {
            let _ = DestroyIcon(i);
        }
    }
    if let Some(p) = ui.probe.take() {
        let pass = p.samples > 0 && p.overlay_hits == 0 && !p.fg_changed && p.shown_seen;
        let summary = json!({"probe": "overlay", "samples": p.samples, "overlayHits": p.overlay_hits, "foregroundChanged": p.fg_changed,
                             "shown": p.shown_seen, "rect": p.rect, "pass": pass});
        println!("{summary}");
        if diag.enabled() {
            diag.emit(json!({"ev": "probe", "result": summary}));
        }
        code = if pass { 0 } else { 1 };
    }
    if !ended {
        log.app(Level::Info, AppEvent::Exit { code, uptime_s: started.elapsed().as_secs() });
        diag.flush();
        log.shutdown();
    }
    unsafe {
        let _ = DestroyWindow(main);
        if let Some(m) = mutex {
            let _ = CloseHandle(m);
        }
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::tests::card;

    #[test]
    fn the_tray_tooltip_names_the_remote_service() {
        let v = VERSION;
        assert_eq!(tray_tip(Lang::En, None, EngineState::ReadyCpu), format!("Translate Companion {v} - translator ready (CPU)"));
        assert_eq!(tray_tip(Lang::En, Some(Provider::DeepL), EngineState::Unloaded), format!("Translate Companion {v} - DeepL (local: ready, translator not loaded)"));
        assert_eq!(tray_tip(Lang::Fr, Some(Provider::Claude), EngineState::ReadyGpu), format!("Translate Companion {v} - Claude (local : traducteur prêt (carte graphique))"));
        // the notification area cuts tooltips at 127 characters
        for lang in [Lang::En, Lang::Fr] {
            for p in Provider::ALL {
                for s in [EngineState::NotInstalled, EngineState::Error, EngineState::ReadyGpu] {
                    assert!(tray_tip(lang, Some(p), s).encode_utf16().count() <= 127, "{lang:?} {p:?} {s:?}");
                }
            }
        }
    }

    #[test]
    fn mask_runs_cover_set_bits() {
        let m = Mask { rect: Rect { x: 10, y: 20, w: 5, h: 2 }, bits: vec![255, 255, 0, 255, 0, 0, 0, 0, 0, 255] };
        assert_eq!(mask_runs(&m), vec![[20, 10, 12], [20, 13, 14], [21, 14, 15]]);
    }

    #[test]
    fn card_record_has_no_text_and_the_design_fields() {
        let sc = SceneCard {
            card: card(3, 1, 500, 2, 0.75),
            kind: CardKind::Request { key: JobKey { session: 4, req_id: 5 }, rows: 2, target: "fr".into(), reply: false },
            covered: false,
            block_px: 5.0,
            bg: CardBg::default(),
        };
        let v = card_json(&sc, "done");
        for k in ["slot", "req", "view", "bands", "maskPx", "alpha", "state", "covered", "reply", "clip", "bg"] {
            assert!(v.get(k).is_some(), "{k}");
        }
        assert!(v.get("lane").is_none(), "no hint lanes any more (2026-10-19, R1)");
        assert_eq!(v["reply"], false);
        assert_eq!(v["bg"], json!([[null, null], [null, null]]));
        assert_eq!(v["req"], 5);
        assert_eq!(v["bands"][1], json!([60, 516, 200, 14]));
        assert_eq!(v["maskPx"], 2 * 210 * 14);
        assert_eq!(v["alpha"], 0.75);
        assert_eq!(v["masks"][0]["runs"][0], json!([500, 57, 267]));
        assert!(!v.to_string().contains("fr"), "no target text either");
    }

    #[test]
    fn message_records_carry_numbers_only() {
        use tp_bridge::msg::{ChatText, Translate};
        let m = Message::Translate(Translate {
            req_id: 9,
            slot: 2,
            rows: 3,
            no_cache: true,
            plate: true,
            reply: false,
            copy: false,
            target: "de".into(),
            text: ChatText::new("ZZSENTINELZZ".into()),
            pairs: vec![(ChatText::new("ZZSENTINELZZ".into()), ChatText::new("ZZSENTINELZZ".into()))],
        });
        let (j, ev) = msg_json(&m);
        assert_eq!(j["bytes"], 12);
        assert_eq!(j["pairs"], 1);
        assert!(!j.to_string().contains("ZZSENTINELZZ"));
        assert!(!format!("{ev:?}").contains("ZZSENTINELZZ"));
    }

    #[test]
    fn reply_card_record_carries_the_clipboard_outcome() {
        let bg = CardBg {
            bands: vec![crate::backdrop::BandBg { left: Some(crate::backdrop::Sample { rgb: [36, 30, 24], x: 5 }), right: None }; 3],
            edges: Vec::new(),
        };
        let sc = SceneCard {
            card: card(126, 200, 500, 3, 1.0),
            kind: CardKind::Request { key: JobKey { session: 4, req_id: 9 }, rows: 3, target: "de".into(), reply: true },
            covered: false,
            block_px: 5.0,
            bg,
        };
        let v = card_json_clip(&sc, "done", Some("off"));
        assert_eq!(v["reply"], true);
        assert_eq!(v["clip"], "off");
        assert_eq!(v["bg"][2], json!([[36, 30, 24], null]), "a band's samples, borrowed from its neighbours when missing");
    }

    #[test]
    fn one_capture_region_per_view_merged_only_when_cheap() {
        let v = |id: u8, x: u16, y: u16, w: u16, h: u16| View { id, x, y, w, h, font_px: 14, row_px: 16 };
        // two chat frames far apart: two regions (their union would be mostly empty)
        let r = plan_regions(&[v(1, 20, 700, 500, 300), v(3, 1300, 100, 500, 300)], 1920, 1080);
        assert_eq!(r.len(), 2);
        assert_eq!((r[0].0.x, r[0].0.y, r[0].0.w, r[0].0.h), (20, 700, 500, 300));
        // the conversation window over a chat frame: one region, the window's view first
        let r = plan_regions(&[v(1, 20, 600, 600, 400), v(200, 100, 650, 400, 300)], 1920, 1080);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].1.iter().map(|x| x.id).collect::<Vec<_>>(), vec![200, 1], "window first");
        assert_eq!((r[0].0.x, r[0].0.y, r[0].0.w, r[0].0.h), (20, 600, 600, 400));
        // side by side, touching: the union is their total area
        let r = plan_regions(&[v(1, 0, 600, 400, 300), v(2, 400, 600, 400, 300)], 1920, 1080);
        assert_eq!(r.len(), 1);
        // the window apart from the chat: two regions, the window's first
        let r = plan_regions(&[v(1, 20, 700, 500, 300), v(200, 1200, 100, 440, 540)], 1920, 1080);
        assert_eq!(r.iter().map(|g| g.1[0].id).collect::<Vec<_>>(), vec![200, 1]);
        // clipped to the client; a view outside it is dropped
        let r = plan_regions(&[v(1, 1800, 1000, 400, 300), v(2, 3000, 100, 100, 100)], 1920, 1080);
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].0.w, r[0].0.h), (120, 80));
        assert!(plan_regions(&[], 1920, 1080).is_empty());
    }

    #[test]
    fn a_tag_in_two_overlapping_views_is_kept_once() {
        let hit = |slot: u8, row: u8, x0: i32, y: i32| TagHit { slot, row, sess_bit: 0, x0, x1: x0 + 47, y_top: y, y_bot: y + 13, block_px: 5.5, pitch_px: 6.0, alpha: 1.0 };
        let mut seen = HashSet::new();
        let mut window = vec![hit(100, 0, 120, 700), hit(100, 1, 120, 716)];
        dedup_hits(&mut window, &mut seen);
        assert_eq!(window.len(), 2);
        // the chat view under the window reads the same tags again, plus one of its own
        let mut chat = vec![hit(100, 0, 120, 700), hit(100, 1, 120, 716), hit(3, 0, 30, 900)];
        dedup_hits(&mut chat, &mut seen);
        assert_eq!(chat.iter().map(|h| h.slot).collect::<Vec<_>>(), vec![3]);
        // within one region too (two overlapping areas scanned in one image)
        let mut same = vec![hit(5, 2, 40, 300), hit(5, 2, 40, 300)];
        dedup_hits(&mut same, &mut HashSet::new());
        assert_eq!(same.len(), 1);
    }

    #[test]
    fn the_strip_is_read_at_its_own_rate_in_the_background_too() {
        // review finding: TRANSLATE and CANCEL sent while the game is not in front were lost
        assert_eq!(capture_period_ms(false, true, true, 30), BUSY_PERIOD_MS, "background, strip active");
        assert_eq!(capture_period_ms(false, true, false, 30), BACKGROUND_PERIOD_MS, "background, idle");
        // an idle background capture still sees a 400 ms chunk twice
        const _: () = assert!(BACKGROUND_PERIOD_MS * 2 <= 400);
        assert_eq!(capture_period_ms(true, true, true, 30), 33);
        assert_eq!(capture_period_ms(true, false, true, 30), BUSY_PERIOD_MS);
        assert_eq!(capture_period_ms(true, false, false, 30), IDLE_PERIOD_MS);
    }

    #[test]
    fn discovery_also_searches_the_middle_of_the_client_for_the_presence_card() {
        let area = Rect { x: 0, y: 16, w: 1920, h: 1064 };
        let areas = discovery_areas(area);
        let r4 = |r: Rect| [r.x, r.y, r.w, r.h];
        assert_eq!(r4(areas[0]), r4(area), "the first area is the whole client, as before");
        // every area reaches the right edge (a plate is never cut) and they chain without a hole
        for w in areas.windows(2) {
            assert_eq!(w[1].x + w[1].w, 1920);
            assert_eq!((w[1].y, w[1].h), (16, 1064));
            assert_eq!(w[1].x, search_end(w[0]) - DISCOVERY_OVERLAP);
        }
        let reach = search_end(*areas.last().unwrap());
        assert!(reach >= (1920.0 * PRESENCE_REACH) as i32, "searched up to {reach}");
        assert!(areas.len() <= 4, "{areas:?}");
        // a popup centred on a 1920 px client: its tag (about 40% of the width) is searched
        assert!(areas.iter().any(|a| (a.x..search_end(*a)).contains(&770)));
        // other screens: a small client is one area, an ultrawide one still reaches 65%
        assert_eq!(discovery_areas(Rect { x: 0, y: 16, w: 300, h: 200 }).len(), 1);
        let wide = discovery_areas(Rect { x: 0, y: 16, w: 3440, h: 1424 });
        assert!(search_end(*wide.last().unwrap()) >= 2236);
        // only the presence card is kept from the extra areas: chat cards are found as before
        let hit = |slot: u8, x0: i32| TagHit { slot, row: 0, sess_bit: 0, x0, x1: x0 + 47, y_top: 500, y_bot: 513, block_px: 5.5, pitch_px: 6.0, alpha: 1.0 };
        assert!(keep_discovery_hit(&hit(3, 100), area) && keep_discovery_hit(&hit(PRESENCE_SLOT, 100), area));
        assert!(keep_discovery_hit(&hit(PRESENCE_SLOT, 900), area));
        assert!(!keep_discovery_hit(&hit(3, 900), area) && !keep_discovery_hit(&hit(126, 900), area));
    }

    #[test]
    fn mutex_names() {
        assert_eq!(mutex_name(None), "Local\\TranslateProbeHelper");
        assert_eq!(mutex_name(Some("exit-test")), "Local\\TranslateProbeHelper.exit-test");
    }

    #[test]
    fn probe_scene_respects_a1() {
        let (scene, center, rect) = probe_scene();
        let wa = wow::primary_work_area();
        assert!(rect[2] <= 400 && rect[3] <= 300, "{rect:?}");
        assert!(rect[0] + rect[2] <= wa.right && rect[1] + rect[3] <= wa.bottom);
        assert!(rect[0] > (wa.left + wa.right) / 2 && rect[1] > (wa.top + wa.bottom) / 2, "bottom-right");
        assert!(center.0 > rect[0] && center.0 < rect[0] + rect[2] && center.1 > rect[1] && center.1 < rect[1] + rect[3]);
        assert_eq!(scene.card_count(), 1);
    }
}
