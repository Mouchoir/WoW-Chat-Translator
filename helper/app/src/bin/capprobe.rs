//! capprobe: opens its own small window with known colours and checks that the Helper's two
//! capture paths (Windows.Graphics.Capture and GDI) read them exactly.
//!
//! Amendment A1 (never disturb the game): every window is 320 x 200, never activated, and the
//! topmost ones are shown only in the bottom-right corner of the primary monitor's work area, for
//! less than 5 s in total. Two extra phases inform the E2E rig and are not part of the verdict:
//! - `offscreen`: the window moved beyond the right edge of the virtual screen (WGC only);
//! - `bottom`: the window at the bottom of the z-order, not topmost (WGC only).
//!
//! Prints one JSON line; exit code 0 when the on-screen phases read every pixel exactly.

#![allow(non_snake_case)]

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tp_bridge::image::Rect;
use tp_helper::capture::gdi::GdiGrabber;
use tp_helper::capture::wgc::{self, WgcGrabber};
use tp_bridge::image::Bgra;
use tp_helper::capture::{GrabStatus, Grabber};
use tp_helper::wow;
use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::*;

const W: i32 = 320;
const H: i32 = 200;
/// Total time topmost windows may be on screen (amendment A1 says at most 5 s).
const TOPMOST_BUDGET: Duration = Duration::from_millis(4500);

/// Pixels painted by the window (BGRA, W x H).
static PIXELS: Mutex<Vec<u8>> = Mutex::new(Vec::new());

/// Colours that matter to the bridge: strip levels, tag colours, the plate, then pseudo-random.
fn pattern(seed: u32) -> Vec<u8> {
    let fixed: [[u8; 3]; 13] = [
        [0, 0, 0],
        [104, 104, 104],
        [146, 146, 146],
        [255, 255, 255],
        [255, 0, 255],
        [0, 255, 0],
        [255, 0, 0],
        [0, 0, 255],
        [255, 255, 0],
        [0, 255, 255],
        [32, 40, 64],
        [104, 146, 255],
        [146, 0, 104],
    ];
    let mut out = vec![0u8; (W * H * 4) as usize];
    let mut rng = seed.wrapping_mul(2_654_435_761).max(1);
    for cy in 0..(H + 7) / 8 {
        for cx in 0..(W + 7) / 8 {
            let cell = (cy * ((W + 7) / 8) + cx) as usize;
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            let c = if (cell + seed as usize) % 3 != 0 {
                fixed[(cell + seed as usize) % fixed.len()]
            } else {
                [(rng & 0xFF) as u8, ((rng >> 8) & 0xFF) as u8, ((rng >> 16) & 0xFF) as u8]
            };
            for y in cy * 8..(cy * 8 + 8).min(H) {
                for x in cx * 8..(cx * 8 + 8).min(W) {
                    let i = ((y * W + x) * 4) as usize;
                    out[i] = c[2];
                    out[i + 1] = c[1];
                    out[i + 2] = c[0];
                    out[i + 3] = 255;
                }
            }
        }
    }
    out
}

unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => unsafe {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut ps);
            let px = PIXELS.lock().map(|p| p.clone()).unwrap_or_default();
            if px.len() == (W * H * 4) as usize {
                let bmi = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: W,
                        biHeight: -H,
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB.0,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                SetDIBitsToDevice(dc, 0, 0, W as u32, H as u32, 0, 0, 0, H as u32, px.as_ptr() as *const _, &bmi, DIB_RGB_COLORS);
            }
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        },
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

