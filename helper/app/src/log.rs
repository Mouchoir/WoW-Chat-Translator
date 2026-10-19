//! Helper log (`logs\helper.log`, rotated at 1 MB, 3 files) and the `--diag-json` writer.
//!
//! Type guard (design D.6): the logger only accepts [`AppEvent`] and `tp_engine::EngineEvent`,
//! whose fields are numbers, enums and `&'static str`. Chat text travels in `ChatText`, which has
//! no Debug, so it cannot be put into an event. The diag writer receives JSON built by the app
//! from numbers and fixed names only.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tp_engine::{EngineEvent, EngineState};

use crate::settings::LoadOutcome;

pub const MAX_LOG_BYTES: u64 = 1024 * 1024;
pub const LOG_FILES: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error = 0,
    Warn = 1,
    Info = 2,
    Debug = 3,
}

impl Level {
    pub fn parse(s: &str) -> Level {
        match s {
            "error" => Level::Error,
            "warn" => Level::Warn,
            "debug" => Level::Debug,
            _ => Level::Info,
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Level::Error => "ERROR",
            Level::Warn => "WARN",
            Level::Info => "INFO",
            Level::Debug => "DEBUG",
        }
    }
}

/// Helper events for the log. Numbers, enums and `&'static str` only.
#[derive(Debug)]
pub enum AppEvent {
    Start { version: &'static str, translator: &'static str, capture: &'static str, overlay: bool, tray: bool, diag: bool },
    Settings { outcome: LoadOutcome, fixed: u32 },
    SettingFixed { field: &'static str },
    WowFound { w: i32, h: i32, dpi: u32, foreground: bool },
    WowLost,
    WowMinimized { minimized: bool },
    CaptureStarted { method: &'static str, ok: bool, hresult: i32 },
    CaptureRecreated { method: &'static str },
    CaptureSwitched { from: &'static str, to: &'static str, reason: &'static str },
    CaptureMinute { method: &'static str, ticks: u64, fresh: u64, stale: u64, errors: u64 },
    DecodeMinute { ok: u64, rejected: u64, absent: u64, torn: u64, margin_avg: u32, messages: u64 },
    TagMinute { scans: u64, hits: u64, cards: u64, covered: u64 },
    Message { kind: &'static str, req: u16, slot: u8, rows: u8, bytes: u32, pairs: u8 },
    MessageCounters { frames: u64, chunks: u64, messages: u64, bad_crc: u64, parse_errors: u64, expired: u64, missed: u64 },
    SessionChange { old: u16, new: u16 },
    AddonBuild { build: u16, expected: u16 },
    Toast { kind: &'static str, shown: bool },
    Balloon { kind: &'static str },
    Engine { state: EngineState },
    OverlayAffinity { exclude: bool, ok: bool },
    /// The overlay's z-order mode (D12): topmost with the game window in front, else right
    /// above it; `moved` when a window was moved.
    ZOrder { topmost: bool, moved: bool },
    /// One clipboard step of a reply: what (begin, finish, release), its outcome, the request
    /// id, whether the text was cut to 255 bytes and kept out of the clipboard history. Never
    /// any text.
    Clipboard { what: &'static str, outcome: &'static str, req: u16, cut: bool, excluded: bool },
    /// The tray's reply switches (or `--no-clipboard`, logged as copy off).
    ReplySwitches { copy: bool, sound: bool },
    TrayIcon { ok: bool },
    Exit { code: i32, uptime_s: u64 },
    Panic { line: u32 },
}

enum LogMsg {
    Line(String),
    Flush(mpsc::Sender<()>),
}

pub struct Logger {
    tx: Mutex<Option<mpsc::Sender<LogMsg>>>,
    join: Mutex<Option<JoinHandle<()>>>,
    level: Level,
}

impl Logger {
    /// Starts the writer thread on `dir\helper.log`. When the folder cannot be created, the
    /// logger drops every line.
    pub fn start(dir: &Path, level: Level) -> Arc<Logger> {
        if std::fs::create_dir_all(dir).is_err() {
            return Logger::disabled();
        }
        let (tx, rx) = mpsc::channel::<LogMsg>();
        let dir = dir.to_path_buf();
        let join = std::thread::Builder::new()
            .name("tp-log".into())
            .spawn(move || writer_thread(dir, rx))
            .ok();
        Arc::new(Logger { tx: Mutex::new(Some(tx)), join: Mutex::new(join), level })
    }

    pub fn disabled() -> Arc<Logger> {
        Arc::new(Logger { tx: Mutex::new(None), join: Mutex::new(None), level: Level::Error })
    }

    pub fn app(&self, level: Level, ev: AppEvent) {
        self.write(level, "app", &ev);
    }

    pub fn engine(&self, ev: &EngineEvent) {
        self.write(Level::Info, "engine", ev);
    }

    fn write(&self, level: Level, source: &'static str, ev: &dyn std::fmt::Debug) {
        if level > self.level {
            return;
        }
        let line = format!("{} {} {} {:?}\n", utc_stamp(SystemTime::now()), level.tag(), source, ev);
        if let Ok(g) = self.tx.lock() {
            if let Some(tx) = g.as_ref() {
                let _ = tx.send(LogMsg::Line(line));
            }
        }
    }

    /// Waits until every line sent so far is on disk.
    pub fn flush(&self) {
        let (ack_tx, ack_rx) = mpsc::channel();
        let sent = self.tx.lock().ok().and_then(|g| g.as_ref().map(|tx| tx.send(LogMsg::Flush(ack_tx)).is_ok()));
        if sent == Some(true) {
            let _ = ack_rx.recv_timeout(std::time::Duration::from_secs(2));
        }
    }

    /// Flushes and stops the writer thread.
    pub fn shutdown(&self) {
        if let Ok(mut g) = self.tx.lock() {
            g.take();
        }
        if let Ok(mut j) = self.join.lock() {
            if let Some(j) = j.take() {
                let _ = j.join();
            }
        }
    }
}

fn log_path(dir: &Path, n: usize) -> PathBuf {
    if n == 0 {
        dir.join("helper.log")
    } else {
        dir.join(format!("helper.{n}.log"))
    }
}

fn open_append(path: &Path) -> Option<(File, u64)> {
    let f = OpenOptions::new().create(true).append(true).open(path).ok()?;
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    Some((f, len))
}

fn rotate(dir: &Path) {
    let _ = std::fs::remove_file(log_path(dir, LOG_FILES - 1));
    for n in (0..LOG_FILES - 1).rev() {
        let _ = std::fs::rename(log_path(dir, n), log_path(dir, n + 1));
    }
}

fn writer_thread(dir: PathBuf, rx: mpsc::Receiver<LogMsg>) {
    let mut cur = open_append(&log_path(&dir, 0));
    for msg in rx {
        match msg {
            LogMsg::Line(line) => {
                let needs_rotate = cur.as_ref().map(|(_, len)| *len + line.len() as u64 > MAX_LOG_BYTES).unwrap_or(false);
                if needs_rotate {
                    // Close the file before renaming it.
                    drop(cur.take());
                    rotate(&dir);
                    cur = open_append(&log_path(&dir, 0));
                }
                if let Some((f, len)) = cur.as_mut() {
                    if f.write_all(line.as_bytes()).is_ok() {
                        *len += line.len() as u64;
                    }
                }
            }
            LogMsg::Flush(ack) => {
                if let Some((f, _)) = cur.as_mut() {
                    let _ = f.flush();
                }
                let _ = ack.send(());
            }
        }
    }
    if let Some((mut f, _)) = cur {
        let _ = f.flush();
    }
}

/// `2026-10-16T12:34:56.789Z`
pub fn utc_stamp(t: SystemTime) -> String {
    let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (H. Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        year,
        month,
        day,
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        d.subsec_millis()
    )
}

/// Unix epoch milliseconds (the `t` of `--diag-json`).
pub fn epoch_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0)
}

/// How often buffered diag lines reach the file (and at exit).
pub const DIAG_FLUSH_EVERY: Duration = Duration::from_secs(1);

/// `--diag-json` writer: JSON Lines, `t` first, never any text (design D.6). Lines are buffered
/// and written about once a second (a write per line cost a system call per capture tick).
pub struct DiagSink {
    enabled: bool,
    out: Mutex<Option<(BufWriter<File>, Instant)>>,
}

impl DiagSink {
    pub fn open(path: &Path) -> Arc<DiagSink> {
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(dir);
            }
        }
        let out = File::create(path).ok().map(|f| (BufWriter::with_capacity(64 * 1024, f), Instant::now()));
        Arc::new(DiagSink { enabled: out.is_some(), out: Mutex::new(out) })
    }

    pub fn none() -> Arc<DiagSink> {
        Arc::new(DiagSink { enabled: false, out: Mutex::new(None) })
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Writes `{"t":<epoch ms>, ...v}`. `v` must be a JSON object.
    pub fn emit(&self, v: serde_json::Value) {
        if !self.enabled {
            return;
        }
        let Ok(mut g) = self.out.lock() else { return };
        let Some((w, last)) = g.as_mut() else { return };
        let body = v.to_string();
        let rest = body.strip_prefix('{').unwrap_or("}");
        let line = if rest == "}" { format!("{{\"t\":{:.1}}}\n", epoch_ms()) } else { format!("{{\"t\":{:.1},{}\n", epoch_ms(), rest) };
        let _ = w.write_all(line.as_bytes());
        if last.elapsed() >= DIAG_FLUSH_EVERY {
            let _ = w.flush();
            *last = Instant::now();
        }
    }

    /// Writes `{"t":<epoch ms>,<fields>}`; `fields` is the inside of a JSON object, built by the
    /// caller from numbers and fixed names only (for records written every capture).
    pub fn emit_raw(&self, fields: &str) {
        if !self.enabled {
            return;
        }
        let Ok(mut g) = self.out.lock() else { return };
        let Some((w, last)) = g.as_mut() else { return };
        let line = format!("{{\"t\":{:.1},{}}}\n", epoch_ms(), fields);
        let _ = w.write_all(line.as_bytes());
        if last.elapsed() >= DIAG_FLUSH_EVERY {
            let _ = w.flush();
            *last = Instant::now();
        }
    }

    pub fn flush(&self) {
        if let Ok(mut g) = self.out.lock() {
            if let Some((w, last)) = g.as_mut() {
                let _ = w.flush();
                *last = Instant::now();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tp_log_test_{}_{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn stamp_format() {
        let t = UNIX_EPOCH + std::time::Duration::from_millis(1_792_154_096_789);
        assert_eq!(utc_stamp(t), "2026-10-16T12:34:56.789Z");
        assert_eq!(utc_stamp(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
        let leap = UNIX_EPOCH + std::time::Duration::from_secs(951_782_400); // 2000-02-29
        assert_eq!(&utc_stamp(leap)[..10], "2000-02-29");
    }

    #[test]
    fn writes_levels_and_rotates() {
        let dir = temp_dir("rotate");
        let log = Logger::start(&dir, Level::Info);
        log.app(Level::Debug, AppEvent::WowLost);
        log.app(Level::Info, AppEvent::Toast { kind: "bridge_ok", shown: false });
        log.flush();
        let text = std::fs::read_to_string(dir.join("helper.log")).unwrap();
        assert!(text.contains("INFO app Toast { kind: \"bridge_ok\", shown: false }"), "{text}");
        assert!(!text.contains("WowLost"));
        // Fill past 1 MB twice: three files at most.
        for i in 0..40_000u32 {
            log.app(Level::Info, AppEvent::Exit { code: i as i32, uptime_s: u64::from(i) });
        }
        log.shutdown();
        assert!(dir.join("helper.log").exists());
        assert!(dir.join("helper.1.log").exists());
        assert!(dir.join("helper.2.log").exists());
        assert!(!dir.join("helper.3.log").exists());
        for n in 0..LOG_FILES {
            assert!(std::fs::metadata(log_path(&dir, n)).unwrap().len() <= MAX_LOG_BYTES);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn diag_lines_start_with_t() {
        let dir = temp_dir("diag");
        let path = dir.join("d.jsonl");
        let d = DiagSink::open(&path);
        assert!(d.enabled());
        d.emit(serde_json::json!({"ev": "frame", "ok": true}));
        d.emit(serde_json::json!({}));
        d.emit_raw(r#""ev":"capture","fresh":true,"method":"wgc""#);
        d.flush();
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        let v3: serde_json::Value = serde_json::from_str(lines[2]).unwrap();
        assert_eq!(v3["ev"], "capture");
        assert_eq!(v3["method"], "wgc");
        assert!(v3["t"].as_f64().unwrap() > 1.7e12);
        assert!(lines[0].starts_with("{\"t\":"));
        let v: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(v["ev"], "frame");
        assert!(v["t"].as_f64().unwrap() > 1.7e12);
        let v2: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert!(v2["t"].is_number());
        assert!(!DiagSink::none().enabled());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
