//! Window-level smoke test of the whole Helper pipeline (ignored by default: it needs a desktop).
//!
//! A small window of class `TPMiniWow` (400 x 260, bottom-right of the primary work area, at the
//! bottom of the z-order, never activated, never topmost: amendment A1) shows a bridge strip that
//! carries a VIEW and a TRANSLATE, plus two tagged plate rows for that request. The real exe runs
//! against it with `--no-overlay --translator echo --capture wgc` and must decode, translate, find
//! the card and render it into memory, as recorded in its `--diag-json`.
//!
//! Run: `cargo test --offline --test window_pipeline -- --ignored`

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tp_bridge::strip::Mode;
use tp_bridge::synth::{self, Canvas, TagDraw};
use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::*;

const W: i32 = 400;
const H: i32 = 260;
const SESSION: u16 = 1234;

static FRAME: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static TICK: Mutex<u32> = Mutex::new(0);

fn messages() -> (Vec<u8>, Vec<u8>) {
    // VIEW: one chat view covering rows 100..250.
    let mut view = vec![0x14, 1, 1];
    for v in [0u16, 100, W as u16, 150] {
        view.extend_from_slice(&v.to_be_bytes());
    }
    view.extend_from_slice(&[14, 18]);
    // TRANSLATE reqId 5, slot 3, 2 rows, plate, target fr, synthetic text, no pairs.
    let text = b"synthetic test line";
    let mut tr = vec![0x10, 0, 5, 3, 2, 2, 2, b'f', b'r'];
    tr.extend_from_slice(&(text.len() as u16).to_be_bytes());
    tr.extend_from_slice(text);
    tr.push(0);
    (view, tr)
}

/// The window content: the strip (when `strip`), then two tagged rows of `slot`.
fn picture(strip: bool, slot: u8) -> Vec<u8> {
    let mut cv = Canvas::new(W, H, [24.0, 22.0, 20.0]);
    cv.aa = false;
    if strip {
        let (view, tr) = messages();
        let mut records = synth::chunk_records(1, &view, 200);
        records.extend(synth::chunk_records(2, &tr, 200));
        let fb = synth::frame(Mode::Dense, 1, SESSION, (W as u16, H as u16), 1, &records);
        let keys = synth::block_keys(&synth::to_symbols(&fb, Mode::Dense), Mode::Dense);
        synth::draw_strip(&mut cv, &keys, 0.0, 0.0);
    }
    for row in 0..2u8 {
        synth::draw_tag(
            &mut cv,
            &TagDraw {
                slot,
                row,
                sess_bit: (SESSION % 2) as u8,
                x: 12.0,
                y: 150.0 + f64::from(row) * 18.0,
                block_w: 6.0,
                block_h: 16.0,
                gap: 0.0,
                alpha: 1.0,
                plate_w: 300.0,
                colors: None,
            },
        );
    }
    let cap = cv.capture();
    cap.buf
}

unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => LRESULT(1),
        WM_TIMER => unsafe {
            // Keep frames coming: WGC only delivers a frame when the window content changes.
            if let Ok(mut t) = TICK.lock() {
                *t += 1;
            }
            let _ = InvalidateRect(Some(hwnd), None, false);
            LRESULT(0)
        },
        WM_PAINT => unsafe {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut ps);
            let mut px = FRAME.lock().map(|p| p.clone()).unwrap_or_default();
            let t = TICK.lock().map(|t| *t).unwrap_or(0);
            // Flicker one pixel in the bottom-right corner (outside the chat view and the strip).
            let i = ((H as usize - 1) * W as usize + W as usize - 1) * 4;
            if px.len() > i + 2 {
                px[i] = (t % 2 * 255) as u8;
            }
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
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        },
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

static SERIAL: Mutex<()> = Mutex::new(());

