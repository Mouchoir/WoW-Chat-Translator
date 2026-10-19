//! Layered, click-through windows that never activate (overlay and toast), and the in-memory DIBs
//! they are drawn from.
//!
//! Styles: `WS_EX_LAYERED|TRANSPARENT|TOPMOST|NOACTIVATE|TOOLWINDOW`; `WM_MOUSEACTIVATE` returns
//! `MA_NOACTIVATE`, `WM_NCHITTEST` returns `HTTRANSPARENT`; shown with `SetWindowPos(..,
//! SWP_NOACTIVATE | SWP_SHOWWINDOW)` only; hidden with `SW_HIDE` when empty.
//!
//! Z-order (decision D12, [`ZMode`]): topmost while the game window is in front; otherwise right
//! above the game window (`SetWindowPos` after the first window above it that is not ours,
//! `SWP_NOACTIVATE`), which also takes the topmost status away. The windows above the game, the
//! one in front among them, then hide the overlay pixel for pixel: a card is never drawn over
//! another application. The z-order is set in the same call that shows a window, so a window
//! never appears topmost first.

use std::sync::Once;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

use tp_bridge::image::Rect;

use super::render::Canvas;

pub const OVERLAY_CLASS: PCWSTR = w!("TPHelperOverlay");
pub const TOAST_CLASS: PCWSTR = w!("TPHelperToast");

unsafe extern "system" fn layered_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        // `taskkill` without /F posts WM_CLOSE to one window of the process only: the first
        // top-level window in z-order, which is a topmost overlay or toast window whenever one
        // exists (measured, see docs\HELPER.md). Exit like the tray's Exit (the UI thread owns
        // these windows) instead of letting DefWindowProc destroy just this window.
        WM_CLOSE => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

pub fn hinstance() -> HINSTANCE {
    unsafe { GetModuleHandleW(PCWSTR::null()).map(|m| m.into()).unwrap_or_default() }
}

/// Registers `TPHelperOverlay` and `TPHelperToast` once per process.
pub fn register_classes() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        for class in [OVERLAY_CLASS, TOAST_CLASS] {
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(layered_proc),
                hInstance: hinstance(),
                lpszClassName: class,
                ..Default::default()
            };
            RegisterClassExW(&wc);
        }
    });
}

/// A 32-bit top-down DIB selected into its own memory DC.
pub struct Dib {
    pub dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    pub w: i32,
    pub h: i32,
}

impl Dib {
    pub fn new(w: i32, h: i32) -> Option<Dib> {
        let (w, h) = (w.max(1), h.max(1));
        unsafe {
            let dc = CreateCompatibleDC(None);
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
            let bmp = match CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(b) if !bits.is_null() => b,
                _ => {
                    let _ = DeleteDC(dc);
                    return None;
                }
            };
            let old = SelectObject(dc, bmp.into());
            Some(Dib { dc, bmp, old, bits: bits as *mut u8, w, h })
        }
    }

    pub fn bits(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.bits, (self.w * self.h * 4) as usize) }
    }

    pub fn bits_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.bits, (self.w * self.h * 4) as usize) }
    }

    /// Copies a canvas of the same size (premultiplied BGRA).
    pub fn copy_from(&mut self, c: &Canvas) -> bool {
        if c.rect.w != self.w || c.rect.h != self.h {
            return false;
        }
        let _ = unsafe { GdiFlush() };
        self.bits_mut().copy_from_slice(&c.px);
        true
    }

    /// Copies the part `r` (canvas-local coordinates, clipped) of a canvas of the same size.
    pub fn copy_region(&mut self, c: &Canvas, r: Rect) -> bool {
        if c.rect.w != self.w || c.rect.h != self.h {
            return false;
        }
        let (x0, y0) = (r.x.max(0), r.y.max(0));
        let (x1, y1) = ((r.x + r.w).min(self.w), (r.y + r.h).min(self.h));
        if x1 <= x0 || y1 <= y0 {
            return true;
        }
        let _ = unsafe { GdiFlush() };
        let row = self.w as usize * 4;
        let (a, b) = (x0 as usize * 4, x1 as usize * 4);
        let bits = self.bits_mut();
        for y in y0 as usize..y1 as usize {
            bits[y * row + a..y * row + b].copy_from_slice(&c.px[y * row + a..y * row + b]);
        }
        true
    }
}

/// The part of a canvas that changed since the previous one of the same size: the union of both
/// dirty rectangles, in canvas-local coordinates (outside both, both are transparent).
pub fn changed_region(canvas: &Canvas, prev_dirty: Option<Rect>) -> Option<Rect> {
    let local = |r: Rect| Rect { x: r.x - canvas.rect.x, y: r.y - canvas.rect.y, w: r.w, h: r.h };
    let now = canvas.dirty().map(local);
    match (now, prev_dirty) {
        (Some(a), Some(b)) => Some(crate::capture::union(a, b)),
        (a, b) => a.or(b),
    }
}

