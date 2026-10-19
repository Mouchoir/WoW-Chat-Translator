//! fakewow: plays a TranslateProbe scenario in a window the Helper can capture, and writes the truth.
//!
//! ```text
//! fakewow.exe --scenario F --truth T [--fps 60] [--gamma G] [--contrast C]
//!   [--place offscreen|bottom|onscreen] [--lead-in-ms 2500] [--tail-ms 2000]
//!   [--selfcheck] [--wgc-check] [--layered] [--max-seconds S] [--class TPFakeWow]
//! ```
//! The window (class `TPFakeWow`, borderless, client = the scenario's screen size) never takes the
//! focus (amendment A1): it is created with WS_EX_NOACTIVATE, shown with SWP_NOACTIVATE and never
//! brought to the foreground. `offscreen` (default) puts it right of the virtual screen, where no
//! monitor shows it, as a layered window (a normal window gets no DWM updates there); `bottom` puts
//! it at (0,0) at the bottom of the z-order; `onscreen` puts it at (0,0) on top of the normal
//! windows (for the Helper's GDI capture mode, which reads the screen) and refuses to run while WoW
//! runs. The summary's `exposedPoints` counts grid points where fakewow was the visible window.
//!
//! Truth (JSON Lines, `t` = Unix epoch ms): header, ui, strip, covers, plates, layers, sent,
//! dropped, mark, selfcheck, end (the `lanes` records of the hint lanes went with them, addon
//! build 6). A scenario's layers (the conversation window, drawn over the chat frames and under
//! the covers) add their visible tag rows to `plates` and their meta data to `layers`.
//! `--selfcheck` grabs the window back with GDI about once a second while the picture is stable and checks it pixel for pixel against what was drawn, then reads the tags,
//! plates and strip calibration back at the truth rectangles. `--wgc-check` does the same with
//! Windows.Graphics.Capture (the Helper's primary capture method). Exit code: 0 pass, 1 a check
//! failed, 2 usage or file error, 3 refused (on-screen placement while the game runs).

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tp_testkit::display::Lut;
use tp_testkit::layout::{self, FrameLayout, Item, Measure};
use tp_testkit::raster::{self, IRect, Surface};
use tp_testkit::scenario::{self, Cover, Entry, FrameState, Layer, Scenario};
use tp_testkit::tagcode::{self, DIGITS, PLATE_RGB, SYNC};
use tp_testkit::truth::{self, Plate};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::*;

const GAME_EXES: &[&str] = &["wowb.exe", "wow.exe", "wowclassic.exe", "wowt.exe"];
/// The moving patch that keeps the window changing (a real game redraws every frame, and a capture
/// API only delivers a frame when something changed). Kept away from the strip and the chat.
const ANIM_W: i32 = 420;
const ANIM_H: i32 = 220;
const PANEL_ALPHA: f64 = 0.3;
/// Colour text is blended toward when a line fades (the chat panel over the ground, roughly).
const PANEL_RGB: [u8; 3] = [28, 32, 22];
const TOOLTIP_BG: [u8; 3] = [16, 16, 24];
const TOOLTIP_BORDER: [u8; 3] = [190, 190, 190];
/// A check starts once the picture has not changed (apart from the animated patch) for this long.
const STABLE_MS: u64 = 150;
const CHECK_EVERY_MS: u64 = 1000;
const WGC_WAIT_MS: u64 = 500;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Place {
    Offscreen,
    Bottom,
    Onscreen,
}

impl Place {
    fn name(self) -> &'static str {
        match self {
            Place::Offscreen => "offscreen",
            Place::Bottom => "bottom",
            Place::Onscreen => "onscreen",
        }
    }
}

struct Args {
    scenario: String,
    truth: String,
    fps: f64,
    gamma: Option<f64>,
    contrast: Option<f64>,
    place: Place,
    lead_in_ms: f64,
    tail_ms: f64,
    selfcheck: bool,
    wgc_check: bool,
    layered: bool,
    max_seconds: Option<f64>,
    class: String,
}

const USAGE: &str = "usage: fakewow --scenario FILE --truth FILE [--fps 60] [--gamma G] [--contrast C] \
[--place offscreen|bottom|onscreen] [--lead-in-ms 2500] [--tail-ms 2000] [--selfcheck] [--wgc-check] \
[--layered] [--max-seconds S] [--class TPFakeWow]";

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        scenario: String::new(),
        truth: String::new(),
        fps: 60.0,
        gamma: None,
        contrast: None,
        place: Place::Offscreen,
        lead_in_ms: 2500.0,
        tail_ms: 2000.0,
        selfcheck: false,
        wgc_check: false,
        layered: false,
        max_seconds: None,
        class: "TPFakeWow".into(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let (key, inline) = match arg.split_once('=') {
            Some((k, v)) if k.starts_with("--") => (k.to_string(), Some(v.to_string())),
            _ => (arg.clone(), None),
        };
        let mut value = |name: &str| -> Result<String, String> {
            match inline.clone() {
                Some(v) => Ok(v),
                None => it.next().ok_or_else(|| format!("{name} needs a value")),
            }
        };
        let num = |s: String, name: &str| s.parse::<f64>().map_err(|_| format!("{name}: not a number"));
        match key.as_str() {
            "--scenario" => a.scenario = value("--scenario")?,
            "--truth" => a.truth = value("--truth")?,
            "--fps" => a.fps = num(value("--fps")?, "--fps")?.clamp(5.0, 240.0),
            "--gamma" => a.gamma = Some(num(value("--gamma")?, "--gamma")?),
            "--contrast" => a.contrast = Some(num(value("--contrast")?, "--contrast")?),
            "--lead-in-ms" => a.lead_in_ms = num(value("--lead-in-ms")?, "--lead-in-ms")?.max(0.0),
            "--tail-ms" => a.tail_ms = num(value("--tail-ms")?, "--tail-ms")?.max(0.0),
            "--max-seconds" => a.max_seconds = Some(num(value("--max-seconds")?, "--max-seconds")?),
            "--class" => a.class = value("--class")?,
            "--place" => {
                a.place = match value("--place")?.as_str() {
                    "offscreen" => Place::Offscreen,
                    "bottom" => Place::Bottom,
                    "onscreen" => Place::Onscreen,
                    other => return Err(format!("--place {other}: expected offscreen, bottom or onscreen")),
                }
            }
            "--selfcheck" => a.selfcheck = true,
            "--wgc-check" => a.wgc_check = true,
            "--layered" => a.layered = true,
            "--help" | "-h" => return Err(String::new()),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if a.scenario.is_empty() || a.truth.is_empty() {
        return Err("--scenario and --truth are required".into());
    }
    Ok(a)
}

fn epoch_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// True when a WoW client process exists (process names from a Toolhelp snapshot; no handle to the
/// game is ever opened).
fn game_running() -> bool {
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return true };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut found = false;
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                let end = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                let name = String::from_utf16_lossy(&e.szExeFile[..end]).to_ascii_lowercase();
                if GAME_EXES.contains(&name.as_str()) {
                    found = true;
                    break;
                }
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
        found
    }
}

