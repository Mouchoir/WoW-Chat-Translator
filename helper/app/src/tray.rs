//! Notification-area icon of Translate Companion: state tooltip (with the version, and the remote
//! service when one is used), balloons and a small menu ("Translation engine..." (the settings
//! window of the remote services, 2026-10-20), unload, "Get updates" (opens `RELEASES_URL` in the
//! browser), log folder, the two reply switches "Copy translated replies to the clipboard" and
//! "Sound when a reply is copied", exit). The icon is drawn at runtime (`CreateIconIndirect`), so
//! the exe carries no resource file.
//!
//! The menu follows the documented notification-icon pattern: `SetForegroundWindow` on the hidden
//! main window right before `TrackPopupMenuEx`, and a `WM_NULL` after it. Without it the menu does
//! not close when the user clicks elsewhere, and Esc goes to another thread. This runs only in
//! answer to the user's own right-click on the icon (the shell grants the foreground right for
//! that click and the taskbar already holds the foreground), never on a timer or in a test.

use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const WM_APP_TRAY: u32 = WM_APP + 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuCmd {
    Unload = 1,
    OpenLogs = 2,
    Exit = 3,
    /// Toggles `replyClipboard`.
    CopyReplies = 4,
    /// Toggles `replySound`.
    ReplySound = 5,
    /// Opens the releases page (`crate::RELEASES_URL`) in the default browser.
    Updates = 6,
    /// Opens the "Translation engine..." window (`crate::settings_window`).
    EngineSettings = 7,
}

impl MenuCmd {
    fn from_id(id: i32) -> Option<MenuCmd> {
        Some(match id {
            1 => MenuCmd::Unload,
            2 => MenuCmd::OpenLogs,
            3 => MenuCmd::Exit,
            4 => MenuCmd::CopyReplies,
            5 => MenuCmd::ReplySound,
            6 => MenuCmd::Updates,
            7 => MenuCmd::EngineSettings,
            _ => return None,
        })
    }
}

/// One menu entry: command, label, check state (None: not a check item), separator before it.
pub struct MenuItem<'a> {
    pub cmd: MenuCmd,
    pub label: &'a str,
    pub checked: Option<bool>,
    pub separator_before: bool,
}

pub struct Tray {
    hwnd: HWND,
    icon: HICON,
    tip: String,
    added: bool,
    /// NOTIFYICON_VERSION_4 accepted: a right-click arrives as WM_CONTEXTMENU (with the anchor
    /// point in wParam) and the shell lets the app take the foreground for its menu.
    v4: bool,
}

// Declared here (under other names) instead of enabling more windows crate features.
#[link(name = "user32")]
extern "system" {
    #[link_name = "AttachThreadInput"]
    fn attach_thread_input(attach: u32, attach_to: u32, on: i32) -> i32;
}
#[link(name = "kernel32")]
extern "system" {
    #[link_name = "GetCurrentThreadId"]
    fn current_thread_id() -> u32;
}

fn copy_wide(dst: &mut [u16], s: &str) {
    let w: Vec<u16> = s.encode_utf16().take(dst.len() - 1).collect();
    dst[..w.len()].copy_from_slice(&w);
    dst[w.len()] = 0;
}

/// 32x32 ARGB icon: a blue rounded square with a white "T" (also the settings window's icon).
pub fn make_icon() -> Option<HICON> {
    const N: i32 = 32;
    unsafe {
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: N,
                biHeight: -N,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let color = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        if bits.is_null() {
            let _ = DeleteObject(color.into());
            return None;
        }
        let px = std::slice::from_raw_parts_mut(bits as *mut u8, (N * N * 4) as usize);
        let r = 6.0f32;
        for y in 0..N {
            for x in 0..N {
                let cx = (x as f32 + 0.5).clamp(r, N as f32 - r);
                let cy = (y as f32 + 0.5).clamp(r, N as f32 - r);
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                let a = (r - d + 0.5).clamp(0.0, 1.0);
                let white = (y >= 7 && y < 12 && x >= 7 && x < 25) || (x >= 13 && x < 19 && y >= 7 && y < 26);
                let (cr, cg, cb) = if white { (255.0, 255.0, 255.0) } else { (45.0, 108.0, 223.0) };
                let i = ((y * N + x) * 4) as usize;
                px[i] = (cb * a) as u8;
                px[i + 1] = (cg * a) as u8;
                px[i + 2] = (cr * a) as u8;
                px[i + 3] = (255.0 * a) as u8;
            }
        }
        let mask = CreateBitmap(N, N, 1, 1, None);
        let info = ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info).ok();
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}

impl Tray {
    /// Adds the icon for the hidden main window `hwnd`.
    pub fn add(hwnd: isize, tip: &str) -> Option<Tray> {
        let icon = make_icon()?;
        let mut t = Tray { hwnd: HWND(hwnd as *mut core::ffi::c_void), icon, tip: tip.to_string(), added: false, v4: false };
        t.readd();
        Some(t)
    }

