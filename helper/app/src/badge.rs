//! The fallback badge's hover windows and tooltip (2026-10-20, remote translation services).
//!
//! When the remote service fails (or its daily limit is reached), the engine translates locally
//! and the result carries a fallback reason. The overlay draws a small amber warning triangle after
//! that card's last text line (`overlay::BadgeSpot`); this module puts one tiny window over each
//! badge so the player can hover it, and shows a small tooltip saying why the line was translated
//! locally (provider, HTTP status, the service's short message).
//!
//! The badge window ([`BADGE_CLASS`]) is layered and drawn at alpha 1/255 over its whole square:
//! invisible (the overlay draws the glyph, masked like the text), but hit-testable, since only
//! fully transparent pixels let the mouse through. It never activates: `WS_EX_NOACTIVATE |
//! WS_EX_TOOLWINDOW`, `WM_MOUSEACTIVATE` answers `MA_NOACTIVATE`, it is shown with
//! `SWP_NOACTIVATE` only, and clicks on it do nothing. `TrackMouseEvent(TME_LEAVE)` reports the
//! leave. It follows the card: placed, moved and hidden on every render, in the overlay's z-order
//! mode (topmost with the game in front, right above the game window otherwise, decision D12),
//! and destroyed when its card goes. The covering test skips it (`wow::is_own_hit_target`).
//!
//! The tooltip ([`crate::overlay::window::TIP_CLASS`]) is click-through like the overlay and
//! drawn by the app (no common controls): a dark rounded box with the title in the badge's amber
//! and the reason below, above the badge (below it when there is no room), inside the monitor's
//! work area. Under `--no-overlay` no window is ever created.

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Once;

use tp_bridge::image::Rect;
use tp_engine::JobKey;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONULL};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::overlay::fit;
use crate::overlay::fonts::{Face, Fonts};
use crate::overlay::render::Canvas;
use crate::overlay::window::{hinstance, LayeredWindow, ZMode, TIP_CLASS};
use crate::overlay::{BadgeSpot, BADGE_RGB};

pub const BADGE_CLASS: PCWSTR = w!("TPHelperBadge");
/// Posted to the notify window (the main window) by a badge window: wParam 1 when the mouse
/// entered it, 0 when it left; lParam is the badge window.
pub const WM_APP_BADGE: u32 = WM_APP + 30;
/// The badge window's alpha: the lowest that the mouse still hits.
const HIT_ALPHA: f32 = 1.0 / 255.0;
/// Tooltip: the widest it gets, its padding, the space between it and the badge.
const TIP_MAX_W: i32 = 460;
const TIP_PAD: i32 = 8;
const TIP_GAP: i32 = 6;
/// Lines of the reason, at most.
const TIP_REASON_LINES: usize = 3;
const TIP_BG: [u8; 3] = [16, 20, 24];
const TIP_TEXT: [u8; 3] = [225, 232, 238];

static NOTIFY: AtomicIsize = AtomicIsize::new(0);

fn post_notify(entered: bool, hwnd: HWND) {
    let n = NOTIFY.load(Ordering::Relaxed);
    if n != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(n as *mut core::ffi::c_void)), WM_APP_BADGE, WPARAM(usize::from(entered)), LPARAM(hwnd.0 as isize));
        }
    }
}

unsafe extern "system" fn badge_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEMOVE => {
            // the first move: ask for the leave, tell the UI thread the mouse is in
            if unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } == 0 {
                let mut t = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                let _ = unsafe { TrackMouseEvent(&mut t) };
                unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 1) };
                post_notify(true, hwnd);
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
            post_notify(false, hwnd);
            LRESULT(0)
        }
        // clicks on the badge do nothing (and never reach the game under it)
        WM_LBUTTONDOWN | WM_LBUTTONUP | WM_LBUTTONDBLCLK | WM_RBUTTONDOWN | WM_RBUTTONUP | WM_RBUTTONDBLCLK | WM_MBUTTONDOWN
        | WM_MBUTTONUP | WM_MBUTTONDBLCLK | WM_XBUTTONDOWN | WM_XBUTTONUP | WM_CONTEXTMENU => LRESULT(0),
        WM_SETCURSOR => {
            if let Ok(c) = unsafe { LoadCursorW(None, IDC_ARROW) } {
                unsafe { SetCursor(Some(c)) };
            }
            LRESULT(1)
        }
        // like every window of the Helper: `taskkill` without /F exits like the tray's Exit
        WM_CLOSE => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