fn cpu_seconds() -> f64 {
    unsafe {
        let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        if GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u).is_ok() {
            let t = |f: FILETIME| ((u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime)) as f64 / 1e7;
            t(k) + t(u)
        } else {
            0.0
        }
    }
}

// ------------------------------------------------------------------------------------------------
// Window

thread_local! {
    /// Memory DC holding the current picture, for WM_PAINT.
    static PAINT_DC: RefCell<Option<(isize, i32, i32)>> = const { RefCell::new(None) };
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => unsafe {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut ps);
            if let Some((mem, w, h)) = PAINT_DC.with(|p| *p.borrow()) {
                let _ = BitBlt(dc, 0, 0, w, h, Some(HDC(mem as *mut core::ffi::c_void)), 0, 0, SRCCOPY);
            }
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        },
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// Points of a 5 x 5 grid over the client where our window is the one a user would see (A1 check):
/// a point on a monitor whose top window is fakewow.
fn exposed_points(hwnd: HWND, origin: (i32, i32), w: i32, h: i32) -> u64 {
    let mut n = 0;
    for i in 0..5 {
        for j in 0..5 {
            let p = POINT { x: origin.0 + w * (2 * i + 1) / 10, y: origin.1 + h * (2 * j + 1) / 10 };
            unsafe {
                if MonitorFromPoint(p, MONITOR_DEFAULTTONULL).is_invalid() {
                    continue;
                }
                let top = WindowFromPoint(p);
                if !top.is_invalid() && GetAncestor(top, GA_ROOT) == hwnd {
                    n += 1;
                }
            }
        }
    }
    n
}

fn pump() {
    unsafe {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn create_window(class: &str, w: i32, h: i32, layered: bool) -> Result<HWND, String> {
    unsafe {
        let hinst: HINSTANCE = GetModuleHandleW(None).map_err(|e| format!("GetModuleHandle: {e}"))?.into();
        let mut cls = wide(class);
        cls.push(0);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            lpszClassName: PCWSTR(cls.as_ptr()),
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            return Err("RegisterClassEx failed".into());
        }
        // Created hidden, at its final size; shown later without activation.
        let ex = WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | if layered { WS_EX_LAYERED } else { WINDOW_EX_STYLE(0) };
        let hwnd = CreateWindowExW(
            ex,
            PCWSTR(cls.as_ptr()),
            w!("fakewow"),
            WS_POPUP,
            0,
            0,
            w,
            h,
            None,
            None,
            Some(hinst),
            None,
        )
        .map_err(|e| format!("CreateWindowEx: {e}"))?;
        // Windows 11 rounds window corners, which would hide the strip's corner pixels.
        let pref = DWMWCP_DONOTROUND;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &pref as *const _ as *const core::ffi::c_void, std::mem::size_of_val(&pref) as u32);
        Ok(hwnd)
    }
}

/// Shows the window at its place without activating it. Returns the client origin on screen.
fn place_window(hwnd: HWND, place: Place, w: i32, h: i32) -> (i32, i32) {
    unsafe {
        let (vx, vy, vw) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN), GetSystemMetrics(SM_CXVIRTUALSCREEN));
        let (x, y, after) = match place {
            Place::Offscreen => (vx + vw + 100, vy, HWND_BOTTOM),
            Place::Bottom => (0, 0, HWND_BOTTOM),
            Place::Onscreen => (0, 0, HWND_TOP),
        };
        let _ = SetWindowPos(hwnd, Some(after), x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        let mut p = POINT { x: 0, y: 0 };
        let _ = ClientToScreen(hwnd, &mut p);
        (p.x, p.y)
    }
}

// ------------------------------------------------------------------------------------------------
// Drawing

struct Gfx {
    mem: HDC,
    bits: *mut u8,
    w: i32,
    h: i32,
    fonts: HashMap<i32, HFONT>,
    font_px: i32,
    widths: HashMap<(i32, String), f64>,
    world: Vec<u8>,
    lut: Lut,
}

impl Gfx {
    fn new(w: i32, h: i32, lut: Lut) -> Result<Gfx, String> {
        unsafe {
            let screen = GetDC(None);
            let mem = CreateCompatibleDC(Some(screen));
            ReleaseDC(None, screen);
            let bits = dib(mem, w, h)?;
            SetBkMode(mem, TRANSPARENT);
            let world = raster::make_world(w, h);
            Ok(Gfx { mem, bits, w, h, fonts: HashMap::new(), font_px: 0, widths: HashMap::new(), world, lut })
        }
    }

    fn surface(&mut self) -> Surface<'_> {
        let n = (self.w * self.h * 4) as usize;
        Surface::new(unsafe { std::slice::from_raw_parts_mut(self.bits, n) }, self.w, self.h)
    }

    fn pixels(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.bits, (self.w * self.h * 4) as usize) }
    }

    fn select_font(&mut self, px: i32) {
        if self.font_px == px {
            return;
        }
        let f = *self.fonts.entry(px).or_insert_with(|| unsafe {
            // positive height = cell height: the glyphs (descenders included) fit the chat row,
            // as WoW's font height is its line height
            CreateFontW(px, 0, 0, 0, FW_NORMAL.0 as i32, 0, 0, 0, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, ANTIALIASED_QUALITY, (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32, w!("Arial"))
        });
        unsafe {
            SelectObject(self.mem, f.into());
        }
        self.font_px = px;
    }

    fn text(&mut self, x: i32, y: i32, s: &str, rgb: [u8; 3], clip: IRect) {
        let wd = wide(s);
        unsafe {
            SelectClipRgn(self.mem, None);
            IntersectClipRect(self.mem, clip.x, clip.y, clip.right(), clip.bottom());
            SetTextColor(self.mem, COLORREF(u32::from(rgb[0]) | (u32::from(rgb[1]) << 8) | (u32::from(rgb[2]) << 16)));
            let _ = TextOutW(self.mem, x, y, &wd);
        }
    }

    fn flush(&self) {
        unsafe {
            let _ = GdiFlush();
        }
    }
}

/// A top-down 32-bit DIB selected into `mem`; returns its bits.
fn dib(mem: HDC, w: i32, h: i32) -> Result<*mut u8, String> {
    unsafe {
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0).map_err(|e| format!("CreateDIBSection: {e}"))?;
        SelectObject(mem, bmp.into());
        Ok(bits as *mut u8)
    }
}

