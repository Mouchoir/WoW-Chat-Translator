//! Graceful exit: `taskkill /PID <pid>` without /F stops the Helper like the tray's Exit (the
//! message loop ends, the capture thread and the engine stop, llama-server would be killed, the
//! log gets its Exit record and the exit code is 0).
//!
//! Measured fact behind it: `taskkill` without /F posts WM_CLOSE to one window of the process
//! only, the first top-level window in z-order. With an overlay or toast window (topmost) that is
//! never the hidden main window, so those windows forward WM_CLOSE (`overlay::window`).
//!
//! The Helper under test runs with its own single-instance mutex (`--instance-name`, so it never
//! collides with a Helper the user is running), no tray icon, the echo translator, its own
//! settings and log folders, and a game window class that does not exist: it never captures
//! anything and never shows a window (amendment A1). It is always stopped by PID.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetClassNameW, GetWindowThreadProcessId, IsWindowVisible};

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("tp_graceful_exit_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn spawn(name: &str, dir: &Path, extra: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_TranslateProbeHelper"))
        .args(["--instance-name", name, "--no-tray", "--translator", "echo", "--wow-class", "TPNoSuchGameWindowClass", "--exit-after", "60"])
        .args(extra)
        .arg("--log-dir")
        .arg(dir.join("logs"))
        .env("APPDATA", dir.join("appdata"))
        .env("LOCALAPPDATA", dir.join("localappdata"))
        .spawn()
        .expect("spawn the Helper")
}

unsafe extern "system" fn collect(h: HWND, lp: LPARAM) -> BOOL {
    let out = unsafe { &mut *(lp.0 as *mut Vec<(u32, String, bool)>) };
    let mut pid = 0u32;
    let mut name = [0u16; 64];
    unsafe {
        GetWindowThreadProcessId(h, Some(&mut pid));
        let n = GetClassNameW(h, &mut name).max(0) as usize;
        out.push((pid, String::from_utf16_lossy(&name[..n]), IsWindowVisible(h).as_bool()));
    }
    BOOL(1)
}

/// Top-level windows of process `pid`, in z-order (first = top): (class, visible).
fn windows_of(pid: u32) -> Vec<(String, bool)> {
    let mut all: Vec<(u32, String, bool)> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut all as *mut _ as isize));
    }
    all.into_iter().filter(|w| w.0 == pid).map(|w| (w.1, w.2)).collect()
}

fn wait_exit(child: &mut Child, limit: Duration) -> Option<ExitStatus> {
    let t0 = Instant::now();
    while t0.elapsed() < limit {
        if let Some(s) = child.try_wait().unwrap() {
            return Some(s);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

fn taskkill(pid: u32) -> bool {
    let exe = std::env::var("SystemRoot").map(|r| PathBuf::from(r).join("System32").join("taskkill.exe")).unwrap_or_else(|_| PathBuf::from("taskkill.exe"));
    // never /F: the point is the polite request
    Command::new(exe).args(["/PID", &pid.to_string()]).output().map(|o| o.status.success()).unwrap_or(false)
}

/// Starts a Helper, waits until it has the windows `want` (classes), checks which window is first
/// in z-order, asks it to stop with taskkill (no /F) and checks a clean exit.
fn exits_on_taskkill(tag: &str, extra: &[&str], want: &[&str], first: &str) {
    let dir = temp_dir(tag);
    let name = format!("exit-test-{tag}-{}", std::process::id());
    let mut child = spawn(&name, &dir, extra);
    let pid = child.id();
    let t0 = Instant::now();
    let mut wins = Vec::new();
    while t0.elapsed() < Duration::from_secs(15) {
        wins = windows_of(pid);
        if want.iter().all(|c| wins.iter().any(|(w, _)| w == c)) {
            break;
        }
        if child.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let ready = want.iter().all(|c| wins.iter().any(|(w, _)| w == c));
    let first_ours = wins.iter().find(|(w, _)| w.starts_with("TPHelper")).map(|(w, _)| w.clone());
    let sent = ready && taskkill(pid);
    let status = if sent { wait_exit(&mut child, Duration::from_secs(8)) } else { None };
    if status.is_none() {
        // never leave a process behind (amendment A5)
        let _ = child.kill();
        let _ = child.wait();
    }
    let log = std::fs::read_to_string(dir.join("logs").join("helper.log")).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ready, "windows {wins:?}, wanted {want:?}");
    assert!(wins.iter().all(|(_, visible)| !visible), "a window is visible: {wins:?}");
    assert_eq!(first_ours.as_deref(), Some(first), "first window in z-order: {wins:?}");
    assert!(sent, "taskkill refused");
    let status = status.expect("the Helper did not exit within 8 s of taskkill without /F");
    assert_eq!(status.code(), Some(0), "exit code");
    assert!(log.contains("Exit { code: 0"), "the log was not closed cleanly");
}

/// Normal mode: the hidden toast window is topmost, so taskkill's WM_CLOSE goes to it, not to the
/// main window.
#[test]
fn taskkill_without_f_exits_cleanly_through_a_topmost_window() {
    exits_on_taskkill("toast", &[], &["TPHelperMain", "TPHelperToast"], "TPHelperToast");
}

/// `--no-overlay`: only the hidden main window exists.
#[test]
fn taskkill_without_f_exits_cleanly_through_the_main_window() {
    exits_on_taskkill("main", &["--no-overlay"], &["TPHelperMain"], "TPHelperMain");
}