impl Drop for Dib {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bmp.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

/// Where the overlay windows sit in the z-order (decision D12).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZMode {
    /// The game window is in front: above everything that is not topmost.
    Topmost,
    /// Another window is in front: right above the game window (its handle).
    Above(isize),
}

/// True when `h` belongs to this process (the overlay, the toast, the main window).
fn ours(h: HWND) -> bool {
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(h, Some(&mut pid as *mut u32));
    }
    pid == std::process::id()
}

fn is_topmost(h: HWND) -> bool {
    (unsafe { GetWindowLongW(h, GWL_EXSTYLE) } as u32 & WS_EX_TOPMOST.0) != 0
}

/// The window the overlay goes right after (below) in mode `z`: `HWND_TOPMOST`; or the first
/// window above the game window that is not ours (`HWND_NOTOPMOST`, the top of the non-topmost
/// band, when that window is topmost or there is none: the game window is then the highest window
/// that is not topmost). None when the game window is gone.
pub fn insert_after(z: ZMode) -> Option<HWND> {
    insert_after_with(z, &ours)
}

fn insert_after_with(z: ZMode, is_ours: &dyn Fn(HWND) -> bool) -> Option<HWND> {
    match z {
        ZMode::Topmost => Some(HWND_TOPMOST),
        ZMode::Above(wow) => {
            let wow = HWND(wow as *mut core::ffi::c_void);
            if !unsafe { IsWindow(Some(wow)) }.as_bool() {
                return None;
            }
            let mut p = unsafe { GetWindow(wow, GW_HWNDPREV) }.unwrap_or_default();
            let mut guard = 0;
            while !p.0.is_null() && is_ours(p) && guard < 64 {
                p = unsafe { GetWindow(p, GW_HWNDPREV) }.unwrap_or_default();
                guard += 1;
            }
            if p.0.is_null() || is_topmost(p) {
                Some(HWND_NOTOPMOST)
            } else {
                Some(p)
            }
        }
    }
}

/// True when every window between the game window and the first window above it that is not
/// ours is ours, and `h` is among them (the overlay sits right above the game window).
pub fn right_above(h: HWND, wow: isize) -> bool {
    right_above_with(h, wow, &ours)
}

fn right_above_with(h: HWND, wow: isize, is_ours: &dyn Fn(HWND) -> bool) -> bool {
    let wow = HWND(wow as *mut core::ffi::c_void);
    let mut p = unsafe { GetWindow(wow, GW_HWNDPREV) }.unwrap_or_default();
    let mut guard = 0;
    while !p.0.is_null() && is_ours(p) && guard < 64 {
        if p == h {
            return true;
        }
        p = unsafe { GetWindow(p, GW_HWNDPREV) }.unwrap_or_default();
        guard += 1;
    }
    false
}

/// One layered window and its backing DIB.
pub struct LayeredWindow {
    hwnd: HWND,
    shown: bool,
    exclude: Option<bool>,
    dib: Option<Dib>,
    pos: (i32, i32, i32, i32),
    /// Dirty rectangle (canvas-local) of the last canvas presented.
    prev_dirty: Option<Rect>,
    /// The z-order mode last applied.
    z: ZMode,
}

impl LayeredWindow {
    pub fn create(class: PCWSTR) -> Option<LayeredWindow> {
        register_classes();
        let ex = WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW;
        let hwnd = unsafe { CreateWindowExW(ex, class, w!("Translate Companion"), WS_POPUP, 0, 0, 1, 1, None, None, Some(hinstance()), None) }.ok()?;
        Some(LayeredWindow { hwnd, shown: false, exclude: None, dib: None, pos: (0, 0, 0, 0), prev_dirty: None, z: ZMode::Topmost })
    }

    /// Puts a shown window where mode `z` wants it (nothing for a hidden one: `present_z` places
    /// it when it shows it). Returns true when the window was moved in the z-order.
    pub fn apply_z(&mut self, z: ZMode) -> bool {
        let need = match z {
            ZMode::Topmost => self.z != z || !is_topmost(self.hwnd),
            ZMode::Above(wow) => self.z != z || is_topmost(self.hwnd) || !right_above(self.hwnd, wow),
        };
        self.z = z;
        if !self.shown || !need {
            return false;
        }
        let Some(after) = insert_after(z) else { return false };
        unsafe { SetWindowPos(self.hwnd, Some(after), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) }.is_ok()
    }