struct GdiMeasure<'a> {
    g: &'a mut Gfx,
    px: i32,
}

impl Measure for GdiMeasure<'_> {
    fn width(&mut self, text: &str) -> f64 {
        let key = (self.px, text.to_string());
        if let Some(w) = self.g.widths.get(&key) {
            return *w;
        }
        self.g.select_font(self.px);
        let wd = wide(text);
        let mut size = SIZE::default();
        unsafe {
            let _ = GetTextExtentPoint32W(self.g.mem, &wd, &mut size);
        }
        let w = f64::from(size.cx);
        self.g.widths.insert(key, w);
        w
    }
}

fn blend_rgb(c: [u8; 3], bg: [u8; 3], a: f64) -> [u8; 3] {
    let m = |x: u8, y: u8| (f64::from(y) + (f64::from(x) - f64::from(y)) * a).round().clamp(0.0, 255.0) as u8;
    [m(c[0], bg[0]), m(c[1], bg[1]), m(c[2], bg[2])]
}

// ------------------------------------------------------------------------------------------------
// Playback state

struct ActiveCover {
    id: u32,
    spec: Cover,
    rect: Option<IRect>,
}

struct State {
    ui: bool,
    frames: Vec<FrameState>,
    history: HashMap<u8, Vec<Entry>>,
    strip: Option<Vec<[u8; 3]>>,
    covers: Vec<ActiveCover>,
    /// Addon windows over the chat frames, by id.
    layers: Vec<Layer>,
    drop: Option<(u32, u32)>,
    scroll_since: HashMap<u8, f64>,
    next: usize,
    /// Bumped whenever anything drawn (other than fading) changes.
    version: u64,
}

#[derive(Default)]
struct Applied {
    sent: Vec<(scenario::Sent, bool)>,
    dropped: Vec<(u32, u32)>,
    marks: Vec<String>,
}

impl State {
    fn new() -> State {
        State {
            ui: true,
            frames: Vec::new(),
            history: HashMap::new(),
            strip: None,
            covers: Vec::new(),
            layers: Vec::new(),
            drop: None,
            scroll_since: HashMap::new(),
            next: 0,
            version: 0,
        }
    }

    /// Applies every event up to scenario time `t` (ms).
    fn apply(&mut self, sc: &Scenario, t: f64) -> Applied {
        let mut out = Applied::default();
        while self.next < sc.events.len() && sc.events[self.next].t <= t {
            let ev = &sc.events[self.next];
            self.next += 1;
            if let Some(ui) = ev.ui {
                if ui != self.ui {
                    self.ui = ui;
                    self.version += 1;
                }
            }
            if let Some(frames) = &ev.frames {
                for f in frames {
                    let old = self.frames.iter().find(|o| o.id == f.id);
                    if old.map(|o| o.scroll) != Some(f.scroll) {
                        self.scroll_since.insert(f.id, ev.t);
                    }
                }
                self.frames = frames.clone();
                self.version += 1;
            }
            if let Some(hs) = &ev.history {
                for h in hs {
                    self.history.insert(h.id, h.entries.clone());
                }
                self.version += 1;
            }
            if let Some(s) = &ev.strip {
                self.strip = if s.shown { Some(scenario::strip_blocks(&s.rgb)) } else { None };
                self.version += 1;
            }
            if let Some(c) = &ev.cover {
                self.covers.retain(|a| a.id != c.id);
                if c.on {
                    let rect = c.rect.map(|r| IRect::new(r[0], r[1], r[2], r[3]));
                    self.covers.push(ActiveCover { id: c.id, spec: c.clone(), rect });
                }
                self.version += 1;
            }
            if let Some(l) = &ev.layer {
                self.layers.retain(|a| a.id != l.id);
                if l.on {
                    self.layers.push(l.clone());
                    self.layers.sort_by_key(|a| a.id);
                }
                self.version += 1;
            }
            if let Some(d) = &ev.drop {
                self.drop = if d.on { Some((d.req, d.slot)) } else { None };
                if d.on {
                    out.dropped.push((d.req, d.slot));
                }
                self.version += 1;
            }
            if let Some(s) = &ev.sent {
                out.sent.push((s.clone(), self.drop.is_some()));
            }
            if let Some(m) = &ev.mark {
                out.marks.push(m.clone());
            }
        }
        out
    }

    fn strip_painted(&self) -> Option<&Vec<[u8; 3]>> {
        if self.ui && self.drop.is_none() {
            self.strip.as_ref()
        } else {
            None
        }
    }

    fn cover_rects(&self) -> Vec<IRect> {
        if !self.ui {
            return Vec::new();
        }
        self.covers.iter().filter_map(|c| c.rect).collect()
    }

    /// Resolves covers given relative to a plate, once that plate is on screen.
    fn resolve_covers(&mut self, plates: &[Plate]) -> bool {
        let mut changed = false;
        for c in self.covers.iter_mut().filter(|c| c.rect.is_none()) {
            let s = &c.spec;
            let Some(p) = plates.iter().find(|p| Some(p.view) == s.frame.or(Some(p.view)) && Some(p.id.slot) == s.slot && Some(p.id.row) == s.row.or(Some(0))) else { continue };
            let Some(pf) = p.plate_f else { continue };
            let fx = s.fx.unwrap_or([0.5, 1.0]);
            let pad = s.pad.unwrap_or([8, 40, 14, 0]);
            let pw = pf[2] - pf[0];
            let x0 = (pf[0] + fx[0] * pw).round() as i32 - pad[3];
            let x1 = (pf[0] + fx[1] * pw).round() as i32 + pad[1];
            let y0 = pf[1].round() as i32 - pad[0];
            let y1 = pf[3].round() as i32 + pad[2];
            c.rect = Some(IRect::new(x0, y0, x1 - x0, y1 - y0));
            changed = true;
        }
        if changed {
            self.version += 1;
        }
        changed
    }
}

fn layouts(st: &State, sc: &Scenario, now: f64, g: &mut Gfx) -> Vec<FrameLayout> {
    let mut out = Vec::new();
    for f in st.frames.iter().filter(|f| f.shown) {
        let empty = Vec::new();
        let entries = st.history.get(&f.id).unwrap_or(&empty);
        let since = st.scroll_since.get(&f.id).copied().unwrap_or(0.0);
        let fade = sc.fade.clone();
        let scrolled = f.scroll != 0;
        let alpha = |i: usize| -> f64 {
            if scrolled {
                1.0
            } else {
                layout::fade_alpha(now, entries[i].ts.max(since), fade.visible, fade.duration)
            }
        };
        let mut m = GdiMeasure { g, px: f.font_px };
        out.push(layout::layout_frame(f, entries, &alpha, &mut m));
    }
    out
}

