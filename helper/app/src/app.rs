//! The Helper process (design D.3): main/UI thread, capture thread and engine wiring.
//!
//! - UI thread: hidden window `TPHelperMain`, tray icon, overlay windows, toast. It handles
//!   `WM_APP_SCENE` (newest scene), `WM_APP_RESULT` (engine updates), `WM_APP_STATE` and
//!   `WM_APP_EVENT` (toasts, balloons), and renders only on change.
//! - Capture thread: finds the game window, grabs the strip and the chat areas, decodes the
//!   bridge, feeds the engine, finds the cards and publishes a scene when it changed.

use std::collections::{HashMap, VecDeque};
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
use crate::capture::gdi::GdiGrabber;
use crate::capture::wgc::{self, WgcGrabber};
use crate::capture::{self, GrabStatus, Grabber, Method};
use crate::cli::{Backend, CaptureChoice, Cli, TranslatorChoice, USAGE};
use crate::i18n::{self, Lang};
use crate::log::{AppEvent, DiagSink, Level, Logger};
use crate::overlay::{self, window::hinstance, OverlayManager};
use crate::perf::{self, Stage, Stages, ThreadClock};
use crate::tagscan::{CardCache, TagScanner};
use crate::scene::*;
use crate::settings::{self, LoadOutcome, Settings};
use crate::toast::Toaster;
use crate::tray::{MenuCmd, Tray, WM_APP_TRAY};
use crate::wow::{self, hwnd, WowFinder, WowInfo};

pub const WM_APP_SCENE: u32 = WM_APP + 1;
pub const WM_APP_RESULT: u32 = WM_APP + 2;
pub const WM_APP_STATE: u32 = WM_APP + 3;
pub const WM_APP_EVENT: u32 = WM_APP + 4;
const WM_APP_TRAY_READD: u32 = WM_APP + 5;

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
/// Capture period with the game not in front (tags only).
pub const BACKGROUND_PERIOD_MS: u64 = 200;
/// The strip counts as active this long after it was last seen (any non-absent read).
pub const STRIP_RECENT_MS: u64 = 2000;
/// Tag scans while nothing is known (no request this session, no tag in the last scan, no strip):
/// the discovery cadence of the design (2 Hz).
pub const QUIET_SCAN_MS: u64 = 500;
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Events for the UI thread (no text).
pub enum UiEvent {
    Toast { kind: ToastKind, method: Method, hz: u32 },
    Balloon(BalloonKind),
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

/// One card of the `cards` diag event. `state` is the engine status name (or the card kind).
pub fn card_json(sc: &SceneCard, state: &str) -> Value {
    let c = &sc.card;
    let req = match &sc.kind {
        CardKind::Request { key, .. } => json!(key.req_id),
        _ => Value::Null,
    };
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
    })
}