fn register_badge_class() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(badge_proc),
            hInstance: hinstance(),
            lpszClassName: BADGE_CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
    });
}

/// A badge window's content: its square at alpha 1/255 (invisible, hit-testable).
pub fn hit_canvas(w: i32, h: i32) -> Canvas {
    let mut c = Canvas::new(Rect { x: 0, y: 0, w: w.max(1), h: h.max(1) });
    c.fill(Rect { x: 0, y: 0, w: w.max(1), h: h.max(1) }, [0, 0, 0], HIT_ALPHA);
    c
}

/// Where the tooltip goes for a badge at `badge` ([x, y, w, h], screen) with a tooltip of
/// `w` x `h` inside the work area `work` ([left, top, right, bottom]): centred over the badge,
/// above it; below it when there is no room above; always inside the work area.
pub fn tip_position(badge: [i32; 4], w: i32, h: i32, work: [i32; 4]) -> (i32, i32) {
    let [bx, by, bw, bh] = badge;
    let x = (bx + bw / 2 - w / 2).min(work[2] - w).max(work[0]);
    let above = by - TIP_GAP - h;
    let y = if above >= work[1] { above } else { (by + bh + TIP_GAP).min(work[3] - h).max(work[1]) };
    (x, y)
}

/// One badge on screen.
struct Item {
    key: JobKey,
    view: u8,
    /// Screen rectangle [x, y, w, h].
    rect: [i32; 4],
    win: Option<LayeredWindow>,
}

/// The tooltip's text: a title and the reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TipText {
    pub title: String,
    pub reason: String,
}

/// The badge windows of the visible fallen-back results, and the tooltip.
pub struct Badges {
    no_overlay: bool,
    exclude: bool,
    z: ZMode,
    items: Vec<Item>,
    tip: Option<LayeredWindow>,
    /// The badge whose tooltip shows (key, view).
    tip_for: Option<(JobKey, u8)>,
    /// The tooltip's last panel (it moves with its badge).
    tip_canvas: Option<Canvas>,
    fonts: Fonts,
}

impl Badges {
    /// `notify`: the window that gets [`WM_APP_BADGE`] (the main window).
    pub fn new(no_overlay: bool, notify: isize) -> Badges {
        if notify != 0 {
            NOTIFY.store(notify, Ordering::Relaxed);
        }
        Badges { no_overlay, exclude: false, z: ZMode::Topmost, items: Vec::new(), tip: None, tip_for: None, tip_canvas: None, fonts: Fonts::new() }
    }

    /// The badges in place, as (key, view, screen rectangle).
    pub fn placed(&self) -> Vec<(JobKey, u8, [i32; 4])> {
        self.items.iter().map(|i| (i.key, i.view, i.rect)).collect()
    }

    /// Window handles of the badges (tests and the overlay probe).
    pub fn hwnds(&self) -> Vec<isize> {
        self.items.iter().filter_map(|i| i.win.as_ref().map(|w| w.hwnd())).collect()
    }

    /// The tooltip's window while it shows.
    pub fn tip_hwnd(&self) -> Option<isize> {
        self.tip.as_ref().filter(|t| t.shown()).map(|t| t.hwnd())
    }

