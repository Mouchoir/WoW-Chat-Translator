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
    Command::new(env!("CARGO_BIN_EXE_WoWTranslateCompanion"))
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
    }
    // reaped in every case (at once when it already exited)
    let _ = child.wait();
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

/// The status file (2026-10-19, user request R1): a Helper whose `--status-root` is a fake WoW
/// folder under the test's temp folder writes "running" there at start and "stopped" when
/// `taskkill` without /F stops it (the WM_CLOSE path); an addon whose TOC does not list the file
/// gets nothing. The user's WoW folders are never touched (a test run writes only with
/// `--status-root`).
#[test]
fn the_status_file_says_running_then_stopped_after_taskkill_without_f() {
    let dir = temp_dir("status");
    let root = dir.join("World of Warcraft");
    let addon = |flavour: &str, lists: bool| -> PathBuf {
        let d = root.join(flavour).join("Interface").join("AddOns").join("TranslateProbe");
        std::fs::create_dir_all(&d).unwrap();
        let toc = if lists { "## Interface: 16001\nCompanionStatus.lua\nCore.lua\n" } else { "## Interface: 16001\nCore.lua\n" };
        std::fs::write(d.join("TranslateProbe.toc"), toc).unwrap();
        if lists {
            std::fs::write(d.join("CompanionStatus.lua"), "TranslateProbe_CompanionStatus = nil\n").unwrap();
        }
        d
    };
    let forever = addon("_classic_beta_", true);
    let old = addon("_classic_", false);
    let name = format!("exit-test-status-{}", std::process::id());
    let root_arg = root.to_string_lossy().to_string();
    let mut child = spawn(&name, &dir, &["--status-root", &root_arg]);
    let pid = child.id();
    let status_file = forever.join("CompanionStatus.lua");
    let read = || std::fs::read_to_string(&status_file).unwrap_or_default();
    let t0 = Instant::now();
    while !read().contains(r#"state = "running""#) && t0.elapsed() < Duration::from_secs(15) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let running = read();
    // the toast window must exist before taskkill (its WM_CLOSE path is the one under test)
    while !windows_of(pid).iter().any(|(w, _)| w == "TPHelperToast") && t0.elapsed() < Duration::from_secs(15) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let sent = taskkill(pid);
    let status = if sent { wait_exit(&mut child, Duration::from_secs(8)) } else { None };
    if status.is_none() {
        // never leave a process behind (amendment A5)
        let _ = child.kill();
    }
    // reaped in every case (at once when it already exited)
    let _ = child.wait();
    let stopped = read();
    let old_has = old.join("CompanionStatus.lua").exists();
    let leftovers: Vec<String> = std::fs::read_dir(&forever).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).collect();
    let log = std::fs::read_to_string(dir.join("logs").join("helper.log")).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(running.contains(r#"TranslateProbe_CompanionStatus = { v = 1, version = "0.6.0", build = 6, state = "running", heartbeat = "#), "{running}");
    assert!(sent, "taskkill refused");
    assert_eq!(status.expect("the Helper did not exit within 8 s of taskkill without /F").code(), Some(0));
    assert!(stopped.contains(r#"state = "stopped""#), "{stopped}");
    assert!(!old_has, "an addon whose TOC does not list the file never gets it");
    assert_eq!(leftovers.len(), 2, "the TOC and the status file only, no temporary file: {leftovers:?}");
    // the log has counts, never a path
    assert!(log.contains(r#"StatusFile { state: "running", roots: 1, targets: 1, written: 1, failed: 0, skipped: 0 }"#), "{log}");
    assert!(log.contains(r#"StatusFile { state: "stopped", roots: 1, targets: 1, written: 1, failed: 0, skipped: 0 }"#), "{log}");
    assert!(!log.contains("World of Warcraft") && !log.contains("tp_graceful_exit"), "no path in the log");
}