/// Fade signature: changes when any drawn row's alpha changes visibly.
fn alpha_sig(ls: &[FrameLayout]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for l in ls {
        for r in &l.rows {
            for v in [r.y as u64, r.entry as u64, (r.alpha * 255.0).round() as u64] {
                h = (h ^ v).wrapping_mul(0x100_0000_01b3);
            }
        }
    }
    h
}

/// Screen areas the UI may draw on: the strip band, the chat panels and the covers.
fn ui_rects(st: &State, ls: &[FrameLayout], w: i32) -> Vec<IRect> {
    let mut v = vec![IRect::new(0, 0, w, 8)];
    for l in ls {
        v.push(l.clip.inflate(4));
    }
    v.extend(st.covers.iter().filter_map(|c| c.rect.map(|r| r.inflate(1))));
    v.extend(st.layers.iter().filter_map(|l| l.bounds()).map(|b| IRect::new(b[0], b[1], b[2], b[3]).inflate(1)));
    v
}

fn draw_region(g: &mut Gfx, r: IRect, st: &State, ls: &[FrameLayout]) {
    let Some(r) = r.intersect(&IRect::new(0, 0, g.w, g.h)) else { return };
    let world = std::mem::take(&mut g.world);
    g.surface().copy_from(&world, r);
    g.world = world;
    if st.ui {
        {
            let mut s = g.surface();
            for l in ls {
                if let Some(p) = l.clip.inflate(4).intersect(&r) {
                    s.blend_rect(p, [0, 0, 0], PANEL_ALPHA);
                }
                let Some(cr) = l.clip.intersect(&r) else { continue };
                for row in &l.rows {
                    for it in &row.items {
                        if let Item::Tex { x0, x1, y0, y1, rgb, .. } = it {
                            let cx = f64::from(l.clip.x);
                            let ry = f64::from(row.y);
                            s.fill_aa(cx + x0, ry + y0, cx + x1, ry + y1, *rgb, row.alpha, cr);
                        }
                    }
                }
            }
        }
        for l in ls {
            let Some(cr) = l.clip.intersect(&r) else { continue };
            for row in &l.rows {
                for it in &row.items {
                    if let Item::Text { x, text, rgb, .. } = it {
                        if text.trim().is_empty() {
                            continue;
                        }
                        let fp = frame_font(st, l.frame_id);
                        g.select_font(fp);
                        let ty = row.y + (row.height - fp) / 2;
                        g.text(l.clip.x + x.round() as i32, ty, text, blend_rgb(*rgb, PANEL_RGB, row.alpha), cr);
                    }
                }
            }
        }
        g.flush();
        // addon windows over the chat frames (opaque rectangles, in order)
        {
            let mut s = g.surface();
            for l in &st.layers {
                for q in &l.rects {
                    if let Some(p) = IRect::new(q[0], q[1], q[2], q[3]).intersect(&r) {
                        s.fill(p, [q[4].clamp(0, 255) as u8, q[5].clamp(0, 255) as u8, q[6].clamp(0, 255) as u8]);
                    }
                }
            }
        }
        for c in st.covers.iter().filter_map(|c| c.rect) {
            let Some(cr) = c.intersect(&r) else { continue };
            {
                let mut s = g.surface();
                s.blend_rect(cr, TOOLTIP_BG, 0.94);
                for edge in [IRect::new(c.x, c.y, c.w, 1), IRect::new(c.x, c.bottom() - 1, c.w, 1), IRect::new(c.x, c.y, 1, c.h), IRect::new(c.right() - 1, c.y, 1, c.h)] {
                    if let Some(e) = edge.intersect(&r) {
                        s.fill(e, TOOLTIP_BORDER);
                    }
                }
            }
            g.select_font(18);
            if let Some(inner) = c.inflate(-2).intersect(&r) {
                g.text(c.x + 8, c.y + 4, "Linen Cloth", [255, 255, 255], inner);
                g.text(c.x + 8, c.y + 26, "Crafting Reagent", [255, 210, 0], inner);
            }
        }
        g.flush();
        if let Some(blocks) = st.strip_painted() {
            let mut s = g.surface();
            for (k, c) in blocks.iter().enumerate() {
                let (bx, by) = scenario::block_pos(k);
                if let Some(b) = IRect::new(bx, by, 2, 2).intersect(&r) {
                    s.fill(b, *c);
                }
            }
        }
    }
    g.flush();
    let lut = g.lut.clone();
    g.surface().apply_lut(r, &lut);
}

fn frame_font(st: &State, id: u8) -> i32 {
    st.frames.iter().find(|f| f.id == id).map(|f| f.font_px).unwrap_or(14)
}

fn anim_rect(w: i32) -> IRect {
    IRect::new(w - ANIM_W - 40, 120, ANIM_W, ANIM_H)
}

fn draw_anim(g: &mut Gfx, frame_no: u64) {
    let r = anim_rect(g.w);
    let world = std::mem::take(&mut g.world);
    g.surface().copy_from(&world, r);
    g.world = world;
    let t = frame_no as f64 / 60.0;
    let x = r.x as f64 + (r.w - 48) as f64 * (0.5 + 0.5 * (t * 1.3).sin());
    let y = r.y as f64 + (r.h - 48) as f64 * (0.5 + 0.5 * (t * 0.9).cos());
    let c = [(128.0 + 120.0 * (t * 2.0).sin()) as u8, (128.0 + 120.0 * (t * 1.7).cos()) as u8, 200];
    {
        let mut s = g.surface();
        s.fill_aa(x, y, x + 48.0, y + 48.0, c, 1.0, r);
    }
    let lut = g.lut.clone();
    g.surface().apply_lut(r, &lut);
}

