//! Layered, topmost, click-through windows that never activate (overlay and toast), and the
//! in-memory DIBs they are drawn from.
//!
//! Styles: `WS_EX_LAYERED|TRANSPARENT|TOPMOST|NOACTIVATE|TOOLWINDOW`; `WM_MOUSEACTIVATE` returns
//! `MA_NOACTIVATE`, `WM_NCHITTEST` returns `HTTRANSPARENT`; shown with `SW_SHOWNOACTIVATE` and
//! `SWP_NOACTIVATE` only; hidden with `SW_HIDE` when empty.

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

/// One layered window and its backing DIB.
pub struct LayeredWindow {
    hwnd: HWND,
    shown: bool,
    exclude: Option<bool>,
    dib: Option<Dib>,
    pos: (i32, i32, i32, i32),
    /// Dirty rectangle (canvas-local) of the last canvas presented.
    prev_dirty: Option<Rect>,
}

impl LayeredWindow {
    pub fn create(class: PCWSTR) -> Option<LayeredWindow> {
        register_classes();
        let ex = WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW;
        let hwnd = unsafe { CreateWindowExW(ex, class, w!("TranslateProbe Helper"), WS_POPUP, 0, 0, 1, 1, None, None, Some(hinstance()), None) }.ok()?;
        Some(LayeredWindow { hwnd, shown: false, exclude: None, dib: None, pos: (0, 0, 0, 0), prev_dirty: None })
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

    /// Draws `canvas` at screen `(x, y)` and shows the window without activating it.
    pub fn present(&mut self, x: i32, y: i32, canvas: &Canvas) -> bool {
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
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
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
