//! The companion status file (2026-10-19, lead decision D1, user request R1): how the addon learns
//! at login or `/reload` that Translate Companion runs, like WeakAuras Companion's data file.
//!
//! The addon can never receive anything from the Helper while the game runs (no file, socket,
//! clipboard or pixel can be read by addon Lua), but the client reads the files of the addon's
//! folder at login and at every `/reload`. The addon ships `CompanionStatus.lua`, listed in its TOC
//! before `Core.lua`, holding only `TranslateProbe_CompanionStatus = nil`. The Helper overwrites
//! the *deployed* copy of that one file, and nothing else, in every WoW flavour folder whose
//! `Interface\AddOns\TranslateProbe\TranslateProbe.toc` lists it:
//!
//! ```lua
//! TranslateProbe_CompanionStatus = { v = 1, version = "0.6.0", build = 6, state = "running", heartbeat = 1792400000 }
//! ```
//!
//! numbers and short ASCII strings only. It writes `running` at start and every
//! [`HEARTBEAT`] (60 s) while it runs, and `stopped` on a clean exit (the tray's Exit, `WM_CLOSE`
//! from `taskkill` without /F, `--exit-after`, a session end). A killed Helper leaves `running`
//! with an old heartbeat: the addon counts it as running only while `heartbeat` is within 150 s of
//! its own `time()`.
//!
//! Each write is atomic: a temporary file in the same folder, flushed, then `MoveFileExW` with
//! `MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH`, so the client never reads half a file
//! (a replace that fails because the client is reading the file is tried again 3 times, 50 ms
//! apart, then left to the next check). The temporary file is always a new file (an old one, even
//! a hard link, is removed first, so its other names keep their content), and a folder that is a
//! reparse point (a junction or symbolic link, for example to a repo checkout) or whose status or
//! temporary file is one is skipped and only counted (integration review: the write followed
//! links out of the addon folder).
//!
//! Every [`CHECK`] (5 s) between two heartbeats, each addon folder's file is read (a few bytes):
//! one that is not this Helper's "running" file (the addon's own nil file after a deploy, an addon
//! update or a new install) is written at once, so a `/reload` soon after a deploy already finds
//! "running" (it waited for the next heartbeat, up to 60 s, before). The final "stopped" is tried
//! again until [`STOP_RETRY`] has passed when the client holds the file at that moment.
//!
//! WoW installs: settings.json `wowRoot` when set (a "World of Warcraft" folder or one flavour
//! folder); else every path ending in "World of Warcraft" in the Battle.net agent's
//! `%ProgramData%\Battle.net\Agent\product.db` (a protobuf file: each path is a length-prefixed
//! string, read without a protobuf library). Under each root the flavour folders are the known
//! ones (`_retail_`, `_classic_`, `_classic_era_`, `_classic_beta_`, `_ptr_`, `_xptr_`, `_beta_`)
//! and any other folder whose name starts with `_` (WoW Forever runs from one of them). The WoW
//! process is never opened. The log gets counts and booleans only, never a path (paths hold user
//! names).

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The file the addon ships and the Helper overwrites (deployed copy only).
pub const FILE_NAME: &str = "CompanionStatus.lua";
/// The temporary file of an atomic write, in the same folder.
pub const TMP_NAME: &str = "CompanionStatus.lua.tmp";
/// The addon's TOC, which must list [`FILE_NAME`] (an older addon never gets a file it does not
/// load).
pub const TOC_NAME: &str = "TranslateProbe.toc";
/// The global the addon reads at `ADDON_LOADED`.
pub const GLOBAL: &str = "TranslateProbe_CompanionStatus";
/// Format of the table (`v`).
pub const FORMAT: u32 = 1;
/// Time between two writes while the Helper runs (the addon's tolerance is 150 s).
pub const HEARTBEAT: Duration = Duration::from_secs(60);
/// Time between two checks that each addon folder still holds this Helper's "running" file.
pub const CHECK: Duration = Duration::from_secs(5);
/// How long the final "stopped" write is tried again for the folders where it failed.
pub const STOP_RETRY: Duration = Duration::from_millis(2500);
/// The first line of every file the Helper writes starts with this (how a check knows its own).
pub const HEADER: &str = "-- Written by Translate Companion while it runs";
/// The flavour folders under a "World of Warcraft" folder; any other folder starting with `_` is
/// looked at too.
pub const FLAVOURS: &[&str] = &["_retail_", "_classic_", "_classic_era_", "_classic_beta_", "_ptr_", "_xptr_", "_beta_"];
/// A replace refused (the client reading the file at that moment) is tried this many times more.
pub const REPLACE_RETRIES: u32 = 3;
pub const REPLACE_RETRY_MS: u64 = 50;
/// Largest product.db and TOC read (they are a few KB).
pub const MAX_READ: u64 = 1 << 20;