fn blit(hwnd: HWND, g: &Gfx, rects: &[IRect], layered: bool) {
    if layered {
        // A layered window gets its picture through UpdateLayeredWindow, one dirty rectangle at a
        // time (their bounding box would often be most of the window).
        let size = SIZE { cx: g.w, cy: g.h };
        let src = POINT { x: 0, y: 0 };
        let mut done: Vec<IRect> = Vec::new();
        for r in rects {
            if done.iter().any(|d| d.intersect(r) == Some(*r)) {
                continue;
            }
            done.push(*r);
            let rc = RECT { left: r.x, top: r.y, right: r.right(), bottom: r.bottom() };
            let info = UPDATELAYEREDWINDOWINFO {
                cbSize: std::mem::size_of::<UPDATELAYEREDWINDOWINFO>() as u32,
                psize: &size,
                hdcSrc: g.mem,
                pptSrc: &src,
                dwFlags: ULW_OPAQUE,
                prcDirty: &rc,
                ..Default::default()
            };
            unsafe {
                let _ = UpdateLayeredWindowIndirect(hwnd, &info);
            }
        }
        return;
    }
    unsafe {
        let dc = GetDC(Some(hwnd));
        for r in rects {
            if let Some(r) = r.intersect(&IRect::new(0, 0, g.w, g.h)) {
                let _ = BitBlt(dc, r.x, r.y, r.w, r.h, Some(g.mem), r.x, r.y, SRCCOPY);
            }
        }
        ReleaseDC(Some(hwnd), dc);
    }
}

// ------------------------------------------------------------------------------------------------
// Truth

struct Truth {
    out: BufWriter<File>,
    lines: u64,
    last_flush: Instant,
}

impl Truth {
    fn open(path: &str) -> Result<Truth, String> {
        let f = File::create(path).map_err(|e| format!("{path}: {e}"))?;
        Ok(Truth { out: BufWriter::new(f), lines: 0, last_flush: Instant::now() })
    }

    fn emit(&mut self, t: f64, mut v: Value) {
        if let Value::Object(m) = &mut v {
            m.insert("t".into(), json!((t * 10.0).round() / 10.0));
        }
        let _ = writeln!(self.out, "{v}");
        self.lines += 1;
        if self.last_flush.elapsed() > Duration::from_millis(250) {
            self.flush();
        }
    }

    fn flush(&mut self) {
        let _ = self.out.flush();
        self.last_flush = Instant::now();
    }
}

fn plates_json(ps: &[Plate], ui: bool, covers: &[IRect]) -> Value {
    Value::Array(ps.iter().map(|p| truth::item_json(p, ui, covers)).collect())
}

// ------------------------------------------------------------------------------------------------
// Self-check

#[derive(Default)]
struct CheckStats {
    checks: u64,
    pixel_mismatch: u64,
    max_diff: u8,
    tags_ok: u64,
    tags_bad: u64,
    plates_ok: u64,
    plates_bad: u64,
    strip_ok: u64,
    strip_bad: u64,
    timeouts: u64,
    errors: u64,
}

impl CheckStats {
    fn json(&self) -> Value {
        json!({"checks": self.checks, "pixelMismatch": self.pixel_mismatch, "maxDiff": self.max_diff,
               "tagsOk": self.tags_ok, "tagsBad": self.tags_bad, "platesOk": self.plates_ok, "platesBad": self.plates_bad,
               "stripOk": self.strip_ok, "stripBad": self.strip_bad, "timeouts": self.timeouts, "errors": self.errors})
    }

    fn pass(&self) -> bool {
        self.checks > 0 && self.pixel_mismatch == 0 && self.tags_bad == 0 && self.plates_bad == 0 && self.strip_bad == 0 && self.timeouts == 0 && self.errors == 0
    }
}

/// (mismatching pixels, largest channel difference) between two BGRA images, outside `skip`.
fn compare(a: &[u8], b: &[u8], w: i32, h: i32, skip: Option<IRect>) -> (u64, u8) {
    let mut bad = 0u64;
    let mut worst = 0u8;
    for y in 0..h {
        let row = (y * w * 4) as usize;
        for x in 0..w {
            if skip.is_some_and(|s| s.contains(x, y)) {
                continue;
            }
            let i = row + (x * 4) as usize;
            let d = a[i].abs_diff(b[i]).max(a[i + 1].abs_diff(b[i + 1])).max(a[i + 2].abs_diff(b[i + 2]));
            if d != 0 {
                bad += 1;
                worst = worst.max(d);
            }
        }
    }
    (bad, worst)
}

fn px(img: &[u8], w: i32, x: i32, y: i32) -> [f32; 3] {
    let i = ((y * w + x) * 4) as usize;
    [f32::from(img[i + 2]), f32::from(img[i + 1]), f32::from(img[i])]
}

/// Reads the tags, plates and strip calibration back from a captured image at the truth rectangles.
fn verify_truth(img: &[u8], w: i32, plates: &[Plate], covers: &[IRect], st: &State, lut: &Lut, stats: &mut CheckStats) {
    let mut refs = [[0f32; 3]; 8];
    for (i, c) in SYNC.iter().chain(DIGITS.iter()).enumerate() {
        let m = lut.map(*c);
        refs[i] = [f32::from(m[0]), f32::from(m[1]), f32::from(m[2])];
    }
    if st.ui {
        for p in plates {
            let (Some(tag), true) = (p.tag, p.alpha >= 0.999) else { continue };
            let full = IRect::from_edges(p.tag_f[0], p.tag_f[1], p.tag_f[2], p.tag_f[3]);
            if tag != full || covers.iter().any(|c| c.intersect(&full).is_some()) {
                continue;
            }
            let bw = (p.tag_f[2] - p.tag_f[0]) / 8.0;
            let cy = ((p.tag_f[1] + p.tag_f[3]) / 2.0).floor() as i32;
            let mut obs = [[0f32; 3]; 8];
            for (i, o) in obs.iter_mut().enumerate() {
                let cx = (p.tag_f[0] + (i as f64 + 0.5) * bw).floor() as i32;
                *o = px(img, w, cx, cy);
            }
            if tagcode::decode_observed(&obs, &refs) == Some(p.id) {
                stats.tags_ok += 1;
            } else {
                stats.tags_bad += 1;
            }
            if let Some(pl) = p.plate {
                let want = lut.map(PLATE_RGB);
                for f in [0.25, 0.5, 0.75] {
                    let x = pl.x + (f64::from(pl.w) * f) as i32;
                    let y = pl.y + pl.h / 2;
                    if covers.iter().any(|c| c.inflate(1).contains(x, y)) {
                        continue;
                    }
                    let got = px(img, w, x, y);
                    let ok = (0..3).all(|c| (got[c] - f32::from(want[c])).abs() <= 1.0);
                    if ok {
                        stats.plates_ok += 1;
                    } else {
                        stats.plates_bad += 1;
                    }
                    break;
                }
            }
        }
    }
    if let Some(blocks) = st.strip_painted() {
        for (k, c) in blocks.iter().take(8).enumerate() {
            let (bx, by) = scenario::block_pos(k);
            let want = lut.map(*c);
            let got = px(img, w, bx, by);
            if (0..3).all(|i| (got[i] - f32::from(want[i])).abs() < 0.5) {
                stats.strip_ok += 1;
            } else {
                stats.strip_bad += 1;
            }
        }
    }
}