    fn data(&self) -> NOTIFYICONDATAW {
        let mut d = NOTIFYICONDATAW { cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32, hWnd: self.hwnd, uID: 1, ..Default::default() };
        d.uCallbackMessage = WM_APP_TRAY;
        d.hIcon = self.icon;
        d
    }

    /// (Re)adds the icon, e.g. after Explorer restarted ("TaskbarCreated").
    pub fn readd(&mut self) -> bool {
        let mut d = self.data();
        d.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        copy_wide(&mut d.szTip, &self.tip);
        self.added = unsafe { Shell_NotifyIconW(NIM_ADD, &d) }.as_bool();
        // Version 4: without it a legacy icon gets WM_RBUTTONUP and the shell does not hand the
        // foreground to the app, so its menu could not be dismissed by clicking elsewhere (seen
        // in use on 2026-10-20).
        self.v4 = false;
        if self.added {
            let mut v = self.data();
            v.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            self.v4 = unsafe { Shell_NotifyIconW(NIM_SETVERSION, &v) }.as_bool();
        }
        self.added
    }

    /// True when the icon runs with NOTIFYICON_VERSION_4 (see `v4`).
    pub fn version4(&self) -> bool {
        self.v4
    }

    pub fn added(&self) -> bool {
        self.added
    }

    pub fn set_tip(&mut self, tip: &str) {
        if tip == self.tip {
            return;
        }
        self.tip = tip.to_string();
        let mut d = self.data();
        d.uFlags = NIF_TIP;
        copy_wide(&mut d.szTip, tip);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &d);
        }
    }

    pub fn balloon(&mut self, title: &str, text: &str) {
        let mut d = self.data();
        d.uFlags = NIF_INFO;
        copy_wide(&mut d.szInfoTitle, title);
        copy_wide(&mut d.szInfo, text);
        d.dwInfoFlags = NIIF_INFO;
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &d);
        }
    }

    /// Shows the menu at `anchor` (the shell's point for a version 4 icon) or at the cursor;
    /// returns the chosen command.
    pub fn menu(&self, items: &[MenuItem], anchor: Option<(i32, i32)>) -> Option<MenuCmd> {
        unsafe {
            let m = CreatePopupMenu().ok()?;
            for it in items {
                if it.separator_before {
                    let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
                }
                let w: Vec<u16> = it.label.encode_utf16().chain(std::iter::once(0)).collect();
                let flags = match it.checked {
                    Some(true) => MF_STRING | MF_CHECKED,
                    _ => MF_STRING,
                };
                let _ = AppendMenuW(m, flags, it.cmd as usize, PCWSTR(w.as_ptr()));
            }
            let mut pt = POINT::default();
            match anchor {
                Some((x, y)) => {
                    pt.x = x;
                    pt.y = y;
                }
                None => {
                    let _ = GetCursorPos(&mut pt);
                }
            }
            let _ = SetForegroundWindow(self.hwnd);
            // Still not in front (the foreground lock): attach to the foreground thread for the
            // call, the documented fallback, so the menu closes on a click elsewhere or on Esc.
            if GetForegroundWindow() != self.hwnd {
                let fg = GetForegroundWindow();
                let fg_thread = GetWindowThreadProcessId(fg, None);
                let me = current_thread_id();
                if fg_thread != 0 && fg_thread != me && attach_thread_input(me, fg_thread, 1) != 0 {
                    let _ = BringWindowToTop(self.hwnd);
                    let _ = SetForegroundWindow(self.hwnd);
                    attach_thread_input(me, fg_thread, 0);
                }
            }
            let r = TrackPopupMenuEx(m, (TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON).0, pt.x, pt.y, self.hwnd, None);
            let _ = PostMessageW(Some(self.hwnd), WM_NULL, WPARAM(0), LPARAM(0));
            let _ = DestroyMenu(m);
            MenuCmd::from_id(r.0)
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            if self.added {
                let d = self.data();
                let _ = Shell_NotifyIconW(NIM_DELETE, &d);
            }
            let _ = DestroyIcon(self.icon);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_is_built_at_runtime() {
        let icon = make_icon().expect("icon");
        unsafe {
            let _ = DestroyIcon(icon);
        }
    }

    #[test]
    fn menu_ids_round_trip() {
        for c in [MenuCmd::Unload, MenuCmd::OpenLogs, MenuCmd::Exit, MenuCmd::CopyReplies, MenuCmd::ReplySound, MenuCmd::Updates, MenuCmd::EngineSettings] {
            assert_eq!(MenuCmd::from_id(c as i32), Some(c));
        }
        assert_eq!(MenuCmd::from_id(0), None, "menu dismissed");
    }

    #[test]
    fn wide_copy_truncates_and_terminates() {
        let mut buf = [1u16; 5];
        copy_wide(&mut buf, "abcdefgh");
        assert_eq!(buf, [97, 98, 99, 100, 0]);
    }
}