/// The `msg` diag record and log event of one bridge message (numbers only).
pub fn msg_json(m: &Message) -> (Value, AppEvent) {
    match m {
        Message::Translate(t) => (
            json!({"ev": "msg", "kind": "translate", "req": t.req_id, "slot": t.slot, "rows": t.rows, "bytes": t.text.len(),
                   "pairs": t.pairs.len(), "noCache": t.no_cache, "plate": t.plate}),
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
    }
}

// ---------------------------------------------------------------------------------------------
// Capture thread

pub struct CaptureCtx {
    pub wow_class: Option<String>,
    pub choice: CaptureChoice,
    pub hz_active: u32,
    pub assume_fg: bool,
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
    tick: u64,
    last_fresh: Instant,
    last_fp: Option<u64>,
    last_states: Vec<&'static str>,
    last_hit_count: usize,
    visible: bool,
    minimized: bool,
    cover_cache: HashMap<(u8, u8), (Instant, bool)>,
    minute: Minute,
    minute_start: Instant,
    perf: PerfWindow,
    /// Last strip read that was not `Absent` (the strip is on screen, possibly torn).
    strip_seen: Option<Instant>,
    /// Last tag scan.
    last_scan: Option<Instant>,
    tagscan: TagScanner,
    card_cache: CardCache,
    /// Design step 7: each card's text width is the widest of the last 2 s, so a short-lived
    /// misread (a cover, a fading plate end) does not re-wrap the text.
    sticky: StickyWidth,
    /// Display curve measured on the strip (sticky like the strip calibration); when it is not
    /// linear, the scan area is linearized before the tag scan and the cards (`tone`).
    tone: Tone,
    /// Linearized current and spare scan-area buffers, and the grab number and rectangle of the
    /// capture `lin` holds (it is the previous capture's linear copy on the next tick).
    lin: Vec<u8>,
    lin_spare: Vec<u8>,
    lin_key: Option<(u64, [i32; 4])>,
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
            tagscan: TagScanner::new(),
            card_cache: CardCache::new(),
            sticky: StickyWidth::new(),
            tone: Tone::identity(),
            lin: Vec::new(),
            lin_spare: Vec::new(),
            lin_key: None,
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
        if !fg {
            BACKGROUND_PERIOD_MS
        } else if self.visible {
            u64::from(1000 / self.ctx.hz_active.clamp(20, 60))
        } else if self.strip_recent() {
            BUSY_PERIOD_MS
        } else {
            IDLE_PERIOD_MS
        }
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
            }
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
        self.tagscan.reset();
        self.card_cache.reset();
        self.sticky = StickyWidth::new();
        self.tone = Tone::identity();
        self.lin_key = None;
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

    /// Area to grab for the tag scan and the views to build cards against.
    fn scan_plan(&self, info: &WowInfo) -> Option<(Rect, Vec<View>)> {
        let (cw, ch) = (info.client_w, info.client_h);
        if !self.scan_due() {
            return None;
        }
        if !self.scene.discovery() {
            let views = self.scene.views().to_vec();
            let mut area: Option<Rect> = None;
            for v in &views {
                if let Some(r) = capture::clip(Rect { x: v.x.into(), y: v.y.into(), w: v.w.into(), h: v.h.into() }, cw, ch) {
                    area = Some(area.map_or(r, |a| capture::union(a, r)));
                }
            }
            return area.map(|a| (a, views));
        }
        // Discovery runs at 2 Hz whatever the tick rate.
        let discovery_due = self.last_scan.is_none_or(|t| t.elapsed() >= Duration::from_millis(QUIET_SCAN_MS));
        if !discovery_due || ch <= OVERLAY_TOP {
            return None;
        }
        // Discovery: the whole width below row 16, because a card's plate extends to the right of
        // its tag; `tags::scan` itself searches only the left 35% of the area.
        let area = Rect { x: 0, y: OVERLAY_TOP, w: cw, h: ch - OVERLAY_TOP };
        let view = View { id: 0, x: 0, y: OVERLAY_TOP as u16, w: cw.clamp(0, 65535) as u16, h: (ch - OVERLAY_TOP).clamp(0, 65535) as u16, font_px: 0, row_px: 0 };
        Some((area, vec![view]))
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
                    self.tagscan.reset();
                    self.card_cache.reset();
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
                    self.tagscan.reset();
                    self.card_cache.reset();
                    self.lin_key = None;
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
                if f.addon_build != ADDON_BUILD {
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

    /// Tag scan and cards of one capture: `frame` is its grab number, `prev` the previous capture
    /// of the same rectangle (for keeping unchanged cards).
    fn scan(&mut self, raw: &Bgra, frame: u64, raw_prev: Option<(Bgra, u64)>, views: Vec<View>, info: &WowInfo) {
        if self.tone.is_identity() {
            self.lin_key = None;
            self.scan_img(raw, frame, raw_prev, views, info);
            return;
        }
        // A display curve: scan a linear copy. The linear copy of the previous capture is reused
        // as `prev` (the card cache compares the two) when it came from the same grab, and as the
        // current copy too when the raw capture did not change (no lookup pass at all).
        let prev_lin = std::mem::take(&mut self.lin);
        let mut cur_lin = std::mem::take(&mut self.lin_spare);
        let key = |b: &Bgra| [b.origin_x, b.origin_y, b.width as i32, b.height as i32];
        let new_key = (frame, key(raw));
        let prev_valid = match (&raw_prev, self.lin_key) {
            (Some((p, n)), Some((k, r))) => *n == k && r == key(p) && prev_lin.len() == (p.width * p.height * 4) as usize,
            _ => false,
        };
        let unchanged = prev_valid && raw_prev.as_ref().is_some_and(|(p, _)| same_raw(p, raw));
        fn view<'p>(px: &'p [u8], b: &Bgra) -> Bgra<'p> {
            Bgra { px, width: b.width, height: b.height, stride: b.width * 4, origin_x: b.origin_x, origin_y: b.origin_y }
        }
        if unchanged {
            let n = raw_prev.as_ref().map_or(0, |(_, n)| *n);
            self.scan_img(&view(&prev_lin, raw), frame, Some((view(&prev_lin, raw), n)), views, info);
            self.lin_key = Some(new_key);
            self.lin = prev_lin;
            self.lin_spare = cur_lin;
            return;
        }
        {
            let cur = self.tone.apply(raw, &mut cur_lin);
            let prev = match &raw_prev {
                Some((p, n)) if prev_valid => Some((view(&prev_lin, p), *n)),
                _ => None,
            };
            self.scan_img(&cur, frame, prev, views, info);
        }
        self.lin_key = Some(new_key);
        self.lin = cur_lin;
        self.lin_spare = prev_lin;
    }

    fn scan_img(&mut self, img: &Bgra, frame: u64, prev: Option<(Bgra, u64)>, mut views: Vec<View>, info: &WowInfo) {
        let t_scan = Instant::now();
        let mut hits: Vec<TagHit> = Vec::new();
        {
            // a linearized image reads 0 / 255 for the strip's black and white
            let white_black = if self.tone.is_identity() { self.scene.white_black() } else { None };
            let areas: Vec<Rect> = views
                .iter()
                .filter_map(|v| capture::clip(Rect { x: v.x.into(), y: v.y.into(), w: v.w.into(), h: v.h.into() }, info.client_w, info.client_h))
                .collect();
            let st = self.tagscan.scan(img, &areas, white_black, &mut hits);
            self.perf.area_rows += u64::from(st.area_rows);
            self.perf.scanned_rows += u64::from(st.scanned_rows);
            self.perf.reused_bands += u64::from(st.reused_bands);
            self.perf.bands += u64::from(st.bands);
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
            let (mut cards, kept) = self.card_cache.build_with_rows(img, frame, prev, &hits, &views, &plate_expected, &rows_expected, scene.sess_bit());
            self.perf.cards_kept += u64::from(kept);
            // The chat background around each card (native look), from the same capture: the
            // same pixels give the same samples, so kept cards keep theirs. Samples that moved by
            // a few levels only keep the previous capture's value (a world moving behind a
            // translucent chat would republish the scene at every capture).
            let discovery = scene.discovery();
            let bgs: Vec<CardBg> = if kept && self.card_bgs.len() == cards.len() {
                self.card_bgs.clone()
            } else {
                cards
                    .iter()
                    .map(|c| {
                        let view = views
                            .iter()
                            .find(|v| v.id == c.view_id)
                            .map(|v| Rect { x: v.x.into(), y: v.y.into(), w: v.w.into(), h: v.h.into() })
                            .unwrap_or(Rect { x: img.origin_x, y: img.origin_y, w: img.width as i32, h: img.height as i32 });
                        let mut bg = backdrop::sample_card(img, c, view, discovery);
                        if !self.tone.is_identity() {
                            bg.map_colors(|rgb| self.tone.display(rgb));
                        }
                        let prev = self
                            .cards
                            .iter()
                            .zip(&self.card_bgs)
                            .find(|(p, _)| p.slot == c.slot && p.view_id == c.view_id && p.bands.len() == c.bands.len());
                        if let Some((_, prev_bg)) = prev {
                            backdrop::settle(&mut bg, prev_bg);
                        }
                        bg
                    })
                    .collect()
            };
            let now = self.now_ms();
            self.sticky.apply(&mut cards, now);
            // the overlay paints over the game's curved output: back to display colours
            if !self.tone.is_identity() {
                for c in cards.iter_mut() {
                    c.plate_rgb = self.tone.display(c.plate_rgb);
                }
            }
            (cards, bgs)
        };
        let t_diag = self.perf.stages.lap(Stage::Cards, t_cards);
        if self.ctx.diag.enabled() && (!hits.is_empty() || self.last_hit_count != 0) {
            // Written every capture while tags are on screen: formatted directly (numbers only).
            for v in &views {
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
        self.hits = hits;
    }

    /// Publishes the scene when it changed; the `cards` diag record also follows status changes.
    fn publish(&mut self, mut s: Scene) {
        s.gdi = self.method() == Some(Method::Gdi);
        let fp = s.fingerprint();
        let scene_changed = self.last_fp != Some(fp);
        if self.ctx.diag.enabled() {
            let shared = &self.ctx.shared;
            let states: Vec<&'static str> = s
                .views
                .iter()
                .flat_map(|v| v.cards.iter())
                .map(|sc| match &sc.kind {
                    CardKind::Request { key, .. } => shared.status_name(key),
                    k => k.name(),
                })
                .collect();
            if scene_changed || states != self.last_states {
                let list: Vec<Value> = s.views.iter().flat_map(|v| v.cards.iter()).zip(&states).map(|(sc, st)| card_json(sc, st)).collect();
                self.ctx.diag.emit(json!({"ev": "cards", "hidden": s.hidden.map(|h| h.name()), "cards": list}));
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
        let fg = self.ctx.assume_fg || info.foreground;
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

        // Regions: the strip (in front only) and the tag scan area.
        let mut regions: Vec<Rect> = Vec::new();
        let strip_index = fg.then(|| {
            regions.push(Rect { x: 0, y: 0, w: info.client_w, h: 8 });
            regions.len() - 1
        });
        let plan = self.scan_plan(&info);
        let scan_index = plan.as_ref().map(|(area, _)| {
            regions.push(*area);
            regions.len() - 1
        });
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
        if self.ctx.diag.enabled() {
            let t = Instant::now();
            // Written every capture: formatted directly (numbers and fixed names only).
            let method = self.method().map_or("null".to_string(), |m| format!("\"{}\"", m.name()));
            self.ctx.diag.emit_raw(&format!(
                r#""ev":"capture","fresh":{},"method":{},"fg":{},"strip":{},"scan":{}"#,
                fresh,
                method,
                fg,
                strip_index.is_some(),
                scan_index.is_some()
            ));
            self.perf.stages.add(Stage::Diag, t.elapsed());
        }
        if fresh {
            if let Some(img) = strip_index.and_then(|i| g.image(i)) {
                let t = Instant::now();
                self.decode_strip(&img);
                self.perf.stages.add(Stage::Strip, t.elapsed());
            }
            if let (Some(i), Some((_, views))) = (scan_index, plan) {
                self.perf.scans += 1;
                self.last_scan = Some(Instant::now());
                // Nothing left of the area after clipping: an empty image finds nothing.
                let empty = Bgra { px: &[], width: 0, height: 0, stride: 0, origin_x: 0, origin_y: 0 };
                let img = g.image(i).unwrap_or(empty);
                self.scan(&img, g.frame_number(), g.previous(i), views, &info);
            }
        }
        g.release();
        self.grabber = Some(g);
        self.taken_method = None;
        let t_scene = Instant::now();

        // Covering test, at most 10 Hz per card.
        let target = info.hwnd;
        let (ox, oy) = (info.origin_x, info.origin_y);
        let skip_cover = self.ctx.assume_uncovered;
        let cache = &mut self.cover_cache;
        let mut covered_count = 0u64;
        let mut covered = |c: &Card| -> bool {
            if skip_cover {
                return false;
            }
            let key = (c.view_id, c.slot);
            if let Some((t, v)) = cache.get(&key) {
                if t.elapsed() < Duration::from_millis(100) {
                    covered_count += u64::from(*v);
                    return *v;
                }
            }
            let v = overlay::card_bounds(c).is_some_and(|b| wow::rect_covered(target, ox + b.x, oy + b.y, b.w, b.h));
            cache.insert(key, (Instant::now(), v));
            covered_count += u64::from(v);
            v
        };
        let input = TickInput { now_ms: now, wow: Some(WowTick { geom, minimized: false, foreground: fg }), fresh };
        let cards: Vec<(Card, CardBg)> = self.cards.iter().cloned().zip(self.card_bgs.iter().cloned().chain(std::iter::repeat(CardBg::default()))).collect();
        let hits = std::mem::take(&mut self.hits);
        let s = self.scene.update_with_bg(&input, cards, &hits, &mut covered);
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
}

impl Ui {
    fn lang(&self) -> Lang {
        self.scene.lang_target.as_deref().map(i18n::for_target).unwrap_or(self.ui_lang)
    }

    fn apply_affinity(&mut self) {
        let want = self.scene.gdi || self.settings.hide_from_recordings;
        if self.exclude != Some(want) {
            self.exclude = Some(want);
            let ok = self.overlay.set_exclude(want);
            self.toaster.set_exclude(want);
            self.log.app(Level::Info, AppEvent::OverlayAffinity { exclude: want, ok });
        }
    }

    fn render(&mut self) {
        let t0 = Instant::now();
        let pulse = overlay::pulse(self.start.elapsed().as_secs_f64());
        let reports = self.overlay.render(&self.scene, &self.results, pulse, self.ui_lang);
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

    fn on_scene(&mut self) {
        let s = self.shared.inbox.lock().ok().and_then(|mut i| i.scene.take());
        if let Some(s) = s {
            self.scene = s;
            self.apply_affinity();
            self.redraw = true;
        }
    }

    fn on_results(&mut self) {
        let updates = self.shared.inbox.lock().map(|mut i| std::mem::take(&mut i.updates)).unwrap_or_default();
        if updates.is_empty() {
            return;
        }
        for u in updates {
            if !self.results.contains_key(&u.key) {
                self.order.push_back(u.key);
            }
            self.results.insert(u.key, u.status);
        }
        while self.order.len() > 512 {
            if let Some(k) = self.order.pop_front() {
                self.results.remove(&k);
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
        let tip = i18n::tr_fmt(self.ui_lang, "tray.tip", &[("state", i18n::tr(self.ui_lang, state_key(s)))]);
        if let Some(t) = self.tray.as_mut() {
            t.set_tip(&tip);
        }
        match s {
            EngineState::NotInstalled if !self.notinstalled_warned => {
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

    fn on_tray(&mut self, lparam: isize) {
        let m = (lparam & 0xFFFF) as u32;
        if m != WM_RBUTTONUP && m != WM_CONTEXTMENU {
            return;
        }
        if self.tray.is_none() {
            return;
        }
        // While the menu is open the UI thread sits in its modal loop, which dispatches (and so
        // drops) the scene, result, state and event messages: the cards would freeze over the
        // game. Hide them, and drain the inbox again once the menu is closed.
        self.overlay.hide_all();
        let l = self.ui_lang;
        let chosen = match self.tray.as_ref() {
            Some(t) => t.menu([i18n::tr(l, "tray.unload"), i18n::tr(l, "tray.open_logs"), i18n::tr(l, "tray.exit")]),
            None => None,
        };
        for m in [WM_APP_SCENE, WM_APP_RESULT, WM_APP_STATE, WM_APP_EVENT] {
            self.shared.post(m);
        }
        self.redraw = true;
        match chosen {
            Some(MenuCmd::Unload) => {
                if let Some(e) = &self.engine {
                    e.unload_now();
                }
            }
            Some(MenuCmd::OpenLogs) => unsafe {
                let dir: Vec<u16> = self.log_dir.as_os_str().to_string_lossy().encode_utf16().chain(std::iter::once(0)).collect();
                let _ = ShellExecuteW(None, w!("open"), PCWSTR(dir.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
            },
            Some(MenuCmd::Exit) => unsafe { PostQuitMessage(0) },
            None => {}
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
                        self.on_tray(msg.lParam.0);
                        continue;
                    }
                    WM_APP_TRAY_READD => {
                        if let Some(t) = self.tray.as_mut() {
                            t.readd();
                        }
                        continue;
                    }
                    _ => {}
                }
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
    log: Arc<Logger>,
    diag: Arc<DiagSink>,
    started: Instant,
}

static END_SESSION: Mutex<Option<EndSession>> = Mutex::new(None);

/// How long the exit waits for the capture thread (it checks its stop flag at least every 50 ms
/// between captures; a capture waits at most a few hundred ms for the GPU).
const CAPTURE_JOIN_MS: u64 = 3000;

/// Stops and joins the capture thread (never waiting more than `CAPTURE_JOIN_MS`), then shuts the
/// engine down (kills llama-server, joins its threads within 2 s and deletes the key file).
fn stop_workers(es: EndSession) {
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
        CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, MAIN_CLASS, w!("TranslateProbe Helper"), WS_POPUP, 0, 0, 0, 0, None, None, Some(hinstance()), None).ok()
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
        eprintln!("TranslateProbeHelper: internal error (line {line})");
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
    let view = SceneView { id: 0, rect: Rect { x: 0, y: OVERLAY_TOP, w: cw, h: ch - OVERLAY_TOP }, font_px: 14, cards: vec![SceneCard { card, kind: CardKind::Test, covered: false, block_px: 6.0, bg: CardBg::default() }] };
    let scene = Scene { wow: Some(WowGeom { hwnd: 0, origin_x: ox, origin_y: oy, client_w: cw, client_h: ch }), views: vec![view], hidden: None, lang_target: None, gdi: false };
    let center = (ox + (8 + 368) / 2, oy + (20 + 58) / 2);
    (scene, center, [ox, oy + OVERLAY_TOP, cw, ch - OVERLAY_TOP])
}

/// Runs the Helper; returns the process exit code.
pub fn run(cli: Cli) -> i32 {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    if cli.help || cli.version {
        attach_console();
        if cli.version {
            println!("TranslateProbeHelper {VERSION}");
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
                d.emit(json!({"ev": "engine", "req": u.key.req_id, "session": u.key.session, "status": name, "ms": ms}));
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
        let tip = i18n::tr_fmt(ui_lang, "tray.tip", &[("state", i18n::tr(ui_lang, "tray.state.unloaded"))]);
        let t = Tray::add(main.0 as isize, &tip);
        log.app(Level::Info, AppEvent::TrayIcon { ok: t.as_ref().is_some_and(|t| t.added()) });
        t
    };

    let mut ui = Ui {
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
    };

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

    if let Ok(mut g) = END_SESSION.lock() {
        *g = Some(EndSession { stop: stop.clone(), capture: capture.take(), engine, log: log.clone(), diag: diag.clone(), started });
    }

    let mut code = ui.message_loop();

    // None: WM_ENDSESSION already stopped everything and closed the log
    let es = END_SESSION.lock().ok().and_then(|mut g| g.take());
    let ended = es.is_none();
    if let Some(es) = es {
        stop_workers(es);
    }
    ui.overlay.hide_all();
    ui.toaster.hide();
    ui.tray = None;
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
    fn mask_runs_cover_set_bits() {
        let m = Mask { rect: Rect { x: 10, y: 20, w: 5, h: 2 }, bits: vec![255, 255, 0, 255, 0, 0, 0, 0, 0, 255] };
        assert_eq!(mask_runs(&m), vec![[20, 10, 12], [20, 13, 14], [21, 14, 15]]);
    }

    #[test]
    fn card_record_has_no_text_and_the_design_fields() {
        let sc = SceneCard { card: card(3, 1, 500, 2, 0.75), kind: CardKind::Request { key: JobKey { session: 4, req_id: 5 }, rows: 2, target: "fr".into() }, covered: false, block_px: 5.0, bg: CardBg::default() };
        let v = card_json(&sc, "done");
        for k in ["slot", "req", "view", "bands", "maskPx", "alpha", "state", "covered"] {
            assert!(v.get(k).is_some(), "{k}");
        }
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