    /// Puts one badge window over each spot of the last render (`origin`: the client origin of
    /// the game window on screen) in z-order mode `z`; destroys the windows of the badges that
    /// went. Returns true when the set of badges or a position changed.
    pub fn sync(&mut self, spots: &[BadgeSpot], origin: (i32, i32), z: ZMode) -> bool {
        self.z = z;
        let before: Vec<(JobKey, u8, [i32; 4])> = self.placed();
        // gone: destroyed with their card
        self.items.retain(|i| spots.iter().any(|s| s.key == i.key && s.view == i.view));
        if let Some(t) = self.tip_for {
            if !self.items.iter().any(|i| (i.key, i.view) == t) {
                self.hide_tip();
            }
        }
        for s in spots {
            let rect = [origin.0 + s.rect[0], origin.1 + s.rect[1], s.rect[2], s.rect[3]];
            let idx = match self.items.iter().position(|i| i.key == s.key && i.view == s.view) {
                Some(i) => i,
                None => {
                    let win = if self.no_overlay {
                        None
                    } else {
                        register_badge_class();
                        LayeredWindow::create_hit_target(BADGE_CLASS).map(|mut w| {
                            w.set_exclude(self.exclude);
                            w
                        })
                    };
                    self.items.push(Item { key: s.key, view: s.view, rect: [i32::MIN, 0, 0, 0], win });
                    self.items.len() - 1
                }
            };
            let it = &mut self.items[idx];
            if let Some(w) = it.win.as_mut() {
                // drawn again only when it moved (or was never shown); the z-order follows in
                // set_z (a new size gets a new canvas; the same size keeps the DIB)
                if it.rect != rect || !w.shown() {
                    w.present_z(rect[0], rect[1], &hit_canvas(rect[2], rect[3]), z);
                }
            }
            it.rect = rect;
        }
        // the tooltip follows its badge
        if let Some(t) = self.tip_for {
            if let Some(i) = self.items.iter().find(|i| (i.key, i.view) == t) {
                let rect = i.rect;
                if let Some(tip) = self.tip.as_mut().filter(|w| w.shown()) {
                    let (_, _, tw, th) = tip.pos();
                    let (x, y) = tip_position(rect, tw, th, work_area(rect));
                    if (x, y) != (tip.pos().0, tip.pos().1) {
                        if let Some(c) = self.tip_canvas.take() {
                            tip.present_z(x, y, &c, z);
                            self.tip_canvas = Some(c);
                        }
                    }
                }
            }
        }
        self.placed() != before
    }

    /// Destroys every badge window and hides the tooltip (the cards are hidden).
    pub fn hide_all(&mut self) {
        self.items.clear();
        self.hide_tip();
    }

    /// The overlay's z-order mode changed (decision D12): the badges and the tooltip follow.
    pub fn set_z(&mut self, z: ZMode) {
        self.z = z;
        for i in self.items.iter_mut() {
            if let Some(w) = i.win.as_mut() {
                w.apply_z(z);
            }
        }
        if let Some(t) = self.tip.as_mut() {
            t.apply_z(z);
        }
    }

    /// Display affinity, like the overlay (`WDA_EXCLUDEFROMCAPTURE` under GDI capture).
    pub fn set_exclude(&mut self, on: bool) {
        self.exclude = on;
        for i in self.items.iter_mut() {
            if let Some(w) = i.win.as_mut() {
                w.set_exclude(on);
            }
        }
        if let Some(t) = self.tip.as_mut() {
            t.set_exclude(on);
        }
    }

    /// The request and view of the badge window `hwnd`.
    pub fn key_of(&self, hwnd: isize) -> Option<(JobKey, u8)> {
        self.items.iter().find(|i| i.win.as_ref().is_some_and(|w| w.hwnd() == hwnd)).map(|i| (i.key, i.view))
    }

    /// The tooltip's panel for `text` at font size `px`: a dark rounded box, the title in the
    /// badge's amber, the reason (up to 3 lines) in a light grey.
    pub fn render_tip(&mut self, text: &TipText, px: i32) -> Canvas {
        let px = px.clamp(11, 24);
        let line_h = px + px / 3;
        let inner = TIP_MAX_W - 2 * TIP_PAD;
        let fonts = &mut self.fonts;
        let mut title = fit::wrap(&text.title, inner, &mut |s| fonts.measure(Face::Ui, px, s));
        title.truncate(2);
        let mut reason = if text.reason.is_empty() { Vec::new() } else { fit::wrap(&text.reason, inner, &mut |s| fonts.measure(Face::Ui, px, s)) };
        if reason.len() > TIP_REASON_LINES {
            reason.truncate(TIP_REASON_LINES);
            if let Some(last) = reason.last_mut() {
                last.push_str(fit::ELLIPSIS);
            }
        }
        let lines: Vec<(&str, [u8; 3])> = title.iter().map(|l| (l.as_str(), BADGE_RGB)).chain(reason.iter().map(|l| (l.as_str(), TIP_TEXT))).collect();
        let tw = lines.iter().map(|(l, _)| self.fonts.measure(Face::Ui, px, l)).max().unwrap_or(0);
        let w = (tw + 2 * TIP_PAD).clamp(60, TIP_MAX_W);
        let h = lines.len() as i32 * line_h + 2 * TIP_PAD - (line_h - px) / 2;
        let mut c = Canvas::new(Rect { x: 0, y: 0, w, h });
        let r = 5.0f32;
        for y in 0..h {
            for x in 0..w {
                let cx = (x as f32 + 0.5).clamp(r, w as f32 - r);
                let cy = (y as f32 + 0.5).clamp(r, h as f32 - r);
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                c.blend(x, y, TIP_BG, 0.92 * (r - d + 0.5).clamp(0.0, 1.0));
            }
        }
        for (i, (line, rgb)) in lines.iter().enumerate() {
            let top = TIP_PAD + i as i32 * line_h - (line_h - px) / 4;
            let cov = self.fonts.coverage(Face::Ui, px, line, w - 2 * TIP_PAD, line_h);
            for y in 0..line_h {
                for x in 0..w - 2 * TIP_PAD {
                    let a = cov.at(x, y);
                    if a != 0 {
                        c.blend(x + TIP_PAD, y + top, *rgb, f32::from(a) / 255.0);
                    }
                }
            }
        }
        c
    }

