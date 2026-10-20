//! Finding the game window and reading its geometry. Never opens a handle to the game process:
//! only window queries (class, rectangles, foreground, minimized).

use windows::core::{PCWSTR, HSTRING};
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::{ClientToScreen, GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONULL, MONITOR_DEFAULTTOPRIMARY};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Window class of the WoW client.
pub const WOW_CLASS: &str = "waApplication Window";

/// Geometry of the game window for one capture tick. All values are physical px.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WowInfo {
    pub hwnd: isize,
    /// Client origin in screen coordinates.
    pub origin_x: i32,
    pub origin_y: i32,
    pub client_w: i32,
    pub client_h: i32,
    pub minimized: bool,
    pub foreground: bool,
    /// Offset of the client origin inside a Windows.Graphics.Capture frame (client origin minus
    /// the DWM extended frame bounds origin; 0 for a borderless window).
    pub frame_dx: i32,
    pub frame_dy: i32,
    pub dpi: u32,
}

pub fn hwnd(h: isize) -> HWND {
    HWND(h as *mut core::ffi::c_void)
}

pub struct WowFinder {
    class: HSTRING,
    custom: bool,
}

impl WowFinder {
    /// `class`: the `--wow-class` test class, or None for the WoW class.
    pub fn new(class: Option<&str>) -> WowFinder {
        WowFinder { class: HSTRING::from(class.unwrap_or(WOW_CLASS)), custom: class.is_some() }
    }

    /// True for a `--wow-class` test window.
    pub fn is_custom(&self) -> bool {
        self.custom
    }

    /// The first visible top-level window of the class.
    pub fn find(&self) -> Option<isize> {
        let mut after: Option<HWND> = None;
        for _ in 0..64 {
            let h = unsafe { FindWindowExW(None, after, PCWSTR(self.class.as_ptr()), PCWSTR::null()) }.ok()?;
            if h.0.is_null() {
                return None;
            }
            if unsafe { IsWindowVisible(h) }.as_bool() {
                return Some(h.0 as isize);
            }
            after = Some(h);
        }
        None
    }
}

pub fn is_window(h: isize) -> bool {
    unsafe { IsWindow(Some(hwnd(h))) }.as_bool()
}

/// The root window that currently has the foreground.
pub fn foreground_root() -> isize {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.0.is_null() {
            return 0;
        }
        GetAncestor(fg, GA_ROOT).0 as isize
    }
}

/// Reads the window's geometry; None when it is gone or its client area cannot be read.
pub fn describe(h: isize) -> Option<WowInfo> {
    describe_cached(h, &mut FrameBoundsCache::default())
}

/// The DWM extended frame bounds of the last window seen, keyed by its window rectangle and
/// client origin: the DWM query is the costly part of [`describe`] and the bounds only change
/// when the window moves, resizes or changes style (which changes the key).
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameBoundsCache {
    key: Option<(isize, [i32; 4], [i32; 2], [i32; 2])>,
    efb: [i32; 2],
}

/// [`describe`] with the frame bounds cached across calls.
pub fn describe_cached(h: isize, cache: &mut FrameBoundsCache) -> Option<WowInfo> {
    let w = hwnd(h);
    unsafe {
        if !IsWindow(Some(w)).as_bool() {
            return None;
        }
        let minimized = IsIconic(w).as_bool();
        let mut rc = RECT::default();
        GetClientRect(w, &mut rc).ok()?;
        let mut origin = POINT { x: 0, y: 0 };
        if !ClientToScreen(w, &mut origin).as_bool() {
            return None;
        }
        let mut wr = RECT::default();
        let _ = GetWindowRect(w, &mut wr);
        let key = (h, [wr.left, wr.top, wr.right, wr.bottom], [origin.x, origin.y], [rc.right, rc.bottom]);
        let mut efb = RECT { left: cache.efb[0], top: cache.efb[1], ..Default::default() };
        if cache.key != Some(key) {
            let have_efb = DwmGetWindowAttribute(
                w,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                &mut efb as *mut RECT as *mut core::ffi::c_void,
                std::mem::size_of::<RECT>() as u32,
            )
            .is_ok();
            if !have_efb {
                efb = wr;
            }
            *cache = FrameBoundsCache { key: Some(key), efb: [efb.left, efb.top] };
        }
        Some(WowInfo {
            hwnd: h,
            origin_x: origin.x,
            origin_y: origin.y,
            client_w: rc.right - rc.left,
            client_h: rc.bottom - rc.top,
            minimized,
            foreground: foreground_root() == h,
            frame_dx: origin.x - efb.left,
            frame_dy: origin.y - efb.top,
            dpi: GetDpiForWindow(w),
        })
    }
}