struct GdiGrab {
    mem: HDC,
    bits: *mut u8,
    n: usize,
}

impl GdiGrab {
    fn new(w: i32, h: i32) -> Result<GdiGrab, String> {
        unsafe {
            let screen = GetDC(None);
            let mem = CreateCompatibleDC(Some(screen));
            ReleaseDC(None, screen);
            let bits = dib(mem, w, h)?;
            Ok(GdiGrab { mem, bits, n: (w * h * 4) as usize })
        }
    }

    /// Reads the window's client area back: BitBlt from its own DC, or PrintWindow for a layered
    /// window (whose picture only DWM holds).
    fn grab(&mut self, hwnd: HWND, w: i32, h: i32, layered: bool) -> &[u8] {
        unsafe {
            if layered {
                let flags = windows::Win32::Storage::Xps::PRINT_WINDOW_FLAGS(windows::Win32::Storage::Xps::PW_CLIENTONLY.0 | PW_RENDERFULLCONTENT);
                let _ = windows::Win32::Storage::Xps::PrintWindow(hwnd, self.mem, flags);
            } else {
                let dc = GetDC(Some(hwnd));
                let _ = BitBlt(self.mem, 0, 0, w, h, Some(dc), 0, 0, SRCCOPY);
                ReleaseDC(Some(hwnd), dc);
            }
            let _ = GdiFlush();
            std::slice::from_raw_parts(self.bits, self.n)
        }
    }
}

mod wgc {
    use windows::core::Interface;
    use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureAccess, GraphicsCaptureAccessKind, GraphicsCaptureItem, GraphicsCaptureSession};
    use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
    use windows::Graphics::DirectX::DirectXPixelFormat;
    use windows::Win32::Foundation::{HMODULE, HWND};
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
    use windows::Win32::Graphics::Direct3D11::*;
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
    use windows::Win32::Graphics::Dxgi::IDXGIDevice;
    use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
    use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
    use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

    pub struct Wgc {
        ctx: ID3D11DeviceContext,
        pool: Direct3D11CaptureFramePool,
        session: GraphicsCaptureSession,
        staging: ID3D11Texture2D,
        w: i32,
        h: i32,
        pub frames: u64,
    }

    pub fn open(hwnd: HWND, w: i32, h: i32) -> windows::core::Result<Wgc> {
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
            if !GraphicsCaptureSession::IsSupported()? {
                return Err(windows::core::Error::new(windows::Win32::Foundation::E_NOTIMPL, "WGC unsupported"));
            }
            if let Ok(op) = GraphicsCaptureAccess::RequestAccessAsync(GraphicsCaptureAccessKind::Borderless) {
                let _ = op.join();
            }
            let mut device: Option<ID3D11Device> = None;
            let mut ctx: Option<ID3D11DeviceContext> = None;
            D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut ctx))?;
            let device = device.unwrap();
            let dxgi: IDXGIDevice = device.cast()?;
            let d3d: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?.cast()?;
            let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
            let item: GraphicsCaptureItem = interop.CreateForWindow(hwnd)?;
            let size = item.Size()?;
            let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&d3d, DirectXPixelFormat::B8G8R8A8UIntNormalized, 1, size)?;
            let session = pool.CreateCaptureSession(&item)?;
            let _ = session.SetIsCursorCaptureEnabled(false);
            let _ = session.SetIsBorderRequired(false);
            session.StartCapture()?;
            let desc = D3D11_TEXTURE2D_DESC {
                Width: w as u32,
                Height: h as u32,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
            };
            let mut staging: Option<ID3D11Texture2D> = None;
            device.CreateTexture2D(&desc, None, Some(&mut staging))?;
            Ok(Wgc { ctx: ctx.unwrap(), pool, session, staging: staging.unwrap(), w, h, frames: 0 })
        }
    }

    impl Wgc {
        /// Copies the newest frame (if any) into `out` (BGRA, w x h). Ok(false): no new frame.
        pub fn latest(&mut self, out: &mut Vec<u8>) -> windows::core::Result<bool> {
            let mut newest = None;
            while let Ok(f) = self.pool.TryGetNextFrame() {
                if let Some(old) = newest.replace(f) {
                    let _ = old.Close();
                }
            }
            let Some(frame) = newest else { return Ok(false) };
            self.frames += 1;
            unsafe {
                let surf = frame.Surface()?;
                let access: IDirect3DDxgiInterfaceAccess = surf.cast()?;
                let tex: ID3D11Texture2D = access.GetInterface()?;
                let b = D3D11_BOX { left: 0, top: 0, front: 0, right: self.w as u32, bottom: self.h as u32, back: 1 };
                self.ctx.CopySubresourceRegion(&self.staging, 0, 0, 0, 0, &tex, 0, Some(&b));
                let _ = frame.Close();
                let mut m = D3D11_MAPPED_SUBRESOURCE::default();
                self.ctx.Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
                out.clear();
                for row in 0..self.h as usize {
                    let p = (m.pData as *const u8).add(row * m.RowPitch as usize);
                    out.extend_from_slice(std::slice::from_raw_parts(p, (self.w * 4) as usize));
                }
                self.ctx.Unmap(&self.staging, 0);
            }
            Ok(true)
        }
    }

    impl Drop for Wgc {
        fn drop(&mut self) {
            let _ = self.session.Close();
            let _ = self.pool.Close();
        }
    }
}

struct WgcPending {
    version: u64,
    deadline: Instant,
    last: Option<(u64, u8)>,
}

// ------------------------------------------------------------------------------------------------