/// What the file says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Running,
    Stopped,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Running => "running",
            State::Stopped => "stopped",
        }
    }
}

/// `version` as written: ASCII letters, digits, '.', '-' and '+' only, at most 16 characters.
fn clean_version(v: &str) -> String {
    v.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+')).take(16).collect()
}

/// The text of the file: a comment and one assignment of a table of numbers and short ASCII
/// strings.
pub fn content(version: &str, build: u16, state: State, heartbeat: u64) -> String {
    format!(
        "{HEADER} (every {} s). Do not edit.\r\n{GLOBAL} = {{ v = {FORMAT}, version = \"{}\", build = {build}, state = \"{}\", heartbeat = {heartbeat} }}\r\n",
        HEARTBEAT.as_secs(),
        clean_version(version),
        state.name()
    )
}

/// Unix time in seconds (the addon compares it with its `time()`).
pub fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// `%ProgramData%\Battle.net\Agent\product.db`.
pub fn product_db_path() -> PathBuf {
    std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\ProgramData"))
        .join("Battle.net")
        .join("Agent")
        .join("product.db")
}

fn read_capped(p: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let f = std::fs::File::open(p).ok()?;
    let mut out = Vec::new();
    f.take(MAX_READ).read_to_end(&mut out).ok()?;
    Some(out)
}

/// True when `s` (a path read from product.db) names a "World of Warcraft" folder: absolute with a
/// drive letter, ending in "World of Warcraft" (any case, a trailing separator allowed).
fn is_wow_root(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() < 4 || !b[0].is_ascii_alphabetic() || b[1] != b':' || !matches!(b[2], b'/' | b'\\') {
        return false;
    }
    if s.chars().any(|c| c.is_control()) {
        return false;
    }
    let t = s.trim_end_matches(['/', '\\']);
    let tail = "world of warcraft";
    t.len() >= tail.len() + 3 && t.is_char_boundary(t.len() - tail.len()) && t[t.len() - tail.len()..].eq_ignore_ascii_case(tail) && matches!(t.as_bytes()[t.len() - tail.len() - 1], b'/' | b'\\')
}

/// Every "World of Warcraft" folder of a product.db (Battle.net agent database, protobuf): each
/// path is a length-delimited string, so a drive letter path is taken when the varint right
/// before it (1 or 2 bytes) is its exact length. Deduplicated, in file order.
pub fn roots_from_product_db(b: &[u8]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut keys: Vec<String> = Vec::new();
    for s in 1..b.len().saturating_sub(3) {
        if !(b[s].is_ascii_alphabetic() && b[s + 1] == b':' && matches!(b[s + 2], b'/' | b'\\')) {
            continue;
        }
        let mut lens: Vec<usize> = Vec::with_capacity(2);
        let last = b[s - 1];
        if last < 0x80 {
            if s >= 2 && b[s - 2] >= 0x80 {
                lens.push(usize::from(b[s - 2] & 0x7F) | usize::from(last) << 7);
            }
            lens.push(usize::from(last));
        }
        for len in lens {
            let Some(bytes) = b.get(s..s + len) else { continue };
            let Ok(text) = std::str::from_utf8(bytes) else { continue };
            if !is_wow_root(text) {
                continue;
            }
            let key = text.trim_end_matches(['/', '\\']).replace('/', "\\").to_ascii_lowercase();
            if !keys.contains(&key) {
                keys.push(key);
                out.push(PathBuf::from(text.trim_end_matches(['/', '\\'])));
            }
            break;
        }
    }
    out
}

/// True when the TOC lists `CompanionStatus.lua` (a line of its own, any case).
pub fn toc_lists_status(toc: &Path) -> bool {
    let Some(bytes) = read_capped(toc) else { return false };
    String::from_utf8_lossy(&bytes).lines().map(|l| l.trim_start_matches('\u{feff}').trim()).any(|l| l.eq_ignore_ascii_case(FILE_NAME))
}

/// The addon folder under a flavour folder.
fn addon_dir(flavour: &Path) -> PathBuf {
    flavour.join("Interface").join("AddOns").join("TranslateProbe")
}

/// True when `dir` is a deployed TranslateProbe whose TOC lists the status file.
fn is_target(dir: &Path) -> bool {
    toc_lists_status(&dir.join(TOC_NAME))
}