    pub fn hwnd(&self) -> isize {
        self.hwnd.0 as isize
    }

    pub fn shown(&self) -> bool {
        self.shown
    }

    /// `WDA_EXCLUDEFROMCAPTURE` (true) or `WDA_NONE`; returns whether the call succeeded.
    pub fn set_exclude(&mut self, on: bool) -> bool {
        if self.exclude == Some(on) {
            return true;
        }
        let ok = unsafe { SetWindowDisplayAffinity(self.hwnd, if on { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE }) }.is_ok();
        if ok {
            self.exclude = Some(on);
        }
        ok
    }

    /// Draws `canvas` at screen `(x, y)` and shows the window topmost without activating it.
    pub fn present(&mut self, x: i32, y: i32, canvas: &Canvas) -> bool {
        self.present_z(x, y, canvas, ZMode::Topmost)
    }

    /// Draws `canvas` at screen `(x, y)` and shows the window without activating it, placed in
    /// the z-order as `z` wants (in the same call that shows it).
    pub fn present_z(&mut self, x: i32, y: i32, canvas: &Canvas, z: ZMode) -> bool {
        if self.shown {
            self.apply_z(z);
        } else {
            self.z = z;
        }
        let (w, h) = (canvas.rect.w, canvas.rect.h);
        if w <= 0 || h <= 0 {
            self.hide();
            return false;
        }
        let fresh = self.dib.as_ref().is_none_or(|d| d.w != w || d.h != h);
        if fresh {
            self.dib = Dib::new(w, h);
        }
        let Some(dib) = self.dib.as_mut() else { return false };
        // Only the changed part is copied and handed to the compositor; a new size or position
        // updates everything.
        let whole = fresh || !self.shown || self.pos != (x, y, w, h);
        let dirty = if whole {
            dib.copy_from(canvas);
            None
        } else {
            match changed_region(canvas, self.prev_dirty) {
                Some(r) => {
                    dib.copy_region(canvas, r);
                    Some(RECT { left: r.x, top: r.y, right: r.x + r.w, bottom: r.y + r.h })
                }
                // Nothing changed at all.
                None => Some(RECT::default()),
            }
        };
        self.prev_dirty = canvas.dirty().map(|r| Rect { x: r.x - canvas.rect.x, y: r.y - canvas.rect.y, w: r.w, h: r.h });
        if dirty.is_some_and(|r| r.right <= r.left || r.bottom <= r.top) {
            return true;
        }
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        let ok = unsafe {
            let screen = GetDC(None);
            let (dst, size, src) = (POINT { x, y }, SIZE { cx: w, cy: h }, POINT { x: 0, y: 0 });
            let info = UPDATELAYEREDWINDOWINFO {
                cbSize: std::mem::size_of::<UPDATELAYEREDWINDOWINFO>() as u32,
                hdcDst: screen,
                pptDst: &dst,
                psize: &size,
                hdcSrc: dib.dc,
                pptSrc: &src,
                crKey: COLORREF(0),
                pblend: &blend,
                dwFlags: ULW_ALPHA,
                prcDirty: dirty.as_ref().map_or(std::ptr::null(), |r| r as *const RECT),
            };
            let r = UpdateLayeredWindowIndirect(self.hwnd, &info);
            ReleaseDC(None, screen);
            r.as_bool()
        };
        self.pos = (x, y, w, h);
        if ok && !self.shown {
            // shown and placed in one call, so it never appears above a window it must stay under
            let after = insert_after(self.z).unwrap_or(HWND_TOPMOST);
            unsafe {
                let _ = SetWindowPos(self.hwnd, Some(after), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
            }
            self.shown = true;
        }
        ok
    }

    pub fn hide(&mut self) {
        if self.shown {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
            self.shown = false;
        }
        self.prev_dirty = None;
    }

    /// Screen rectangle of the last `present`.
    pub fn pos(&self) -> (i32, i32, i32, i32) {
        self.pos
    }
}

impl Drop for LayeredWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    /// A WM_CLOSE sent to an overlay or toast window quits the owning thread's message loop
    /// (the Helper then exits like the tray's Exit) and leaves the window alone. The window is
    /// created hidden and never shown (amendment A1).
    #[test]
    fn wm_close_on_a_layered_window_quits_the_ui_loop() {
        let w = LayeredWindow::create(OVERLAY_CLASS).expect("hidden layered window");
        let h = HWND(w.hwnd() as *mut core::ffi::c_void);
        unsafe {
            assert!(!IsWindowVisible(h).as_bool());
            SendMessageW(h, WM_CLOSE, Some(WPARAM(0)), Some(LPARAM(0)));
            assert!(IsWindow(Some(h)).as_bool(), "the window is not destroyed");
            let mut msg = MSG::default();
            let got = PeekMessageW(&mut msg, None, WM_QUIT, WM_QUIT, PM_REMOVE).as_bool();
            assert!(got && msg.message == WM_QUIT && msg.wParam.0 == 0, "WM_QUIT with code 0 posted");
        }
    }