/// True when the screen point lies on a monitor.
pub fn point_on_monitor(x: i32, y: i32) -> bool {
    unsafe { !MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONULL).0.is_null() }
}

/// Root window under a screen point (layered click-through windows such as our overlays are skipped
/// by `WindowFromPoint`).
pub fn root_at(x: i32, y: i32) -> isize {
    unsafe {
        let h = WindowFromPoint(POINT { x, y });
        if h.0.is_null() {
            return 0;
        }
        GetAncestor(h, GA_ROOT).0 as isize
    }
}

/// True for a layered window of this process that the mouse can reach: the fallback badge's
/// hover target (`crate::badge`), which sits over a card's last line. The overlay and the toast
/// are click-through, so `WindowFromPoint` never returns them.
pub fn is_own_hit_target(root: isize) -> bool {
    if root == 0 {
        return false;
    }
    let h = hwnd(root);
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(h, Some(&mut pid as *mut u32));
    }
    pid == std::process::id() && (unsafe { GetWindowLongW(h, GWL_EXSTYLE) } as u32 & WS_EX_LAYERED.0) != 0
}

/// Covering test (design D.3 step 5): a 3x3 grid of points over `rect` (screen coordinates) must
/// all resolve to `target`. A point on no monitor cannot be covered by anything (and nothing we
/// draw there is visible), so it counts as uncovered: this keeps an off-screen test window usable.
/// A point on our own fallback badge says nothing about the card under it and is skipped (else
/// the badge would hide its own card).
pub fn rect_covered(target: isize, x: i32, y: i32, w: i32, h: i32) -> bool {
    for i in 0..3 {
        for j in 0..3 {
            let px = x + (w.max(1) - 1) * (1 + 2 * i) / 6;
            let py = y + (h.max(1) - 1) * (1 + 2 * j) / 6;
            if !point_on_monitor(px, py) {
                continue;
            }
            let root = root_at(px, py);
            if root != target && !is_own_hit_target(root) {
                return true;
            }
        }
    }
    false
}

/// Full covering test (decision D12, the game window in the background): true when every point
/// of the same 3x3 grid that lies on a monitor resolves to another window than `target` (a card
/// partly under another window is still drawn: the overlay then sits right above the game window,
/// so that window hides the covered part itself). False when no point lies on a monitor.
pub fn rect_fully_covered(target: isize, x: i32, y: i32, w: i32, h: i32) -> bool {
    let mut on_monitor = 0;
    for i in 0..3 {
        for j in 0..3 {
            let px = x + (w.max(1) - 1) * (1 + 2 * i) / 6;
            let py = y + (h.max(1) - 1) * (1 + 2 * j) / 6;
            if !point_on_monitor(px, py) {
                continue;
            }
            let root = root_at(px, py);
            if is_own_hit_target(root) {
                continue;
            }
            on_monitor += 1;
            if root == target {
                return false;
            }
        }
    }
    on_monitor > 0
}

/// True when the game window is iconic (minimized), for the UI thread's quick check.
pub fn is_minimized(h: isize) -> bool {
    unsafe { IsIconic(hwnd(h)) }.as_bool()
}

/// The virtual screen (every monitor) in screen coordinates: (x, y, w, h).
pub fn virtual_screen() -> (i32, i32, i32, i32) {
    unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

/// Work area (without the taskbar) of the primary monitor.
pub fn primary_work_area() -> RECT {
    unsafe {
        let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        mi.rcWork
    }
}