/// The addon folders under one root: the root itself when it is a flavour folder (a `wowRoot`
/// pointing there), else its known flavour folders and any other folder starting with `_`.
pub fn targets_under(root: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let own = addon_dir(root);
    if is_target(&own) {
        out.push(own);
    }
    let mut flavours: Vec<String> = FLAVOURS.iter().map(|s| s.to_string()).collect();
    if let Ok(rd) = std::fs::read_dir(root) {
        let mut more: Vec<String> = rd
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|n| n.starts_with('_') && !flavours.iter().any(|f| f.eq_ignore_ascii_case(n)))
            .collect();
        more.sort();
        flavours.extend(more);
    }
    for f in flavours {
        let d = addon_dir(&root.join(&f));
        if is_target(&d) && !out.contains(&d) {
            out.push(d);
        }
    }
    out
}

/// Where the file goes: the roots looked at and the addon folders found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Found {
    pub roots: usize,
    pub targets: Vec<PathBuf>,
}

/// The WoW roots: `wow_root` when given, else the product.db's (when it can be read).
pub fn roots(wow_root: Option<&Path>, product_db: Option<&Path>) -> Vec<PathBuf> {
    match (wow_root, product_db) {
        (Some(r), _) => vec![r.to_path_buf()],
        (None, Some(db)) => read_capped(db).map(|b| roots_from_product_db(&b)).unwrap_or_default(),
        (None, None) => Vec::new(),
    }
}

/// Finds every addon folder to write to.
pub fn discover(wow_root: Option<&Path>, product_db: Option<&Path>) -> Found {
    let roots = roots(wow_root, product_db);
    let mut targets: Vec<PathBuf> = Vec::new();
    for r in &roots {
        for t in targets_under(r) {
            if !targets.contains(&t) {
                targets.push(t);
            }
        }
    }
    Found { roots: roots.len(), targets }
}

/// True when `p` exists and is a reparse point (a junction, a symbolic link or any other kind):
/// the Helper never writes through one.
pub fn is_reparse_point(p: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink() || m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
}

/// True when the addon folder, its status file or its temporary file is a reparse point.
pub fn is_linked(dir: &Path) -> bool {
    is_reparse_point(dir) || is_reparse_point(&dir.join(FILE_NAME)) || is_reparse_point(&dir.join(TMP_NAME))
}

/// True when `dir`'s status file is this Helper's own and says `state` (its first bytes only).
pub fn holds_own(dir: &Path, state: State) -> bool {
    use std::io::Read;
    let Ok(f) = std::fs::File::open(dir.join(FILE_NAME)) else { return false };
    let mut head = Vec::new();
    if f.take(512).read_to_end(&mut head).is_err() {
        return false;
    }
    let text = String::from_utf8_lossy(&head);
    text.starts_with(HEADER) && text.contains(&format!("state = \"{}\"", state.name()))
}

fn wide(p: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

/// `MoveFileExW(from, to, REPLACE_EXISTING | WRITE_THROUGH)`.
fn replace(from: &Path, to: &Path) -> std::io::Result<()> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH};
    let (f, t) = (wide(from), wide(to));
    unsafe { MoveFileExW(PCWSTR(f.as_ptr()), PCWSTR(t.as_ptr()), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH) }
        .map_err(|e| std::io::Error::from_raw_os_error(e.code().0 & 0xFFFF))
}

/// Writes `text` to `dir\CompanionStatus.lua` atomically: `dir\CompanionStatus.lua.tmp`, a new
/// file (an old one is removed first: a hard link loses only this name), flushed, then moved over
/// the file. The temporary file never stays behind. Refused (`ErrorKind::Unsupported`) when the
/// folder or either file is a reparse point.
pub fn write_atomic(dir: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    if is_linked(dir) {
        return Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "reparse point"));
    }
    let tmp = dir.join(TMP_NAME);
    let dst = dir.join(FILE_NAME);
    let made = (|| -> std::io::Result<()> {
        match std::fs::remove_file(&tmp) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()
    })();
    if let Err(e) = made {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    let mut res = replace(&tmp, &dst);
    let mut tries = 0;
    while res.is_err() && tries < REPLACE_RETRIES {
        tries += 1;
        std::thread::sleep(Duration::from_millis(REPLACE_RETRY_MS));
        res = replace(&tmp, &dst);
    }
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

/// What one round of writes did (counts only, for the log).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Report {
    pub state: State,
    /// WoW roots looked at.
    pub roots: u32,
    /// Addon folders found (a deployed TranslateProbe whose TOC lists the file).
    pub targets: u32,
    pub written: u32,
    pub failed: u32,
    /// Addon folders skipped because the folder or a file there is a reparse point.
    pub skipped: u32,
    /// Something differs from the previous report (the log gets it at Info, else at Debug).
    pub changed: bool,
}