    /// Shows the tooltip for the badge window `hwnd` (the mouse entered it). Returns false when
    /// `hwnd` is not a badge or under `--no-overlay`.
    pub fn show_tip(&mut self, hwnd: isize, text: &TipText) -> bool {
        let Some((key, view)) = self.key_of(hwnd) else { return false };
        let Some(rect) = self.items.iter().find(|i| i.key == key && i.view == view).map(|i| i.rect) else { return false };
        if self.no_overlay {
            return false;
        }
        let px = ((rect[3] as f32) * 1.1).round() as i32;
        let canvas = self.render_tip(text, px);
        let (x, y) = tip_position(rect, canvas.rect.w, canvas.rect.h, work_area(rect));
        if self.tip.is_none() {
            self.tip = LayeredWindow::create(TIP_CLASS).map(|mut w| {
                w.set_exclude(self.exclude);
                w
            });
        }
        let z = self.z;
        let shown = self.tip.as_mut().is_some_and(|t| t.present_z(x, y, &canvas, z));
        self.tip_canvas = Some(canvas);
        if shown {
            self.tip_for = Some((key, view));
        }
        shown
    }

    /// The mouse left the badge window `hwnd`: its tooltip goes.
    pub fn leave(&mut self, hwnd: isize) {
        if self.key_of(hwnd).is_some_and(|k| Some(k) == self.tip_for) || self.key_of(hwnd).is_none() {
            self.hide_tip();
        }
    }

    pub fn hide_tip(&mut self) {
        self.tip_for = None;
        if let Some(t) = self.tip.as_mut() {
            t.hide();
        }
    }

    /// The badge whose tooltip shows.
    pub fn tip_for(&self) -> Option<(JobKey, u8)> {
        self.tip_for
    }
}

/// No bounds: the tooltip of a badge on no monitor stays next to it.
const NO_BOUNDS: [i32; 4] = [i32::MIN / 2, i32::MIN / 2, i32::MAX / 2, i32::MAX / 2];