fn run(args: Args) -> Result<bool, String> {
    let text = std::fs::read_to_string(&args.scenario).map_err(|e| format!("{}: {e}", args.scenario))?;
    let sc = Scenario::parse(&text)?;
    let (w, h) = (sc.phys_w, sc.phys_h);
    let lut = Lut::new(args.gamma.unwrap_or(sc.display.gamma), args.contrast.unwrap_or(sc.display.contrast));
    // Off every monitor, a normal window's GDI drawing never reaches its DWM surface (measured: WGC
    // delivers one frame, then nothing; a read-back of the window DC is black). A layered window
    // fed with UpdateLayeredWindow is composed wherever it is, so the off-screen placement uses one.
    let layered = args.layered || args.place == Place::Offscreen;
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let fg_before = unsafe { GetForegroundWindow() };
    let mut truth = Truth::open(&args.truth)?;
    let hwnd = create_window(&args.class, w, h, layered)?;
    let mut g = Gfx::new(w, h, lut.clone())?;
    PAINT_DC.with(|p| *p.borrow_mut() = Some((g.mem.0 as isize, w, h)));
    let mut st = State::new();
    let cpu0 = cpu_seconds();

    // Initial picture (events at t <= 0), drawn before the window is shown.
    let applied0 = st.apply(&sc, 0.0);
    let mut ls = layouts(&st, &sc, 0.0, &mut g);
    let mut plates: Vec<Plate> = ls.iter().flat_map(truth::plates).collect();
    st.resolve_covers(&plates);
    draw_region(&mut g, IRect::new(0, 0, w, h), &st, &ls);
    draw_anim(&mut g, 0);
    if layered {
        blit(hwnd, &g, &[IRect::new(0, 0, w, h)], true);
    }
    let origin = place_window(hwnd, args.place, w, h);
    blit(hwnd, &g, &[IRect::new(0, 0, w, h)], layered);
    pump();

    let period = Duration::from_secs_f64(1.0 / args.fps);
    let lead = Duration::from_secs_f64(args.lead_in_ms / 1000.0);
    let start = Instant::now();
    let play_start = start + lead;
    let play_start_epoch = epoch_ms() + args.lead_in_ms;
    let end_ms = sc.duration_ms + args.tail_ms;
    let watchdog = Duration::from_secs_f64(args.max_seconds.unwrap_or((args.lead_in_ms + end_ms) / 1000.0 + 30.0));
    truth.emit(
        epoch_ms(),
        json!({"ev": "header", "scenario": sc.name, "physW": w, "physH": h, "place": args.place.name(), "origin": [origin.0, origin.1],
               "gamma": args.gamma.unwrap_or(sc.display.gamma), "contrast": args.contrast.unwrap_or(sc.display.contrast),
               "fps": args.fps, "leadInMs": args.lead_in_ms, "playStart": play_start_epoch, "helperStartMs": sc.helper_start_ms,
               "durationMs": sc.duration_ms, "tailMs": args.tail_ms, "session": sc.session, "class": args.class}),
    );
    truth.flush();

    let mut stats_gdi = CheckStats::default();
    let mut stats_wgc = CheckStats::default();
    let mut grab = if args.selfcheck { Some(GdiGrab::new(w, h)?) } else { None };
    let mut wgc_cap = None;
    let mut wgc_error = None;
    if args.wgc_check {
        match wgc::open(hwnd, w, h) {
            Ok(c) => wgc_cap = Some(c),
            Err(e) => {
                wgc_error = Some(format!("0x{:08X}", e.code().0));
                stats_wgc.errors += 1;
            }
        }
    }
    let mut wgc_buf = Vec::new();
    let mut wgc_pending: Option<WgcPending> = None;

    // Truth state
    let mut last_items = String::new();
    let mut last_strip: Option<bool> = None;
    let mut last_ui: Option<bool> = None;
    let mut last_covers = String::new();
    let mut last_layers = String::new();
    let mut pending_sent: Vec<(scenario::Sent, bool)> = applied0.sent;
    let mut pending_dropped: Vec<(u32, u32)> = applied0.dropped;
    let mut pending_marks: Vec<String> = applied0.marks;

    let mut drawn_version = st.version;
    let mut drawn_alpha = alpha_sig(&ls);
    let mut prev_ui_rects = ui_rects(&st, &ls, w);
    let mut stable_since = Instant::now();
    let mut last_check = Instant::now() - Duration::from_millis(CHECK_EVERY_MS);
    let mut checked_version = u64::MAX;
    let mut frame_no: u64 = 0;
    let mut late = 0u64;
    let mut own_fg = 0u64;
    let (mut exposed, mut exposure_checks) = (0u64, 0u64);
    let mut next = Instant::now();
    loop {
        pump();
        let now = Instant::now();
        if now.duration_since(start) > watchdog {
            truth.emit(epoch_ms(), json!({"ev": "watchdog"}));
            break;
        }
        let t = if now >= play_start { now.duration_since(play_start).as_secs_f64() * 1000.0 } else { 0.0 };
        if t > end_ms {
            break;
        }
        let a = st.apply(&sc, t);
        pending_sent.extend(a.sent);
        pending_dropped.extend(a.dropped);
        pending_marks.extend(a.marks);

        ls = layouts(&st, &sc, t, &mut g);
        plates = ls.iter().flat_map(truth::plates).collect();
        st.resolve_covers(&plates);
        let sig = alpha_sig(&ls);
        let mut dirty: Vec<IRect> = Vec::new();
        if st.version != drawn_version || sig != drawn_alpha {
            let cur = ui_rects(&st, &ls, w);
            for r in prev_ui_rects.iter().chain(cur.iter()) {
                if dirty.iter().any(|d| d.intersect(r) == Some(*r)) {
                    continue; // already redrawn (most rectangles do not move)
                }
                draw_region(&mut g, *r, &st, &ls);
                dirty.push(*r);
            }
            prev_ui_rects = cur;
            drawn_version = st.version;
            drawn_alpha = sig;
            stable_since = now;
        }
        frame_no += 1;
        draw_anim(&mut g, frame_no);
        dirty.push(anim_rect(w));
        blit(hwnd, &g, &dirty, layered);
        let painted = epoch_ms();

        // Truth for what is now on screen.
        if last_ui != Some(st.ui) {
            truth.emit(painted, json!({"ev": "ui", "shown": st.ui}));
            last_ui = Some(st.ui);
        }
        let strip_now = st.strip_painted().is_some();
        if last_strip != Some(strip_now) {
            truth.emit(painted, json!({"ev": "strip", "shown": strip_now}));
            last_strip = Some(strip_now);
        }
        let covers = st.cover_rects();
        let ckey = format!("{covers:?}");
        if ckey != last_covers {
            truth.emit(painted, json!({"ev": "covers", "rects": covers.iter().map(|r| r.to_array()).collect::<Vec<_>>()}));
            last_covers = ckey;
        }
        for (req, slot) in pending_dropped.drain(..) {
            truth.emit(painted, json!({"ev": "dropped", "req": req, "slot": slot}));
        }
        for (s, dropped) in pending_sent.drain(..) {
            truth.emit(painted, json!({"ev": "sent", "kind": s.kind, "req": s.req, "slot": s.slot, "rows": s.rows, "bytes": s.bytes, "dropped": dropped}));
        }
        for m in pending_marks.drain(..) {
            truth.emit(painted, json!({"ev": "mark", "text": m}));
        }
        let mut items = plates_json(&plates, st.ui, &covers);
        if st.ui {
            if let Value::Array(a) = &mut items {
                for l in &st.layers {
                    a.extend(l.items.iter().cloned());
                }
            }
        }
        let layers = json!(st.layers.iter().map(|l| json!({"id": l.id, "meta": l.meta})).collect::<Vec<_>>());
        let lkey = format!("{}{}", st.ui, layers);
        // nothing to report while no scenario layer was ever shown
        if lkey != last_layers && !(last_layers.is_empty() && st.layers.is_empty()) {
            truth.emit(painted, json!({"ev": "layers", "shown": st.ui, "items": layers}));
            last_layers = lkey;
        }
        let ikey = items.to_string();
        if ikey != last_items {
            truth.emit(painted, json!({"ev": "plates", "items": items}));
            last_items = ikey;
        }

        // Self-checks while the picture is stable.
        let stable = now.duration_since(stable_since) >= Duration::from_millis(STABLE_MS);
        let due = last_check.elapsed() >= Duration::from_millis(CHECK_EVERY_MS) || checked_version != st.version;
        if stable && due && wgc_pending.is_none() {
            last_check = now;
            checked_version = st.version;
            if let Some(gr) = grab.as_mut() {
                let img = gr.grab(hwnd, w, h, layered);
                let (bad, worst) = compare(img, g.pixels(), w, h, None);
                stats_gdi.checks += 1;
                stats_gdi.pixel_mismatch += bad;
                stats_gdi.max_diff = stats_gdi.max_diff.max(worst);
                let before = (stats_gdi.tags_bad, stats_gdi.plates_bad, stats_gdi.strip_bad);
                verify_truth(img, w, &plates, &covers, &st, &lut, &mut stats_gdi);
                let after = (stats_gdi.tags_bad, stats_gdi.plates_bad, stats_gdi.strip_bad);
                truth.emit(epoch_ms(), json!({"ev": "selfcheck", "method": if layered { "gdi-printwindow" } else { "gdi-bitblt" }, "mismatch": bad, "maxDiff": worst, "truthOk": before == after}));
            }
            if wgc_cap.is_some() {
                wgc_pending = Some(WgcPending { version: st.version, deadline: now + Duration::from_millis(WGC_WAIT_MS), last: None });
            }
        }
        if let (Some(p), Some(cap)) = (wgc_pending.as_mut(), wgc_cap.as_mut()) {
            if p.version != st.version || !stable {
                wgc_pending = None; // the picture changed: not a failure, try again later
            } else {
                match cap.latest(&mut wgc_buf) {
                    Ok(true) if wgc_buf.len() == g.pixels().len() => {
                        let r = compare(&wgc_buf, g.pixels(), w, h, Some(anim_rect(w).inflate(2)));
                        p.last = Some(r);
                        if r.0 == 0 {
                            stats_wgc.checks += 1;
                            let before = (stats_wgc.tags_bad, stats_wgc.plates_bad, stats_wgc.strip_bad);
                            verify_truth(&wgc_buf, w, &plates, &covers, &st, &lut, &mut stats_wgc);
                            let after = (stats_wgc.tags_bad, stats_wgc.plates_bad, stats_wgc.strip_bad);
                            truth.emit(epoch_ms(), json!({"ev": "selfcheck", "method": "wgc", "mismatch": 0, "truthOk": before == after}));
                            wgc_pending = None;
                        }
                    }
                    Ok(true) => {
                        stats_wgc.errors += 1;
                        wgc_pending = None;
                    }
                    Ok(false) => {}
                    Err(_) => {
                        stats_wgc.errors += 1;
                        wgc_pending = None;
                    }
                }
                if let Some(p) = wgc_pending.as_ref() {
                    if Instant::now() >= p.deadline {
                        stats_wgc.checks += 1;
                        stats_wgc.timeouts += 1;
                        if let Some((bad, worst)) = p.last {
                            stats_wgc.pixel_mismatch += bad;
                            stats_wgc.max_diff = stats_wgc.max_diff.max(worst);
                        }
                        truth.emit(epoch_ms(), json!({"ev": "selfcheck", "method": "wgc", "timeout": true, "mismatch": p.last.map(|l| l.0)}));
                        wgc_pending = None;
                    }
                }
            }
        }

        if unsafe { GetForegroundWindow() } == hwnd {
            own_fg += 1;
        }
        if frame_no % 30 == 1 {
            exposure_checks += 1;
            exposed += exposed_points(hwnd, origin, w, h);
        }
        next += period;
        let after = Instant::now();
        if after > next + period {
            late += 1;
            next = after;
        } else if next > after {
            std::thread::sleep(next - after);
        }
    }
    let wall = start.elapsed().as_secs_f64();
    let cpu = cpu_seconds() - cpu0;
    let fg_after = unsafe { GetForegroundWindow() };
    let pass_gdi = !args.selfcheck || stats_gdi.pass();
    let pass_wgc = !args.wgc_check || stats_wgc.pass();
    let pass = pass_gdi && pass_wgc && own_fg == 0;
    let summary = json!({"fakewow": {
        "scenario": sc.name, "place": args.place.name(), "origin": [origin.0, origin.1], "client": [w, h],
        "frames": frame_no, "lateFrames": late, "wallS": (wall * 100.0).round() / 100.0,
        "cpuPctOneCore": ((cpu / wall.max(0.001)) * 1000.0).round() / 10.0,
        "foregroundChanged": fg_after != fg_before, "ownForegroundTicks": own_fg, "layered": layered,
        "exposedPoints": exposed, "exposureChecks": exposure_checks, "truthLines": truth.lines + 1,
        "selfcheck": {"gdi": if args.selfcheck { stats_gdi.json() } else { Value::Null },
                      "wgc": if args.wgc_check { let mut v = stats_wgc.json(); v["openError"] = json!(wgc_error); v["framesSeen"] = json!(wgc_cap.as_ref().map(|c| c.frames)); v } else { Value::Null }},
        "pass": pass,
    }});
    truth.emit(epoch_ms(), json!({"ev": "end", "summary": summary["fakewow"].clone()}));
    truth.flush();
    drop(wgc_cap);
    PAINT_DC.with(|p| *p.borrow_mut() = None);
    unsafe {
        let _ = ShowWindow(hwnd, SW_HIDE);
        let _ = DestroyWindow(hwnd);
    }
    // nothing is on screen from here on
    truth.emit(epoch_ms(), json!({"ev": "gone"}));
    truth.flush();
    pump();
    println!("{summary}");
    Ok(pass)
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("fakewow: {e}");
            }
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };
    if args.place == Place::Onscreen && game_running() {
        eprintln!("fakewow: refusing --place onscreen while the game is running (amendment A1); use offscreen or bottom");
        std::process::exit(3);
    }
    match run(args) {
        Ok(true) => std::process::exit(0),
        Ok(false) => std::process::exit(1),
        Err(e) => {
            eprintln!("fakewow: {e}");
            std::process::exit(2);
        }
    }
}