/// Where and what the writer writes.
#[derive(Clone, Debug)]
pub struct WriterConfig {
    /// settings.json `wowRoot`, or `--status-root`: the only root when set.
    pub wow_root: Option<PathBuf>,
    /// The Battle.net agent's product.db (None: no other root).
    pub product_db: Option<PathBuf>,
    pub version: String,
    pub build: u16,
    pub heartbeat: Duration,
    /// Time between two checks of the files between heartbeats ([`CHECK`]).
    pub check: Duration,
}

/// What one round did to the addon folders.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Round {
    pub written: u32,
    pub failed: u32,
    pub skipped: u32,
    /// The folders where the write failed (tried again by the final "stopped" round).
    pub failed_dirs: Vec<PathBuf>,
}

/// Writes `state` (heartbeat `now`) to each of `targets`; `only_foreign`: only to those whose file
/// is not already this Helper's file saying `state` (a check between heartbeats).
pub fn write_targets(cfg: &WriterConfig, targets: &[PathBuf], state: State, now: u64, only_foreign: bool) -> Round {
    let text = content(&cfg.version, cfg.build, state, now);
    let mut r = Round::default();
    for t in targets {
        if is_linked(t) {
            r.skipped += 1;
            continue;
        }
        if only_foreign && holds_own(t, state) {
            continue;
        }
        match write_atomic(t, &text) {
            Ok(()) => r.written += 1,
            Err(_) => {
                r.failed += 1;
                r.failed_dirs.push(t.clone());
            }
        }
    }
    r
}

/// Discovers the targets and writes `state` to each; returns where and what it did.
pub fn write_round(cfg: &WriterConfig, state: State, now: u64) -> (Found, Round) {
    let found = discover(cfg.wow_root.as_deref(), cfg.product_db.as_deref());
    let r = write_targets(cfg, &found.targets, state, now, false);
    (found, r)
}

/// The writer thread: `running` at once and every heartbeat, `stopped` when stopped.
pub struct StatusWriter {
    tx: mpsc::Sender<()>,
    join: Option<JoinHandle<()>>,
}

/// How long `stop` waits for the final write.
pub const STOP_WAIT: Duration = Duration::from_secs(3);

impl StatusWriter {
    /// Starts the thread. `report` gets the counts of every round (never a path).
    pub fn start(cfg: WriterConfig, report: Arc<dyn Fn(Report) + Send + Sync>) -> Option<StatusWriter> {
        let (tx, rx) = mpsc::channel::<()>();
        let join = std::thread::Builder::new()
            .name("tp-status".into())
            .spawn(move || {
                let mut last: Option<(State, u32, u32, u32, u32, u32)> = None;
                let mut emit = |state: State, found: &Found, r: &Round, news: bool| {
                    let key = (state, found.roots as u32, found.targets.len() as u32, r.written, r.failed, r.skipped);
                    let changed = news || last != Some(key);
                    last = Some(key);
                    report(Report { state, roots: key.1, targets: key.2, written: r.written, failed: r.failed, skipped: r.skipped, changed });
                };
                let (found, r) = write_round(&cfg, State::Running, unix_now());
                emit(State::Running, &found, &r, false);
                let mut beat_at = Instant::now();
                let tick = cfg.check.min(cfg.heartbeat).max(Duration::from_millis(10));
                // a heartbeat or a check each time the wait times out; a stop message or a dropped
                // sender ends it
                while let Err(mpsc::RecvTimeoutError::Timeout) = rx.recv_timeout(tick) {
                    if beat_at.elapsed() >= cfg.heartbeat {
                        beat_at = Instant::now();
                        let (found, r) = write_round(&cfg, State::Running, unix_now());
                        emit(State::Running, &found, &r, false);
                    } else {
                        // between heartbeats: only the files that are not ours (a deploy, an
                        // update, a new install); reported only when one was written or failed
                        let found = discover(cfg.wow_root.as_deref(), cfg.product_db.as_deref());
                        let r = write_targets(&cfg, &found.targets, State::Running, unix_now(), true);
                        if r.written > 0 || r.failed > 0 {
                            emit(State::Running, &found, &r, true);
                        }
                    }
                }
                // "stopped", tried again where the client held the file, for STOP_RETRY at most
                let t0 = Instant::now();
                let found = discover(cfg.wow_root.as_deref(), cfg.product_db.as_deref());
                let mut r = write_targets(&cfg, &found.targets, State::Stopped, unix_now(), false);
                while !r.failed_dirs.is_empty() && t0.elapsed() < STOP_RETRY {
                    std::thread::sleep(Duration::from_millis(100));
                    let again = write_targets(&cfg, &r.failed_dirs, State::Stopped, unix_now(), false);
                    r.written += again.written;
                    r.failed = again.failed;
                    r.failed_dirs = again.failed_dirs;
                }
                emit(State::Stopped, &found, &r, false);
            })
            .ok()?;
        Some(StatusWriter { tx, join: Some(join) })
    }

