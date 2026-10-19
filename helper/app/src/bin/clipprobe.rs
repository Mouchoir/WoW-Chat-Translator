//! The one live check of the Helper's real clipboard path (decision D9, run by hand after a
//! build; the automated tests use a fake clipboard and every E2E run uses `--no-clipboard`).
//!
//! `clipprobe live`:
//! 1. lists the formats on the clipboard; when anything other than plain text is there (an
//!    image, files, rich text, an application's own format), nothing is touched and the probe
//!    reports "skipped" (exit code 3). The same when text is listed but could not be read (a
//!    delayed rendering whose owner does not answer): only a clipboard whose text was really read,
//!    or a clipboard with no format at all, goes on;
//! 2. keeps the current Unicode text in memory (never printed, never written anywhere, never put
//!    on a command line, not even as a length or a hash) with the sequence number read while the
//!    clipboard was open;
//! 3. runs the Helper's own reply flow (`clip::ReplyClip` on `clip::WinClip`): `begin` empties the
//!    clipboard (refused, and the probe skipped, when the sequence number moved since step 2:
//!    something was copied in between), `finish` writes a synthetic sentence with the
//!    history-exclusion formats;
//! 4. a separate child process (`clipprobe check`, no argument) reads it back as a paste would and
//!    checks the exclusion formats (`CanIncludeInClipboardHistory` = 0,
//!    `CanUploadToCloudClipboard` = 0, `ExcludeClipboardContentFromMonitorProcessing` present); it
//!    prints a length and a hash of what it read on its stdout pipe, which only this process
//!    reads and compares, and no text;
//! 5. puts the saved text back exactly (kept out of the history again, so the Win+V history is
//!    unchanged) only when the clipboard still holds the probe's own last change (its write, or
//!    its empty when nothing was written), checked with the clipboard open; anything copied
//!    meanwhile is left alone and reported. A child checks the restore.
//!
//! Output: one JSON line of verdicts and numbers. Exit code 0 when every step passed, 1 when one
//! failed, 3 when skipped, 64 for bad arguments.

use std::process::Command;

use tp_helper::clip::{exclusion_formats, Begin, ClipOs, ClipOutcome, Permit, ReplyClip, WinClip, CF_UNICODETEXT};
use windows::core::w;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData, GetClipboardSequenceNumber, OpenClipboard, SetClipboardData};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE};

const CF_TEXT: u32 = 1;
const CF_OEMTEXT: u32 = 7;
const CF_LOCALE: u32 = 16;
/// The synthetic sentence written by the live check (accents, Cyrillic and CJK, a line break that
/// the reply flow turns into a space).
const SYNTHETIC: &str = "TranslateProbe clipprobe: Ça va ? On part dans 5 minutes\r\n- Привет 你好";

const USAGE: &str = "clipprobe live   (the one live clipboard check of decision D9; see the source header)\n";

/// FNV-1a over UTF-16 units (the clipboard's own form, so a text is compared exactly, unpaired
/// surrogates included).
fn fnv(units: &[u16]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for u in units {
        for b in u.to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x100_0000_01b3);
        }
    }
    h
}

fn units(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// Puts `text` (UTF-16 units, without the NUL) back exactly, kept out of the clipboard history
/// like the Helper's own writes (the history already holds it from when it was copied). The
/// clipboard must be open and owned.
fn put_units(text: &[u16]) -> bool {
    let set = |format: u32, bytes: &[u8]| unsafe {
        let Ok(h) = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)) else { return false };
        let p = GlobalLock(h) as *mut u8;
        if p.is_null() {
            let _ = GlobalFree(Some(h));
            return false;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        let _ = GlobalUnlock(h);
        if SetClipboardData(format, Some(HANDLE(h.0))).is_ok() {
            true
        } else {
            let _ = GlobalFree(Some(h));
            false
        }
    };
    let bytes: Vec<u8> = text.iter().copied().chain(std::iter::once(0)).flat_map(|u| u.to_le_bytes()).collect();
    let ok = unsafe { EmptyClipboard() }.is_ok() && set(CF_UNICODETEXT, &bytes);
    let zero = 0u32.to_le_bytes();
    for f in exclusion_formats() {
        if f != 0 {
            set(f, &zero);
        }
    }
    ok
}