/// The work area of the monitor under the centre of a screen rectangle, as [left, top, right,
/// bottom]. A badge is always on a monitor in use; one on no monitor (an off-screen test window)
/// gets no bounds, so its tooltip stays next to it and never lands on a screen.
fn work_area(r: [i32; 4]) -> [i32; 4] {
    unsafe {
        let mon = MonitorFromPoint(POINT { x: r[0] + r[2] / 2, y: r[1] + r[3] / 2 }, MONITOR_DEFAULTTONULL);
        if mon.is_invalid() {
            return NO_BOUNDS;
        }
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(mon, &mut mi).as_bool() {
            let w = mi.rcWork;
            [w.left, w.top, w.right, w.bottom]
        } else {
            NO_BOUNDS
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::window::OVERLAY_CLASS;

    const OFF: (i32, i32) = (-32000, -32000);

    fn spot(req: u16, x: i32, y: i32) -> BadgeSpot {
        BadgeSpot { key: JobKey { session: 3, req_id: req }, view: 1, rect: [x, y, 12, 12] }
    }

    fn ex_style(h: isize) -> u32 {
        unsafe { GetWindowLongW(HWND(h as *mut core::ffi::c_void), GWL_EXSTYLE) as u32 }
    }

    /// A hidden window that receives the badges' notifications.
    fn notify_window() -> HWND {
        crate::overlay::window::register_classes();
        unsafe {
            CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, OVERLAY_CLASS, w!("badge test"), WS_POPUP, -32000, -32000, 1, 1, None, None, Some(hinstance()), None).expect("hidden window")
        }
    }

    #[test]
    fn the_tooltip_stays_in_the_work_area() {
        let work = [0, 0, 1920, 1040];
        // above the badge, centred
        assert_eq!(tip_position([500, 600, 12, 12], 200, 40, work), (406, 554));
        // no room above: below
        assert_eq!(tip_position([500, 10, 12, 12], 200, 40, work), (406, 28));
        // at the right edge and at the left edge
        assert_eq!(tip_position([1910, 600, 12, 12], 200, 40, work).0, 1720);
        assert_eq!(tip_position([2, 600, 12, 12], 200, 40, work).0, 0);
        // a second monitor left of the primary
        assert_eq!(tip_position([-1000, 500, 12, 12], 200, 40, [-1920, 0, 0, 1040]), (-1094, 454));
    }

    #[test]
    fn a_badge_window_is_invisible_but_hit_testable() {
        let c = hit_canvas(12, 12);
        assert!(c.px.chunks_exact(4).all(|p| p == [0, 0, 0, 1]), "alpha 1/255 everywhere, black");
    }

    #[test]
    fn badge_windows_follow_the_cards_and_never_take_the_focus() {
        let fg = unsafe { GetForegroundWindow() };
        let mut b = Badges::new(false, 0);
        // shown far off-screen (the test never puts anything on screen)
        assert!(b.sync(&[spot(1, 300, 400), spot(2, 300, 430)], OFF, ZMode::Topmost));
        let hw = b.hwnds();
        assert_eq!(hw.len(), 2);
        for h in &hw {
            let ex = ex_style(*h);
            assert!(ex & WS_EX_LAYERED.0 != 0 && ex & WS_EX_NOACTIVATE.0 != 0 && ex & WS_EX_TOOLWINDOW.0 != 0);
            assert_eq!(ex & WS_EX_TRANSPARENT.0, 0, "the mouse reaches it");
            assert!(crate::wow::is_own_hit_target(*h), "the covering test skips it");
        }
        assert_eq!(b.placed()[0].2, [-32000 + 300, -32000 + 400, 12, 12], "screen coordinates");
        // unchanged: nothing to report; moved: reported, same windows
        assert!(!b.sync(&[spot(1, 300, 400), spot(2, 300, 430)], OFF, ZMode::Topmost));
        assert!(b.sync(&[spot(1, 300, 380), spot(2, 300, 430)], OFF, ZMode::Topmost));
        assert_eq!(b.hwnds(), hw);
        // a card goes: its window is destroyed
        assert!(b.sync(&[spot(2, 300, 430)], OFF, ZMode::Topmost));
        assert_eq!(b.hwnds(), vec![hw[1]]);
        unsafe {
            assert!(!IsWindow(Some(HWND(hw[0] as *mut core::ffi::c_void))).as_bool());
        }
        assert_eq!(b.key_of(hw[1]), Some((JobKey { session: 3, req_id: 2 }, 1)));
        assert_eq!(b.key_of(hw[0]), None);
        // the cards are hidden: every window goes
        b.hide_all();
        assert!(b.hwnds().is_empty());
        unsafe {
            assert!(!IsWindow(Some(HWND(hw[1] as *mut core::ffi::c_void))).as_bool());
            assert_eq!(GetForegroundWindow(), fg, "the focus never moved");
        }
    }

    #[test]
    fn no_window_under_no_overlay() {
        let mut b = Badges::new(true, 0);
        assert!(b.sync(&[spot(1, 10, 10)], (0, 0), ZMode::Topmost));
        assert_eq!(b.placed().len(), 1);
        assert!(b.hwnds().is_empty());
        assert!(!b.show_tip(0, &TipText { title: "t".into(), reason: "r".into() }));
    }

    #[test]
    fn the_badge_never_activates_and_reports_the_mouse() {
        let n = notify_window();
        let mut b = Badges::new(false, n.0 as isize);
        b.sync(&[spot(7, 100, 100)], OFF, ZMode::Topmost);
        let h = HWND(b.hwnds()[0] as *mut core::ffi::c_void);
        unsafe {
            assert_eq!(SendMessageW(h, WM_MOUSEACTIVATE, None, None).0, MA_NOACTIVATE as isize);
            // clicks do nothing
            for m in [WM_LBUTTONDOWN, WM_LBUTTONUP, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_LBUTTONDBLCLK] {
                assert_eq!(SendMessageW(h, m, None, None).0, 0);
            }
            // the mouse comes in (once), then leaves
            SendMessageW(h, WM_MOUSEMOVE, None, None);
            SendMessageW(h, WM_MOUSEMOVE, None, None);
            SendMessageW(h, WM_MOUSELEAVE, None, None);
            let mut got = Vec::new();
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, Some(n), WM_APP_BADGE, WM_APP_BADGE, PM_REMOVE).as_bool() {
                got.push((msg.wParam.0, msg.lParam.0));
            }
            assert_eq!(got, vec![(1, h.0 as isize), (0, h.0 as isize)]);
            let _ = DestroyWindow(n);
        }
        NOTIFY.store(0, Ordering::Relaxed);
    }

    #[test]
    fn the_tooltip_shows_over_its_badge_and_goes_with_it() {
        let fg = unsafe { GetForegroundWindow() };
        let mut b = Badges::new(false, 0);
        b.sync(&[spot(7, 100, 100)], OFF, ZMode::Topmost);
        let h = b.hwnds()[0];
        let text = TipText { title: "Translated locally after an error from Claude".into(), reason: "HTTP 401 - invalid x-api-key".into() };
        assert!(b.show_tip(h, &text));
        let tip = b.tip_hwnd().expect("tooltip shown");
        let ex = ex_style(tip);
        assert!(ex & WS_EX_TRANSPARENT.0 != 0 && ex & WS_EX_NOACTIVATE.0 != 0, "click-through, never active");
        // next to its off-screen badge: above it, never pulled onto a screen
        let mut r = RECT::default();
        unsafe { GetWindowRect(HWND(tip as *mut core::ffi::c_void), &mut r) }.unwrap();
        assert!(r.right < -30000 && r.bottom <= -32000 + 100 - TIP_GAP, "tooltip at {r:?}");
        assert_eq!(work_area([-31900, -31900, 12, 12]), NO_BOUNDS, "no monitor there");
        assert_eq!(b.tip_for(), Some((JobKey { session: 3, req_id: 7 }, 1)));
        // the badge's card goes: so does the tooltip
        b.sync(&[], OFF, ZMode::Topmost);
        assert_eq!(b.tip_hwnd(), None);
        assert_eq!(b.tip_for(), None);
        // and leaving the badge hides it
        b.sync(&[spot(8, 100, 100)], OFF, ZMode::Topmost);
        let h = b.hwnds()[0];
        assert!(b.show_tip(h, &text));
        b.leave(h);
        assert_eq!(b.tip_hwnd(), None);
        b.hide_all();
        assert_eq!(unsafe { GetForegroundWindow() }, fg);
    }

    #[test]
    fn the_tooltip_panel_holds_the_title_and_the_reason() {
        let mut b = Badges::new(true, 0);
        let c = b.render_tip(&TipText { title: "Translated locally after an error from DeepL".into(), reason: "HTTP 456 - Quota exceeded".into() }, 14);
        assert!(c.rect.w > 120 && c.rect.w <= TIP_MAX_W);
        assert!(c.rect.h >= 2 * 14 + 2 * TIP_PAD - 6, "two lines: {}", c.rect.h);
        // the title line is amber, the reason light grey
        let amber = (0..c.rect.h / 2).any(|y| (0..c.rect.w).any(|x| c.pixel(x, y).is_some_and(|(p, _)| p[0] > 150 && p[1] > 100 && p[2] < 60)));
        let grey = (c.rect.h / 2..c.rect.h).any(|y| (0..c.rect.w).any(|x| c.pixel(x, y).is_some_and(|(p, _)| p[0] > 150 && p[2] > 150)));
        assert!(amber && grey);
        assert_eq!(c.pixel(0, 0).unwrap().1, 0, "rounded corner");
        // a long service message wraps into at most 3 lines and the panel stays narrow
        let long = "x ".repeat(300);
        let c = b.render_tip(&TipText { title: "t".into(), reason: long }, 14);
        assert!(c.rect.w <= TIP_MAX_W && c.rect.h <= 5 * 19 + 2 * TIP_PAD);
    }
}