    /// Writes `stopped` and ends the thread (waits at most [`STOP_WAIT`]).
    pub fn stop(mut self) {
        let _ = self.tx.send(());
        if let Some(j) = self.join.take() {
            let t0 = Instant::now();
            while !j.is_finished() && t0.elapsed() < STOP_WAIT {
                std::thread::sleep(Duration::from_millis(5));
            }
            if j.is_finished() {
                let _ = j.join();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A fresh folder under the system temp folder (never a WoW folder).
    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tp_status_test_{}_{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A fake WoW install: `root\<flavour>\Interface\AddOns\TranslateProbe` with a TOC (listing
    /// the status file when `lists`) and the shipped CompanionStatus.lua.
    fn deploy(root: &Path, flavour: &str, lists: bool) -> PathBuf {
        let d = addon_dir(&root.join(flavour));
        std::fs::create_dir_all(&d).unwrap();
        let toc = if lists { "## Interface: 16001\r\nLocales.lua\r\nCompanionStatus.lua\r\nCore.lua\r\n" } else { "## Interface: 16001\nLocales.lua\nCore.lua\n" };
        std::fs::write(d.join(TOC_NAME), toc).unwrap();
        if lists {
            std::fs::write(d.join(FILE_NAME), "-- shipped\r\nTranslateProbe_CompanionStatus = nil\r\n").unwrap();
        }
        d
    }

    /// A product.db-like protobuf: field 1 (length-delimited) strings with other fields around.
    fn fake_db(paths: &[&str]) -> Vec<u8> {
        let mut b = vec![0x0a, 0x05, b'a', b'g', b'e', b'n', b't', 0x10, 0x01];
        for p in paths {
            let mut inner = vec![0x0a];
            let n = p.len();
            if n < 0x80 {
                inner.push(n as u8);
            } else {
                inner.push((n as u8 & 0x7F) | 0x80);
                inner.push((n >> 7) as u8);
            }
            inner.extend_from_slice(p.as_bytes());
            inner.extend_from_slice(&[0x12, 0x04, b'e', b'n', b'U', b'S', 0x2A, 0x03, b'w', b'o', b'w']);
            b.push(0x12);
            b.push(inner.len() as u8);
            b.extend(inner);
        }
        b
    }

    #[test]
    fn the_file_is_one_table_of_numbers_and_short_ascii_strings() {
        let t = content("0.6.0", 6, State::Running, 1_792_400_000);
        let line = t.lines().find(|l| l.starts_with(GLOBAL)).unwrap();
        assert_eq!(line, "TranslateProbe_CompanionStatus = { v = 1, version = \"0.6.0\", build = 6, state = \"running\", heartbeat = 1792400000 }");
        assert!(t.lines().next().unwrap().starts_with("-- "), "a comment first");
        assert!(t.is_ascii() && t.lines().count() == 2);
        assert!(content("0.6.0", 6, State::Stopped, 5).contains("state = \"stopped\", heartbeat = 5 }"));
        // nothing but safe characters in the version, at most 16
        let odd = content("1.0\"; os.exit() --\u{e9}", 6, State::Running, 1);
        assert!(odd.contains("version = \"1.0os.exit--\""), "{odd}");
        assert_eq!(clean_version(&"9".repeat(40)).len(), 16);
    }

    #[test]
    fn product_db_paths_ending_in_world_of_warcraft() {
        let db = fake_db(&[
            "C:/ProgramData/Battle.net/Agent",
            "Z:/Jeux/Battle.net/World of Warcraft",
            "A:/Jeux/Battle.net/Overwatch",
            "Z:/Jeux/Battle.net/World of Warcraft",
            "D:\\Games\\world of warcraft\\",
            "E:/Not World of Warcraft Launcher",
        ]);
        let r = roots_from_product_db(&db);
        assert_eq!(r, vec![PathBuf::from("Z:/Jeux/Battle.net/World of Warcraft"), PathBuf::from("D:\\Games\\world of warcraft")]);
        // a long path (2-byte length varint)
        let long = format!("F:/{}/World of Warcraft", "x".repeat(150));
        assert_eq!(roots_from_product_db(&fake_db(&[&long])), vec![PathBuf::from(&long)]);
        // a length that does not match is not a path; garbage gives nothing
        let mut bad = fake_db(&["Z:/Jeux/Battle.net/World of Warcraft"]);
        let at = bad.windows(3).position(|w| w == b"Z:/").unwrap();
        bad[at - 1] += 1;
        assert!(roots_from_product_db(&bad).is_empty());
        assert!(roots_from_product_db(&[]).is_empty());
        assert!(roots_from_product_db(b"\x05Z:/World of Warcraft").is_empty(), "too short a length");
        assert!(!is_wow_root("World of Warcraft") && !is_wow_root("Z:/MyWorld of Warcraft") && is_wow_root("Z:\\World of Warcraft"));
    }

    #[test]
    fn product_db_on_this_pc_if_any() {
        // read-only: the real agent database (on the dev PC it names one WoW folder); nothing is written
        let p = product_db_path();
        if let Some(b) = read_capped(&p) {
            let r = roots_from_product_db(&b);
            let targets: usize = r.iter().map(|p| targets_under(p).len()).sum();
            eprintln!("product.db: {} bytes, {} WoW roots, {} addon folders listing the status file", b.len(), r.len(), targets);
            assert!(r.iter().all(|p| p.to_string_lossy().to_ascii_lowercase().ends_with("world of warcraft")), "{} roots", r.len());
        }
    }

    #[test]
    fn flavour_folders_with_a_toc_listing_the_file() {
        let root = temp("flavours").join("World of Warcraft");
        let classic_beta = deploy(&root, "_classic_beta_", true);
        let retail = deploy(&root, "_retail_", true);
        let forever = deploy(&root, "_forever_", true);
        deploy(&root, "_classic_", false); // an older addon: its TOC does not list the file
        std::fs::create_dir_all(root.join("_anniversary_")).unwrap(); // no addon there
        std::fs::create_dir_all(addon_dir(&root.join("Data"))).unwrap(); // not a flavour folder
        let t = targets_under(&root);
        assert_eq!(t.len(), 3, "{t:?}");
        for d in [&retail, &classic_beta, &forever] {
            assert!(t.contains(d));
        }
        // wowRoot naming one flavour folder
        assert_eq!(targets_under(&root.join("_classic_beta_")), vec![classic_beta.clone()]);
        // discovery from settings: only that root; from a product.db: its roots
        let f = discover(Some(&root), None);
        assert_eq!((f.roots, f.targets.len()), (1, 3));
        let db_path = root.parent().unwrap().join("product.db");
        std::fs::write(&db_path, fake_db(&[&root.to_string_lossy(), "C:/ProgramData/Battle.net/Agent"])).unwrap();
        let f = discover(None, Some(&db_path));
        assert_eq!((f.roots, f.targets.len()), (1, 3));
        assert_eq!(discover(None, Some(&root.join("missing.db"))), Found::default());
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn toc_listing_is_a_line_of_its_own() {
        let d = temp("toc");
        let toc = d.join(TOC_NAME);
        for (text, want) in [
            ("\u{feff}CompanionStatus.lua\nCore.lua", true),
            ("  companionstatus.lua  \r\n", true),
            ("# CompanionStatus.lua\nCore.lua", false),
            ("Old\\CompanionStatus.lua", false),
            ("", false),
        ] {
            std::fs::write(&toc, text).unwrap();
            assert_eq!(toc_lists_status(&toc), want, "{text:?}");
        }
        assert!(!toc_lists_status(&d.join("none.toc")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn atomic_writes_replace_the_shipped_file_and_leave_no_temporary_file() {
        let root = temp("atomic");
        let d = deploy(&root, "_retail_", true);
        let before: Vec<_> = std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name()).collect();
        write_atomic(&d, &content("0.6.0", 6, State::Running, 100)).unwrap();
        write_atomic(&d, &content("0.6.0", 6, State::Stopped, 200)).unwrap();
        let text = std::fs::read_to_string(d.join(FILE_NAME)).unwrap();
        assert!(text.contains("state = \"stopped\", heartbeat = 200"));
        let after: Vec<_> = std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(before, after, "only the status file changed, no temporary file left");
        // a folder that does not exist: an error, nothing created
        assert!(write_atomic(&root.join("nowhere"), "x").is_err());
        assert!(!root.join("nowhere").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_reader_holding_the_file_delays_the_replace_but_never_sees_half_a_file() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = temp("locked");
        let d = deploy(&root, "_retail_", true);
        // a reader that shares reading only (no delete): MoveFileEx cannot replace the file
        let held = std::fs::OpenOptions::new().read(true).share_mode(1).open(d.join(FILE_NAME)).unwrap();
        let t0 = Instant::now();
        assert!(write_atomic(&d, &content("0.6.0", 6, State::Running, 1)).is_err());
        assert!(t0.elapsed() >= Duration::from_millis(REPLACE_RETRY_MS * u64::from(REPLACE_RETRIES)), "retried");
        assert!(!d.join(TMP_NAME).exists());
        assert!(std::fs::read_to_string(d.join(FILE_NAME)).unwrap().contains("= nil"), "the shipped file is intact");
        drop(held);
        write_atomic(&d, &content("0.6.0", 6, State::Running, 2)).unwrap();
        assert!(std::fs::read_to_string(d.join(FILE_NAME)).unwrap().contains("heartbeat = 2 }"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_writer_writes_running_at_once_each_heartbeat_and_stopped_at_the_end() {
        let root = temp("writer").join("World of Warcraft");
        let a = deploy(&root, "_classic_beta_", true);
        let b = deploy(&root, "_ptr_", true);
        let old = deploy(&root, "_classic_", false);
        let reports: Arc<Mutex<Vec<Report>>> = Arc::new(Mutex::new(Vec::new()));
        let r2 = reports.clone();
        let cfg = WriterConfig { wow_root: Some(root.clone()), product_db: None, version: "0.6.0".into(), build: 6, heartbeat: Duration::from_millis(150), check: Duration::from_secs(5) };
        let w = StatusWriter::start(cfg, Arc::new(move |r| r2.lock().unwrap().push(r))).unwrap();
        let t0 = Instant::now();
        while reports.lock().unwrap().len() < 3 && t0.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let running = std::fs::read_to_string(a.join(FILE_NAME)).unwrap();
        assert!(running.contains("state = \"running\""), "{running}");
        let beat: u64 = running.rsplit("heartbeat = ").next().unwrap().trim_end_matches([' ', '}', '\r', '\n']).parse().unwrap();
        assert!(beat.abs_diff(unix_now()) <= 2);
        w.stop();
        for d in [&a, &b] {
            assert!(std::fs::read_to_string(d.join(FILE_NAME)).unwrap().contains("state = \"stopped\""));
            assert!(!d.join(TMP_NAME).exists());
        }
        assert!(!old.join(FILE_NAME).exists(), "an addon whose TOC does not list it never gets the file");
        let r = reports.lock().unwrap();
        assert!(r.len() >= 4, "{r:?}");
        assert_eq!((r[0].state, r[0].roots, r[0].targets, r[0].written, r[0].failed, r[0].changed), (State::Running, 1, 2, 2, 0, true));
        assert!(!r[1].changed, "an unchanged round is not news");
        let last = r.last().unwrap();
        assert_eq!((last.state, last.written, last.changed), (State::Stopped, 2, true));
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn a_hard_link_at_the_temporary_name_is_never_written_through() {
        let root = temp("hardlink");
        let d = deploy(&root, "_retail_", true);
        let victim = root.join("victim.txt");
        std::fs::write(&victim, "victim").unwrap();
        std::fs::hard_link(&victim, d.join(TMP_NAME)).unwrap();
        write_atomic(&d, &content("0.6.0", 6, State::Running, 7)).unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "victim", "the other name keeps its content");
        assert!(std::fs::read_to_string(d.join(FILE_NAME)).unwrap().contains("heartbeat = 7 }"));
        assert!(!d.join(TMP_NAME).exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `mklink /J link target` (a junction needs no privilege); false when it could not be made.
    fn junction(link: &Path, target: &Path) -> bool {
        let cmd = std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".into());
        std::process::Command::new(cmd)
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .is_ok_and(|o| o.status.success())
            && link.exists()
    }

    #[test]
    fn an_addon_folder_that_is_a_junction_is_skipped_and_counted() {
        let root = temp("junction").join("World of Warcraft");
        // the "repo": an addon folder outside the WoW folder, with the shipped file
        let repo = root.parent().unwrap().join("repo").join("TranslateProbe");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join(TOC_NAME), "CompanionStatus.lua\nCore.lua\n").unwrap();
        std::fs::write(repo.join(FILE_NAME), "TranslateProbe_CompanionStatus = nil\n").unwrap();
        let addons = root.join("_retail_").join("Interface").join("AddOns");
        std::fs::create_dir_all(&addons).unwrap();
        let link = addons.join("TranslateProbe");
        if !junction(&link, &repo) {
            eprintln!("no junction could be made here: skipped");
            let _ = std::fs::remove_dir_all(root.parent().unwrap());
            return;
        }
        let real = deploy(&root, "_classic_beta_", true);
        assert!(is_reparse_point(&link) && !is_reparse_point(&real));
        let cfg = WriterConfig { wow_root: Some(root.clone()), product_db: None, version: "0.6.0".into(), build: 6, heartbeat: HEARTBEAT, check: CHECK };
        let (found, r) = write_round(&cfg, State::Running, 9);
        assert_eq!((found.targets.len(), r.written, r.failed, r.skipped), (2, 1, 0, 1));
        assert_eq!(std::fs::read_to_string(repo.join(FILE_NAME)).unwrap(), "TranslateProbe_CompanionStatus = nil\n", "the repo's file is untouched");
        assert!(holds_own(&real, State::Running));
        assert!(write_atomic(&link, "x").is_err_and(|e| e.kind() == std::io::ErrorKind::Unsupported));
        // the junction goes (rmdir removes the link, not the target), then the rest
        std::fs::remove_dir(&link).unwrap();
        assert!(repo.join(FILE_NAME).exists());
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn a_file_that_is_not_ours_is_written_at_the_next_check() {
        let root = temp("check").join("World of Warcraft");
        let a = deploy(&root, "_classic_beta_", true);
        let reports: Arc<Mutex<Vec<Report>>> = Arc::new(Mutex::new(Vec::new()));
        let r2 = reports.clone();
        let cfg = WriterConfig { wow_root: Some(root.clone()), product_db: None, version: "0.6.0".into(), build: 6, heartbeat: Duration::from_secs(600), check: Duration::from_millis(100) };
        let w = StatusWriter::start(cfg, Arc::new(move |r| r2.lock().unwrap().push(r))).unwrap();
        let wait = |pred: &dyn Fn() -> bool| {
            let t0 = Instant::now();
            while !pred() && t0.elapsed() < Duration::from_secs(5) {
                std::thread::sleep(Duration::from_millis(10));
            }
            pred()
        };
        assert!(wait(&|| holds_own(&a, State::Running)), "running at start");
        // checks that find their own file write nothing and report nothing
        std::thread::sleep(Duration::from_millis(350));
        let n = reports.lock().unwrap().len();
        assert_eq!(n, 1, "only the start round");
        let before = std::fs::read_to_string(a.join(FILE_NAME)).unwrap();
        // a deploy puts the shipped file back: written again within a check, long before a heartbeat
        std::fs::write(a.join(FILE_NAME), "TranslateProbe_CompanionStatus = nil\r\n").unwrap();
        assert!(wait(&|| holds_own(&a, State::Running)), "written again at the next check");
        // a new install (another flavour) is found by a check too
        let b = deploy(&root, "_ptr_", true);
        assert!(wait(&|| holds_own(&b, State::Running)), "a new addon folder gets it at the next check");
        w.stop();
        assert!(holds_own(&a, State::Stopped) && holds_own(&b, State::Stopped));
        let r = reports.lock().unwrap();
        assert!(r.len() >= 4 && r[1..r.len() - 1].iter().all(|x| x.state == State::Running && x.written == 1 && x.changed), "{r:?}");
        assert!(before.starts_with(HEADER));
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn stopped_is_tried_again_while_the_client_holds_the_file() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = temp("stopretry").join("World of Warcraft");
        let a = deploy(&root, "_classic_beta_", true);
        let cfg = WriterConfig { wow_root: Some(root.clone()), product_db: None, version: "0.6.0".into(), build: 6, heartbeat: HEARTBEAT, check: CHECK };
        let reports: Arc<Mutex<Vec<Report>>> = Arc::new(Mutex::new(Vec::new()));
        let r2 = reports.clone();
        let w = StatusWriter::start(cfg, Arc::new(move |r| r2.lock().unwrap().push(r))).unwrap();
        let t0 = Instant::now();
        while !holds_own(&a, State::Running) && t0.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        // a reader that shares reading only holds the file for 600 ms while the Helper stops
        let held = std::fs::OpenOptions::new().read(true).share_mode(1).open(a.join(FILE_NAME)).unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(600));
            drop(held);
        });
        w.stop();
        release.join().unwrap();
        assert!(holds_own(&a, State::Stopped), "stopped once the reader let go");
        let last = *reports.lock().unwrap().last().unwrap();
        assert_eq!((last.state, last.written, last.failed), (State::Stopped, 1, 0));
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }
}