/// True when every format is plain text (or one of the exclusion markers): nothing would be lost
/// by saving the Unicode text and putting it back.
fn only_plain_text(formats: &[u32], exclusion: &[u32; 3]) -> bool {
    formats.iter().all(|f| [CF_TEXT, CF_OEMTEXT, CF_UNICODETEXT, CF_LOCALE].contains(f) || exclusion.contains(f))
}

/// A message-only window owning the clipboard while the probe writes (like the Helper's hidden
/// main window).
fn owner() -> Option<HWND> {
    unsafe {
        let inst = GetModuleHandleW(None).ok()?;
        CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("clipprobe"), WINDOW_STYLE(0), 0, 0, 0, 0, Some(HWND_MESSAGE), None, Some(inst.into()), None).ok()
    }
}

/// Opens the clipboard (3 tries, 30 ms apart).
fn open(h: HWND) -> bool {
    for i in 0..3 {
        if unsafe { OpenClipboard(Some(h)) }.is_ok() {
            return true;
        }
        if i < 2 {
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
    }
    false
}

/// What `read` found.
struct Read {
    formats: Vec<u32>,
    /// The Unicode text (UTF-16 units, without the NUL), None when absent or unreadable.
    text: Option<Vec<u16>>,
    /// (id, DWORD value) of each exclusion format.
    dwords: Vec<(u32, Option<u32>)>,
    /// `GetClipboardSequenceNumber` while the clipboard was open (nothing can change it then).
    seq: u32,
}

/// The formats on the clipboard and its Unicode text (read for the save, or by the child).
fn read(h: HWND) -> Option<Read> {
    if !open(h) {
        return None;
    }
    let seq = unsafe { GetClipboardSequenceNumber() };
    let mut formats = Vec::new();
    let mut f = 0u32;
    loop {
        f = unsafe { EnumClipboardFormats(f) };
        if f == 0 {
            break;
        }
        formats.push(f);
    }
    let text = if formats.contains(&CF_UNICODETEXT) {
        unsafe {
            GetClipboardData(CF_UNICODETEXT).ok().and_then(|hd| {
                let hg = HGLOBAL(hd.0);
                let p = GlobalLock(hg) as *const u16;
                if p.is_null() {
                    return None;
                }
                let max = GlobalSize(hg) / 2;
                let mut len = 0;
                while len < max && *p.add(len) != 0 {
                    len += 1;
                }
                let s = std::slice::from_raw_parts(p, len).to_vec();
                let _ = GlobalUnlock(hg);
                Some(s)
            })
        }
    } else {
        None
    };
    // the DWORD value of each exclusion format present
    let dwords: Vec<(u32, Option<u32>)> = exclusion_formats()
        .iter()
        .map(|&id| {
            let v = if formats.contains(&id) {
                unsafe {
                    GetClipboardData(id).ok().and_then(|hd| {
                        let hg = HGLOBAL(hd.0);
                        let p = GlobalLock(hg) as *const u8;
                        if p.is_null() || GlobalSize(hg) < 4 {
                            if !p.is_null() {
                                let _ = GlobalUnlock(hg);
                            }
                            return None;
                        }
                        let v = u32::from_le_bytes([*p, *p.add(1), *p.add(2), *p.add(3)]);
                        let _ = GlobalUnlock(hg);
                        Some(v)
                    })
                }
            } else {
                None
            };
            (id, v)
        })
        .collect();
    unsafe {
        let _ = CloseClipboard();
    }
    Some(Read { formats, text, dwords, seq })
}

/// Child: reads the clipboard as a paste would and prints, on its stdout pipe (read by the parent
/// only), the length and hash of the text it read plus the exclusion verdicts; never the text.
/// The expected values never travel on a command line.
fn check() -> i32 {
    let Some(h) = owner() else { return 1 };
    let r = read(h);
    unsafe {
        let _ = DestroyWindow(h);
    }
    let Some(r) = r else {
        println!("{{\"read\":false}}");
        return 1;
    };
    let (units, hash) = match &r.text {
        Some(t) => (t.len().to_string(), format!("\"{}\"", fnv(t))),
        None => ("null".to_string(), "null".to_string()),
    };
    let history = r.dwords[0].1 == Some(0);
    let cloud = r.dwords[1].1 == Some(0);
    let monitor = r.formats.contains(&r.dwords[2].0);
    println!(
        "{{\"read\":true,\"units\":{units},\"hash\":{hash},\"historyExcluded\":{history},\"cloudExcluded\":{cloud},\"monitorExcluded\":{monitor},\"formats\":{}}}",
        r.formats.len()
    );
    0
}

fn child(args: &[&str]) -> Option<serde_json::Value> {
    let exe = std::env::current_exe().ok()?;
    let out = Command::new(exe).args(args).output().ok()?;
    serde_json::from_slice(String::from_utf8_lossy(&out.stdout).trim().as_bytes()).ok()
}

/// Whether a child's reading equals `want` (None: no text on the clipboard).
fn matches(reading: &Option<serde_json::Value>, want: Option<&[u16]>) -> bool {
    let Some(v) = reading else { return false };
    if v.get("read").and_then(|x| x.as_bool()) != Some(true) {
        return false;
    }
    match want {
        None => v.get("units").is_some_and(|x| x.is_null()),
        Some(t) => {
            v.get("units").and_then(|x| x.as_u64()) == Some(t.len() as u64) && v.get("hash").and_then(|x| x.as_str()) == Some(fnv(t).to_string().as_str())
        }
    }
}

/// Whether the probe may go on with what `read` found: only plain text, and the text really read
/// (or nothing at all on the clipboard). Otherwise the reason to skip.
fn may_touch(formats: &[u32], text_read: bool, exclusion: &[u32; 3]) -> Result<(), &'static str> {
    if !only_plain_text(formats, exclusion) {
        return Err("the clipboard holds something other than plain text: nothing touched");
    }
    if !formats.is_empty() && !text_read {
        return Err("the clipboard text could not be read: nothing touched");
    }
    Ok(())
}