/// Shows the window with `picture`, runs the exe against it for 6 s, returns its diag records.
fn run_scenario(picture: Vec<u8>) -> Vec<serde_json::Value> {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    *FRAME.lock().unwrap() = picture;
    let wa = tp_helper::wow::primary_work_area();
    let hwnd = unsafe {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(proc_),
            hInstance: tp_helper::overlay::window::hinstance(),
            lpszClassName: w!("TPMiniWow"),
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let h = CreateWindowExW(WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW, w!("TPMiniWow"), w!("mini"), WS_POPUP, wa.right - W - 8, wa.bottom - H - 8, W, H, None, None, None, None).unwrap();
        let _ = SetWindowPos(h, Some(HWND_BOTTOM), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
        SetTimer(Some(h), 1, 40, None);
        h
    };
    let fg_before = tp_helper::wow::foreground_root();

    let dir = std::env::temp_dir().join(format!("tp_window_pipeline_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let diag = dir.join("diag.jsonl");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_TranslateProbeHelper"))
        .args(["--wow-class", "TPMiniWow", "--no-overlay", "--no-tray", "--translator", "echo", "--capture", "wgc", "--exit-after", "6"])
        .arg("--diag-json")
        .arg(&diag)
        .arg("--log-dir")
        .arg(dir.join("logs"))
        .env("APPDATA", dir.join("appdata"))
        .env("LOCALAPPDATA", dir.join("localappdata"))
        .spawn()
        .unwrap();
    let t0 = Instant::now();
    let mut code = None;
    while t0.elapsed() < Duration::from_secs(20) {
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if let Some(c) = child.try_wait().unwrap() {
            code = Some(c);
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if code.is_none() {
        let _ = child.kill();
    }
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
    assert_eq!(tp_helper::wow::foreground_root(), fg_before, "focus never moves");
    assert!(code.is_some_and(|c| c.success()), "helper exit: {code:?}");
    let text = std::fs::read_to_string(&diag).unwrap();
    let events: Vec<serde_json::Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    // Nothing of the message text in any file.
    for f in [diag.clone(), dir.join("logs").join("helper.log")] {
        let body = std::fs::read_to_string(&f).unwrap_or_default();
        assert!(!body.to_lowercase().contains("synthetic test line"), "{} leaks text", f.display());
    }
    let _ = std::fs::remove_dir_all(&dir);
    events
}

fn ev<'a>(events: &'a [serde_json::Value], name: &'static str) -> impl Iterator<Item = &'a serde_json::Value> {
    events.iter().filter(move |e| e["ev"] == name)
}

#[test]
#[ignore]
fn helper_reads_a_window_end_to_end() {
    let events = run_scenario(picture(true, 3));
    assert!(ev(&events, "frame").any(|e| e["ok"] == true && e["session"] == SESSION), "strip decoded");
    assert!(ev(&events, "msg").any(|e| e["kind"] == "view"), "VIEW received");
    assert!(ev(&events, "msg").any(|e| e["kind"] == "translate" && e["req"] == 5 && e["slot"] == 3 && e["rows"] == 2), "TRANSLATE received");
    assert!(ev(&events, "engine").any(|e| e["req"] == 5 && e["status"] == "done"), "echo translation done");
    let card = ev(&events, "cards")
        .flat_map(|e| e["cards"].as_array().cloned().unwrap_or_default())
        .find(|c| c["slot"] == 3 && c["state"] == "done")
        .expect("a card for slot 3 in state done");
    assert_eq!(card["kind"], "request");
    assert_eq!(card["req"], 5);
    assert_eq!(card["view"], 1);
    assert_eq!(card["bands"].as_array().unwrap().len(), 2, "{card}");
    assert_eq!(card["covered"], false);
    assert!(ev(&events, "overlay").any(|e| e["shown"] == true && e["window"] == false && e["textPx"].as_u64().unwrap_or(0) > 0), "rendered into memory");
}

/// The Helper starts after the click: no strip ever shows, no VIEW. The discovery scan finds the
/// tags and the card says "Not received" after 1.5 s.
#[test]
#[ignore]
fn late_start_shows_missing() {
    let events = run_scenario(picture(false, 9));
    assert!(!ev(&events, "msg").any(|_| true), "no message without a strip");
    let card = ev(&events, "cards")
        .flat_map(|e| e["cards"].as_array().cloned().unwrap_or_default())
        .find(|c| c["slot"] == 9 && c["kind"] == "missing")
        .expect("a missing card for slot 9");
    assert_eq!(card["view"], 0, "discovery view");
    assert_eq!(card["bands"].as_array().unwrap().len(), 2, "{card}");
    assert!(ev(&events, "overlay").any(|e| e["shown"] == true && e["textPx"].as_u64().unwrap_or(0) > 0), "status drawn into memory");
}