fn pump(d: Duration) {
    let until = Instant::now() + d;
    unsafe {
        let mut msg = MSG::default();
        loop {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if Instant::now() >= until {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

fn set_pattern(hwnd: HWND, seed: u32) -> Vec<u8> {
    let p = pattern(seed);
    if let Ok(mut g) = PIXELS.lock() {
        *g = p.clone();
    }
    unsafe {
        let _ = InvalidateRect(Some(hwnd), None, false);
        let _ = UpdateWindow(hwnd);
    }
    p
}

/// (pixels compared, mismatching pixels, largest channel difference)
fn compare(img: &Bgra, expected: &[u8]) -> (usize, usize, u8) {
    if img.width as i32 != W || img.height as i32 != H || img.origin_x != 0 || img.origin_y != 0 {
        return (0, usize::MAX, 255);
    }
    let mut bad = 0;
    let mut worst = 0u8;
    let row = W as usize * 4;
    for y in 0..H as usize {
        let got = &img.px[y * img.stride as usize..y * img.stride as usize + row];
        for (a, b) in got.chunks_exact(4).zip(expected[y * row..(y + 1) * row].chunks_exact(4)) {
            let d = (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0);
            if d != 0 {
                bad += 1;
                worst = worst.max(d);
            }
        }
    }
    (expected.len() / 4, bad, worst)
}

fn region() -> Vec<Rect> {
    vec![Rect { x: 0, y: 0, w: W, h: H }]
}

fn class_of(h: isize) -> String {
    let mut buf = [0u16; 128];
    let n = unsafe { GetClassNameW(HWND(h as *mut core::ffi::c_void), &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// GDI read of the client; retried for up to 300 ms while the compositor catches up.
fn gdi_check(hwnd: HWND, expected: &[u8]) -> Value {
    let mut g = GdiGrabber::new();
    let mut last = json!({"ok": false, "error": "grab"});
    for attempt in 0..6 {
        if attempt > 0 {
            pump(Duration::from_millis(50));
        }
        let Some(info) = wow::describe(hwnd.0 as isize) else { return json!({"ok": false, "error": "describe"}) };
        // What is on top at the client centre (a window covering ours makes GDI read it instead).
        let top = wow::root_at(info.origin_x + W / 2, info.origin_y + H / 2);
        if g.grab(&info, &region()) != GrabStatus::Fresh {
            continue;
        }
        let Some(img) = g.image(0) else { continue };
        let (n, bad, worst) = compare(&img, expected);
        let own = top == hwnd.0 as isize;
        last = json!({"ok": bad == 0 && n > 0, "px": n, "mismatch": bad, "maxDiff": worst, "attempts": attempt + 1,
                      "ownWindowOnTop": own, "topClass": if own { String::new() } else { class_of(top) }});
        if bad == 0 && n > 0 {
            break;
        }
    }
    last
}

/// Reads frames until one matches `expected` or `timeout` passes.
fn wgc_wait(g: &mut WgcGrabber, hwnd: HWND, expected: &[u8], timeout: Duration) -> Value {
    let t0 = Instant::now();
    let mut fresh = 0;
    let mut last: Option<(usize, usize, u8)> = None;
    while t0.elapsed() < timeout {
        pump(Duration::from_millis(10));
        let Some(info) = wow::describe(hwnd.0 as isize) else { break };
        if g.grab(&info, &region()) == GrabStatus::Fresh {
            fresh += 1;
            if let Some(img) = g.image(0) {
                let r = compare(&img, expected);
                last = Some(r);
                if r.1 == 0 && r.0 > 0 {
                    return json!({"ok": true, "px": r.0, "mismatch": 0, "maxDiff": 0, "freshFrames": fresh, "ms": t0.elapsed().as_millis() as u64,
                                  "frameOffset": [info.frame_dx, info.frame_dy]});
                }
            }
        }
    }
    match last {
        Some((n, bad, worst)) => json!({"ok": false, "px": n, "mismatch": bad, "maxDiff": worst, "freshFrames": fresh}),
        None => json!({"ok": false, "error": "no fresh frame", "freshFrames": fresh}),
    }
}

/// WGC on the window: the first pattern, then a second one after a repaint.
fn wgc_phase(hwnd: HWND, seed: u32, wait: Duration) -> Value {
    if !wgc::supported() {
        return json!({"ok": false, "error": "unsupported"});
    }
    let t0 = Instant::now();
    let mut g = match WgcGrabber::open(hwnd.0 as isize, 0) {
        Ok(g) => g,
        Err(e) => return json!({"ok": false, "error": format!("open 0x{:08X}", e.code().0)}),
    };
    let open_ms = t0.elapsed().as_millis() as u64;
    let p1 = set_pattern(hwnd, seed);
    let first = wgc_wait(&mut g, hwnd, &p1, wait);
    let p2 = set_pattern(hwnd, seed + 1);
    let second = wgc_wait(&mut g, hwnd, &p2, wait);
    let ok = first["ok"].as_bool() == Some(true) && second["ok"].as_bool() == Some(true);
    json!({"ok": ok, "openMs": open_ms, "first": first, "afterRepaint": second})
}

fn create(style: WINDOW_STYLE, ex: WINDOW_EX_STYLE, x: i32, y: i32) -> Option<HWND> {
    unsafe {
        let mut rc = RECT { left: 0, top: 0, right: W, bottom: H };
        let _ = AdjustWindowRectEx(&mut rc, style, false, ex);
        let h = CreateWindowExW(ex, w!("TPCapProbe"), w!("capprobe"), style, x, y, rc.right - rc.left, rc.bottom - rc.top, None, None, None, None).ok()?;
        // Windows 11 rounds window corners, which would hide the client's corner pixels.
        let pref = DWMWCP_DONOTROUND;
        let _ = DwmSetWindowAttribute(h, DWMWA_WINDOW_CORNER_PREFERENCE, &pref as *const _ as *const core::ffi::c_void, std::mem::size_of_val(&pref) as u32);
        Some(h)
    }
}

/// Top-left position that puts a window of this style in the bottom-right corner of the work area.
fn bottom_right(style: WINDOW_STYLE, ex: WINDOW_EX_STYLE) -> (i32, i32) {
    let wa = wow::primary_work_area();
    let mut rc = RECT { left: 0, top: 0, right: W, bottom: H };
    unsafe {
        let _ = AdjustWindowRectEx(&mut rc, style, false, ex);
    }
    (wa.right - (rc.right - rc.left) - 8, wa.bottom - (rc.bottom - rc.top) - 8)
}

/// A topmost window in the bottom-right corner: GDI, WGC, then GDI again after a repaint.
fn on_screen_phase(name: &str, style: WINDOW_STYLE, seed: u32, used: &mut Duration) -> Value {
    if TOPMOST_BUDGET.saturating_sub(*used) < Duration::from_millis(2000) {
        return json!({"name": name, "skipped": "topmost time budget"});
    }
    let ex = WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW;
    let (x, y) = bottom_right(style, ex);
    let Some(hwnd) = create(style, ex, x, y) else { return json!({"name": name, "error": "create"}) };
    let p = set_pattern(hwnd, seed);
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
    let shown_at = Instant::now();
    pump(Duration::from_millis(100));
    // GDI reads the screen: if another topmost window sits on this corner, try the next spot of
    // the bottom-right area (still well inside the corner region).
    let mut spot = 0;
    let mut blocker = String::new();
    for (i, (dx, dy)) in [(0, 0), (-(W + 24), 0), (0, -(H + 48)), (-(W + 24), -(H + 48))].iter().enumerate() {
        if i > 0 {
            unsafe {
                let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x + dx, y + dy, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
            }
            pump(Duration::from_millis(80));
        }
        let Some(info) = wow::describe(hwnd.0 as isize) else { break };
        let other = (0..3)
            .flat_map(|i| (0..3).map(move |j| (i, j)))
            .map(|(i, j)| wow::root_at(info.origin_x + (W - 1) * (1 + 2 * i) / 6, info.origin_y + (H - 1) * (1 + 2 * j) / 6))
            .find(|h| *h != hwnd.0 as isize);
        spot = i;
        match other {
            None => break,
            Some(h) => blocker = class_of(h),
        }
    }
    let gdi = gdi_check(hwnd, &p);
    let wgc = wgc_phase(hwnd, seed + 10, Duration::from_millis(800));
    let p3 = set_pattern(hwnd, seed + 20);
    let gdi2 = gdi_check(hwnd, &p3);
    let info = wow::describe(hwnd.0 as isize);
    unsafe {
        let _ = ShowWindow(hwnd, SW_HIDE);
        let _ = DestroyWindow(hwnd);
    }
    let visible = shown_at.elapsed();
    *used += visible;
    let ok = gdi["ok"].as_bool() == Some(true) && gdi2["ok"].as_bool() == Some(true) && wgc["ok"].as_bool() == Some(true);
    json!({"name": name, "ok": ok, "visibleMs": visible.as_millis() as u64, "gdi": gdi, "gdiAfterRepaint": gdi2, "wgc": wgc,
           "frameOffset": info.map(|i| [i.frame_dx, i.frame_dy]), "spot": spot, "coveredBy": blocker})
}

/// A window that is not topmost: off the virtual screen, or at the bottom of the z-order.
fn hidden_phase(name: &str, x: i32, y: i32, seed: u32) -> Value {
    let ex = WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW;
    let Some(hwnd) = create(WS_POPUP, ex, x, y) else { return json!({"name": name, "error": "create"}) };
    set_pattern(hwnd, seed);
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_BOTTOM), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
    }
    pump(Duration::from_millis(100));
    let info = wow::describe(hwnd.0 as isize);
    let on_monitor = info.map(|i| wow::point_on_monitor(i.origin_x + W / 2, i.origin_y + H / 2));
    let covered = info.map(|i| wow::rect_covered(i.hwnd, i.origin_x, i.origin_y, W, H));
    let wgc = wgc_phase(hwnd, seed + 1, Duration::from_millis(1500));
    unsafe {
        let _ = ShowWindow(hwnd, SW_HIDE);
        let _ = DestroyWindow(hwnd);
    }
    json!({"name": name, "wgc": wgc, "centreOnMonitor": on_monitor, "coveredTest": covered, "origin": info.map(|i| [i.origin_x, i.origin_y])})
}

fn main() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(proc_),
            hInstance: tp_helper::overlay::window::hinstance(),
            lpszClassName: w!("TPCapProbe"),
            ..Default::default()
        };
        RegisterClassExW(&wc);
    }
    tp_helper::app::attach_console();
    let fg_before = wow::foreground_root();
    let t0 = Instant::now();
    let mut topmost_used = Duration::ZERO;

    let popup = on_screen_phase("popup", WS_POPUP, 1, &mut topmost_used);
    let framed = on_screen_phase("captioned", WS_POPUP | WS_CAPTION | WS_BORDER, 30, &mut topmost_used);
    let (vx, vy, vw, _) = wow::virtual_screen();
    let offscreen = hidden_phase("offscreen", vx + vw + 100, vy + 100, 60);
    let (bx, by) = bottom_right(WS_POPUP, WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW);
    let bottom = hidden_phase("bottom", bx, by, 90);

    let fg_changed = wow::foreground_root() != fg_before;
    let pass = popup["ok"].as_bool() == Some(true) && framed["ok"].as_bool() == Some(true) && !fg_changed;
    let out = json!({
        "probe": "capture", "pass": pass, "foregroundChanged": fg_changed,
        "topmostVisibleMs": topmost_used.as_millis() as u64, "totalMs": t0.elapsed().as_millis() as u64,
        "offscreenWgc": offscreen["wgc"]["ok"], "bottomWgc": bottom["wgc"]["ok"],
        "phases": [popup, framed, offscreen, bottom],
    });
    println!("{out}");
    std::process::exit(if pass { 0 } else { 1 });
}