    /// D12 z-order with three hidden windows of the test: `w` stands for the game window, `a` for
    /// another application's window above it, `o` for an overlay window. Nothing is ever shown
    /// and the focus never changes (amendment A1): only the z-order of hidden windows moves.
    #[test]
    fn z_order_right_above_the_game_window() {
        register_classes();
        let mk = || unsafe {
            CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, OVERLAY_CLASS, w!("z test"), WS_POPUP, -32000, -32000, 1, 1, None, None, Some(hinstance()), None)
                .expect("hidden window")
        };
        let (w, a, o) = (mk(), mk(), mk());
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
        unsafe {
            for h in [w, a, o] {
                assert!(!IsWindowVisible(h).as_bool());
            }
        }
        // creation order puts the newest on top: o, a, w
        let is_ours = |h: HWND| h == o;
        let wid = w.0 as isize;
        assert_eq!(insert_after_with(ZMode::Above(wid), &is_ours), Some(a));
        assert!(!right_above_with(o, wid, &is_ours));
        unsafe { SetWindowPos(o, Some(a), 0, 0, 0, 0, flags) }.unwrap();
        // a, o, w: the overlay is right above the game window, and the anchor skips it
        assert!(right_above_with(o, wid, &is_ours));
        assert_eq!(insert_after_with(ZMode::Above(wid), &is_ours), Some(a));
        // the game window is the highest window that is not topmost: the top of that band
        unsafe { SetWindowPos(a, Some(HWND_TOPMOST), 0, 0, 0, 0, flags) }.unwrap();
        assert!(is_topmost(a));
        let anchor = insert_after_with(ZMode::Above(wid), &is_ours);
        let above_o = unsafe { GetWindow(o, GW_HWNDPREV) }.unwrap_or_default();
        if above_o.0.is_null() || is_topmost(above_o) {
            assert_eq!(anchor, Some(HWND_NOTOPMOST));
        }
        assert_eq!(insert_after_with(ZMode::Topmost, &is_ours), Some(HWND_TOPMOST));
        // the game window is gone
        unsafe {
            let _ = DestroyWindow(w);
        }
        assert_eq!(insert_after_with(ZMode::Above(wid), &is_ours), None);
        unsafe {
            let _ = DestroyWindow(a);
            let _ = DestroyWindow(o);
        }
    }

    #[test]
    fn changed_region_is_the_union_of_both_dirty_rectangles() {
        let mut c = Canvas::new(rect(100, 200, 50, 40));
        assert!(changed_region(&c, None).is_none());
        c.fill(rect(110, 210, 4, 3), [1, 2, 3], 1.0);
        let r = changed_region(&c, None).unwrap();
        assert_eq!((r.x, r.y, r.w, r.h), (10, 10, 4, 3), "canvas-local");
        let r = changed_region(&c, Some(rect(30, 0, 5, 5))).unwrap();
        assert_eq!((r.x, r.y, r.w, r.h), (10, 0, 25, 13));
        c.reset(rect(100, 200, 50, 40));
        let r = changed_region(&c, Some(rect(30, 0, 5, 5))).unwrap();
        assert_eq!((r.x, r.y, r.w, r.h), (30, 0, 5, 5), "what was painted before must be cleared");
    }

    #[test]
    fn copy_region_updates_only_that_part() {
        let mut c = Canvas::new(rect(0, 0, 20, 10));
        c.fill(rect(0, 0, 20, 10), [255, 255, 255], 1.0);
        let mut d = Dib::new(20, 10).unwrap();
        assert!(d.copy_region(&c, rect(5, 2, 3, 4)));
        let px = |d: &Dib, x: usize, y: usize| d.bits()[(y * 20 + x) * 4 + 3];
        assert_eq!(px(&d, 5, 2), 255);
        assert_eq!(px(&d, 7, 5), 255);
        assert_eq!(px(&d, 8, 5), 0);
        assert_eq!(px(&d, 5, 6), 0);
        assert_eq!(px(&d, 4, 2), 0);
        // Clipped to the bitmap; another size is refused.
        assert!(d.copy_region(&c, rect(18, 8, 10, 10)));
        assert_eq!(px(&d, 19, 9), 255);
        assert!(!d.copy_region(&Canvas::new(rect(0, 0, 5, 5)), rect(0, 0, 5, 5)));
    }
}