/// The real clipboard for the probe's `begin`: it refuses to open once the sequence number moved
/// away from `expect` (something was copied since the text was saved), so nothing copied in
/// between is ever emptied.
struct Guarded {
    inner: WinClip,
    expect: Option<u32>,
    moved: bool,
}

impl ClipOs for Guarded {
    fn seq(&self) -> u32 {
        self.inner.seq()
    }

    fn open(&mut self) -> bool {
        if !self.inner.open() {
            return false;
        }
        if self.expect.is_some_and(|e| self.inner.seq() != e) {
            self.inner.close();
            self.moved = true;
            return false;
        }
        true
    }

    fn empty(&mut self) -> bool {
        self.inner.empty()
    }

    fn set_unicode(&mut self, text: &str) -> Option<bool> {
        self.inner.set_unicode(text)
    }

    fn close(&mut self) {
        self.inner.close()
    }
}

/// The live check (decision D9).
fn live() -> i32 {
    let Some(h) = owner() else {
        println!("{{\"error\":\"no window\"}}");
        return 1;
    };
    let code = live_with(h);
    unsafe {
        let _ = DestroyWindow(h);
    }
    code
}

fn live_with(h: HWND) -> i32 {
    let excl = exclusion_formats();
    let Some(before) = read(h) else {
        println!("{{\"skipped\":\"the clipboard could not be opened\"}}");
        return 3;
    };
    if let Err(why) = may_touch(&before.formats, before.text.is_some(), &excl) {
        println!("{}", serde_json::json!({ "skipped": why, "formats": before.formats.len() }));
        return 3;
    }
    let saved = before.text;
    let mut os = Guarded { inner: WinClip::new(h.0 as isize), expect: Some(before.seq), moved: false };
    // the Helper's own reply flow; begin empties only a clipboard nobody changed since the save
    let mut reply = ReplyClip::new(Permit::Allowed);
    let begin = reply.begin(&mut os);
    if os.moved {
        println!("{}", serde_json::json!({ "skipped": "something was copied before the probe started: nothing touched" }));
        return 3;
    }
    os.expect = None;
    let emptied_seq = match begin {
        Begin::Emptied(s) => Some(s),
        _ => None,
    };
    let emptied = emptied_seq.is_some() && read(h).is_some_and(|r| r.formats.is_empty());
    let outcome = reply.finish(&mut os, SYNTHETIC);
    let expected = units(&tp_helper::clip::one_line_255(SYNTHETIC).0);
    let back = child(&["check"]);
    let field = |v: &Option<serde_json::Value>, k: &str| v.as_ref().and_then(|v| v.get(k)).and_then(|x| x.as_bool()).unwrap_or(false);
    // the probe's own last change: its write, else its empty (nothing written); the saved text
    // goes back only over that (checked with the clipboard open, when nobody can change it)
    let last_own = if outcome.copied() { reply.own_write() } else { emptied_seq };
    let mut copied_meanwhile = false;
    let restored = match last_own {
        Some(own) if open(h) => {
            let ok = if unsafe { GetClipboardSequenceNumber() } != own {
                copied_meanwhile = true;
                false
            } else {
                match &saved {
                    Some(t) => put_units(t),
                    None => unsafe { EmptyClipboard() }.is_ok(),
                }
            };
            unsafe {
                let _ = CloseClipboard();
            }
            ok
        }
        _ => false,
    };
    let verify = child(&["check"]);
    let child_ok = matches(&back, Some(&expected));
    let restore_ok = matches(&verify, saved.as_deref());
    let pass = emptied && outcome == ClipOutcome::Copied { cut: false } && child_ok && field(&back, "historyExcluded") && field(&back, "cloudExcluded") && restored && restore_ok;
    println!(
        "{}",
        serde_json::json!({
            "skipped": false, "savedText": saved.is_some(), "begin": begin.name(), "emptied": emptied, "finish": outcome.name(),
            "childReadOk": child_ok, "historyExcluded": field(&back, "historyExcluded"), "cloudExcluded": field(&back, "cloudExcluded"),
            "monitorExcluded": field(&back, "monitorExcluded"), "copiedMeanwhile": copied_meanwhile, "restored": restored,
            "restoreVerified": restore_ok, "pass": pass,
        })
    );
    i32::from(!pass)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["live"] => live(),
        ["check"] => check(),
        ["--help"] | ["-h"] => {
            print!("{USAGE}");
            0
        }
        _ => {
            eprint!("{USAGE}");
            64
        }
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_only() {
        let ex = [49_500, 49_501, 49_502];
        assert!(only_plain_text(&[], &ex), "an empty clipboard");
        assert!(only_plain_text(&[CF_UNICODETEXT, CF_LOCALE, CF_TEXT, CF_OEMTEXT], &ex));
        assert!(only_plain_text(&[CF_UNICODETEXT, 49_500], &ex), "text kept out of the history");
        assert!(!only_plain_text(&[CF_UNICODETEXT, 49_161], &ex), "HTML or another application format");
        assert!(!only_plain_text(&[2], &ex), "a bitmap");
        assert!(!only_plain_text(&[15], &ex), "files (CF_HDROP)");
    }

    #[test]
    fn unreadable_text_is_never_touched() {
        let ex = [49_500, 49_501, 49_502];
        assert!(may_touch(&[], false, &ex).is_ok(), "an empty clipboard");
        assert!(may_touch(&[CF_UNICODETEXT, CF_LOCALE], true, &ex).is_ok(), "text that was read");
        assert!(may_touch(&[CF_UNICODETEXT, CF_LOCALE], false, &ex).is_err(), "text listed but not read (review finding)");
        assert!(may_touch(&[49_500], false, &ex).is_err(), "formats but no text");
        assert!(may_touch(&[CF_UNICODETEXT, 2], true, &ex).is_err(), "a bitmap too");
    }

    #[test]
    fn a_child_reading_is_compared_here() {
        let t = units("abc");
        let ok = Some(serde_json::json!({"read": true, "units": 3, "hash": fnv(&t).to_string()}));
        assert!(matches(&ok, Some(&t)));
        assert!(!matches(&ok, Some(&units("abd"))));
        assert!(!matches(&ok, None));
        let empty = Some(serde_json::json!({"read": true, "units": null, "hash": null}));
        assert!(matches(&empty, None) && !matches(&empty, Some(&t)));
        assert!(!matches(&Some(serde_json::json!({"read": false})), None));
        assert!(!matches(&None, None));
    }

    #[test]
    fn synthetic_sentence_is_one_line() {
        let (s, cut) = tp_helper::clip::one_line_255(SYNTHETIC);
        assert!(!cut && !s.contains('\n') && s.contains("Привет"));
        assert_ne!(fnv(&units("a")), fnv(&units("b")));
        // an unpaired surrogate is compared as it is (the restore is exact)
        assert_ne!(fnv(&[0xD800]), fnv(&[0xFFFD]));
    }
}
