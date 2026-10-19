//! The clipboard for translated replies (conversation window, design A.6.2-A.6.4 and A.11;
//! decisions D2, D6, D9).
//!
//! Only a reply request (TRANSLATE with the reply bit) that carries the copy bit (the player
//! allowed it in the game's consent dialog) can touch the clipboard, and only while the tray item
//! "Copy translated replies to the clipboard" is on and `--no-clipboard` is not given (it is
//! implied by `--no-overlay`, so automated runs never touch it). An incoming message is never
//! copied: copy without reply is refused by the parser.
//!
//! 1. [`ReplyClip::begin`] (the request arrives): the clipboard is emptied at once (D2), so a
//!    Ctrl+V pressed before the translation is ready pastes nothing instead of something old.
//! 2. [`ReplyClip::finish`] (the final translation): the text is written only when the clipboard
//!    sequence number is still the one of that empty (the player copied nothing meanwhile), or
//!    still the one seen before it when the clipboard could not be emptied. Partial results are
//!    never copied.
//! 3. [`ReplyClip::release`] (CANCEL, a new reply, a session change, graceful exit): the clipboard
//!    is emptied only when it still holds the Helper's own write.
//!
//! The Helper never reads the clipboard's content: it compares sequence numbers only
//! (`GetClipboardSequenceNumber`). It writes `CF_UNICODETEXT` plus the formats that keep the
//! text out of the Win+V history and the cloud clipboard (`CanIncludeInClipboardHistory` = 0,
//! `CanUploadToCloudClipboard` = 0, `ExcludeClipboardContentFromMonitorProcessing`), with its
//! hidden main window `TPHelperMain` as the owner. Every Windows call goes through [`ClipOs`], so
//! the tests drive a fake one and never touch the user's clipboard (D9).

use std::time::Duration;

/// WoW sends at most 255 bytes per chat message.
pub const CHAT_MAX_BYTES: usize = 255;
/// Attempts to open the clipboard (another program may hold it open for a moment).
pub const TRIES: u32 = 3;
/// Wait between two attempts.
pub const TRY_GAP_MS: u64 = 30;

/// The Windows clipboard calls the reply flow needs (a fake one in tests).
pub trait ClipOs {
    /// `GetClipboardSequenceNumber`.
    fn seq(&self) -> u32;
    /// `OpenClipboard(owner)`; false when another program holds it.
    fn open(&mut self) -> bool;
    /// `EmptyClipboard` (the clipboard must be open).
    fn empty(&mut self) -> bool;
    /// Writes `text` as `CF_UNICODETEXT` plus the history-exclusion formats (the clipboard must be
    /// open and emptied by this owner). None when the text could not be set; Some(excluded) with
    /// excluded true when every exclusion format was set too.
    fn set_unicode(&mut self, text: &str) -> Option<bool>;
    /// `CloseClipboard`.
    fn close(&mut self);
    /// Waits between two attempts to open the clipboard.
    fn wait(&mut self, ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }
}

/// Whether a reply may be copied, decided when its request arrives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permit {
    /// Copy bit set, tray switch on.
    Allowed,
    /// No copy bit: the player refused in the consent dialog.
    NoConsent,
    /// The tray switch is off, or `--no-clipboard`.
    Off,
}

impl Permit {
    pub fn new(copy_bit: bool, switch_on: bool) -> Permit {
        match (copy_bit, switch_on) {
            (false, _) => Permit::NoConsent,
            (true, false) => Permit::Off,
            (true, true) => Permit::Allowed,
        }
    }
}

/// What [`ReplyClip::begin`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Begin {
    /// Emptied; the sequence number after the empty.
    Emptied(u32),
    /// The clipboard could not be opened or emptied (it is left as it was).
    Busy,
    /// Not allowed (no consent, or the switch is off): nothing touched.
    NotAllowed,
}

impl Begin {
    pub fn name(self) -> &'static str {
        match self {
            Begin::Emptied(_) => "emptied",
            Begin::Busy => "busy",
            Begin::NotAllowed => "not_allowed",
        }
    }
}

/// What [`ReplyClip::finish`] did (no text: loggable).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipOutcome {
    /// Written; `cut` when the text was shortened to 255 bytes.
    Copied { cut: bool },
    /// The player copied something after clicking Translate: left alone.
    Changed,
    /// The clipboard stayed busy for every attempt.
    Busy,
    /// No consent (no copy bit).
    NotAllowed,
    /// The tray switch is off, or `--no-clipboard`.
    Off,
}

impl ClipOutcome {
    pub fn name(self) -> &'static str {
        match self {
            ClipOutcome::Copied { cut: false } => "copied",
            ClipOutcome::Copied { cut: true } => "copied_cut",
            ClipOutcome::Changed => "changed",
            ClipOutcome::Busy => "busy",
            ClipOutcome::NotAllowed => "not_allowed",
            ClipOutcome::Off => "off",
        }
    }

    pub fn copied(self) -> bool {
        matches!(self, ClipOutcome::Copied { .. })
    }
}

/// The clipboard side of one reply request.
#[derive(Clone, Debug)]
pub struct ReplyClip {
    permit: Permit,
    begin: Option<Begin>,
    /// Sequence number before `begin` tried to empty the clipboard.
    seq0: u32,
    /// Sequence number after the Helper's own write (None until a write succeeded).
    own_write: Option<u32>,
    outcome: Option<ClipOutcome>,
    /// The last write also set every history-exclusion format.
    pub history_excluded: bool,
}

/// Opens the clipboard, trying `TRIES` times `TRY_GAP_MS` apart.
fn open_tries(os: &mut dyn ClipOs) -> bool {
    for i in 0..TRIES {
        if os.open() {
            return true;
        }
        if i + 1 < TRIES {
            os.wait(TRY_GAP_MS);
        }
    }
    false
}

impl ReplyClip {
    pub fn new(permit: Permit) -> ReplyClip {
        ReplyClip { permit, begin: None, seq0: 0, own_write: None, outcome: None, history_excluded: false }
    }

    pub fn permit(&self) -> Permit {
        self.permit
    }

    /// The result of `finish`, once it ran.
    pub fn outcome(&self) -> Option<ClipOutcome> {
        self.outcome
    }

    /// The clipboard sequence number right after the Helper's own write (None until a write
    /// succeeded, and after `release`).
    pub fn own_write(&self) -> Option<u32> {
        self.own_write
    }

    /// The reply request arrived: empty the clipboard at once when allowed (D2).
    pub fn begin(&mut self, os: &mut dyn ClipOs) -> Begin {
        if self.permit != Permit::Allowed {
            self.begin = Some(Begin::NotAllowed);
            return Begin::NotAllowed;
        }
        self.seq0 = os.seq();
        let b = if open_tries(os) {
            let ok = os.empty();
            os.close();
            if ok {
                Begin::Emptied(os.seq())
            } else {
                Begin::Busy
            }
        } else {
            Begin::Busy
        };
        self.begin = Some(b);
        b
    }

    /// The final translation (or the original text, when it is already in the target language):
    /// copies it when allowed and the player copied nothing since `begin`. Called once; a second
    /// call returns the first outcome without touching the clipboard.
    pub fn finish(&mut self, os: &mut dyn ClipOs, text: &str) -> ClipOutcome {
        if let Some(o) = self.outcome {
            return o;
        }
        let o = self.write(os, text);
        self.outcome = Some(o);
        o
    }

    fn write(&mut self, os: &mut dyn ClipOs, text: &str) -> ClipOutcome {
        match self.permit {
            Permit::NoConsent => return ClipOutcome::NotAllowed,
            Permit::Off => return ClipOutcome::Off,
            Permit::Allowed => {}
        }
        // the sequence number the clipboard must still have: after our empty, or before the
        // empty that failed (begin not run: whatever it is now)
        let expected = match self.begin {
            Some(Begin::Emptied(s)) => s,
            Some(Begin::Busy) => self.seq0,
            _ => os.seq(),
        };
        if os.seq() != expected {
            return ClipOutcome::Changed;
        }
        let (line, cut) = one_line_255(text);
        if !open_tries(os) {
            return ClipOutcome::Busy;
        }
        // nobody can change it while it is open: check again after opening
        if os.seq() != expected {
            os.close();
            return ClipOutcome::Changed;
        }
        let set = if os.empty() { os.set_unicode(&line) } else { None };
        os.close();
        match set {
            Some(excluded) => {
                self.history_excluded = excluded;
                self.own_write = Some(os.seq());
                ClipOutcome::Copied { cut }
            }
            None => ClipOutcome::Busy,
        }
    }

    /// The reply ended (CANCEL, replaced, session change, exit): empties the clipboard when it
    /// still holds the Helper's own write. Returns true when it emptied it.
    pub fn release(&mut self, os: &mut dyn ClipOs) -> bool {
        let Some(own) = self.own_write.take() else { return false };
        if os.seq() != own || !open_tries(os) {
            return false;
        }
        let ok = os.seq() == own && os.empty();
        os.close();
        ok
    }
}

/// UTF-8 bytes a character costs on the chat wire: `|` is escaped as `||` by the addon.
fn wire_len(c: char) -> usize {
    if c == '|' {
        2
    } else {
        c.len_utf8()
    }
}

/// One chat line of at most 255 wire bytes: line breaks and tabs become spaces, other control
/// characters (the 0x02/0x03 link marks among them) are removed, runs of spaces collapse and the
/// ends are trimmed; a longer text is cut at the last word boundary that fits (or at a character
/// boundary when one word is longer than that). `|` counts 2 bytes (the addon doubles it). The
/// bool is true when the text was cut.
pub fn one_line_255(text: &str) -> (String, bool) {
    let mut s = String::with_capacity(text.len().min(1024));
    let mut space = false;
    for c in text.chars() {
        let c = match c {
            '\r' | '\n' | '\t' => ' ',
            c if c.is_control() => continue,
            c => c,
        };
        if c == ' ' {
            if !space && !s.is_empty() {
                s.push(' ');
            }
            space = true;
        } else {
            s.push(c);
            space = false;
        }
    }
    while s.ends_with(' ') {
        s.pop();
    }
    let total: usize = s.chars().map(wire_len).sum();
    if total <= CHAT_MAX_BYTES {
        return (s, false);
    }
    // the longest prefix that fits, on a character boundary
    let mut used = 0;
    let mut end = 0;
    for (i, c) in s.char_indices() {
        if used + wire_len(c) > CHAT_MAX_BYTES {
            break;
        }
        used += wire_len(c);
        end = i + c.len_utf8();
    }
    // back to the last space when the cut falls inside a word
    let next_is_space = s[end..].starts_with(' ');
    if !next_is_space {
        if let Some(sp) = s[..end].rfind(' ') {
            if sp > 0 {
                end = sp;
            }
        }
    }
    let mut out = s[..end].to_string();
    while out.ends_with(' ') {
        out.pop();
    }
    (out, true)
}

// ---------------------------------------------------------------------------------------------
// The real clipboard (Windows)

use windows::core::w;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HWND};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, GetClipboardSequenceNumber, OpenClipboard, RegisterClipboardFormatW, SetClipboardData};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

/// Standard clipboard format of UTF-16 text.
pub const CF_UNICODETEXT: u32 = 13;

/// The Windows clipboard, owned by the Helper's hidden main window.
pub struct WinClip {
    owner: HWND,
}

// SAFETY: the window handle is only an identifier passed to OpenClipboard; WinClip is used on the
// UI thread that owns the window.
unsafe impl Send for WinClip {}

impl WinClip {
    pub fn new(owner: isize) -> WinClip {
        WinClip { owner: HWND(owner as *mut core::ffi::c_void) }
    }
}

/// Moveable global memory holding `bytes` (the system owns it once SetClipboardData succeeds).
fn set_bytes(format: u32, bytes: &[u8]) -> bool {
    unsafe {
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
    }
}

/// Sets every exclusion format through `set` and returns true when all of them were set. It never
/// stops at the first failure: a history format that could not be set must not leave the cloud
/// and monitor formats unset too. A format id of 0 (registration failed) counts as a failure.
pub fn set_every_exclusion(formats: &[u32; 3], mut set: impl FnMut(u32) -> bool) -> bool {
    let mut all = true;
    for &f in formats {
        let ok = f != 0 && set(f);
        all &= ok;
    }
    all
}

/// The three formats that keep a write out of the Win+V history, the cloud clipboard and the
/// clipboard managers that honour them (0 when a registration failed).
pub fn exclusion_formats() -> [u32; 3] {
    unsafe {
        [
            RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory")),
            RegisterClipboardFormatW(w!("CanUploadToCloudClipboard")),
            RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing")),
        ]
    }
}

impl ClipOs for WinClip {
    fn seq(&self) -> u32 {
        unsafe { GetClipboardSequenceNumber() }
    }

    fn open(&mut self) -> bool {
        unsafe { OpenClipboard(Some(self.owner)) }.is_ok()
    }

    fn empty(&mut self) -> bool {
        unsafe { EmptyClipboard() }.is_ok()
    }

    fn set_unicode(&mut self, text: &str) -> Option<bool> {
        let bytes: Vec<u8> = text.encode_utf16().chain(std::iter::once(0)).flat_map(|u| u.to_le_bytes()).collect();
        if !set_bytes(CF_UNICODETEXT, &bytes) {
            return None;
        }
        // DWORD 0 for each: "do not include", "do not upload"; the third one's mere presence
        // excludes it from monitoring
        let zero = 0u32.to_le_bytes();
        let excluded = set_every_exclusion(&exclusion_formats(), |f| set_bytes(f, &zero));
        Some(excluded)
    }

    fn close(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The fake clipboard of the tests

/// An in-memory clipboard with Windows' sequence-number rules, for the tests (D9: the tests never
/// touch the real clipboard). The text is kept only so that the tests can check it.
#[derive(Clone, Debug, Default)]
pub struct FakeClip {
    pub seq: u32,
    pub text: Option<String>,
    pub excluded: bool,
    pub is_open: bool,
    /// Fail this many `open` calls (another program holds the clipboard).
    pub busy_opens: u32,
    pub opens: u32,
    pub waits: u32,
    pub empties: u32,
    pub writes: u32,
    /// `set_unicode` sets the exclusion formats.
    pub exclusion_ok: bool,
}

impl FakeClip {
    pub fn new() -> FakeClip {
        FakeClip { seq: 100, exclusion_ok: true, ..FakeClip::default() }
    }

    /// Another program puts something on the clipboard (the player copies).
    pub fn user_copy(&mut self, text: &str) {
        self.seq += 1;
        self.text = Some(text.to_string());
        self.excluded = false;
    }
}

impl ClipOs for FakeClip {
    fn seq(&self) -> u32 {
        self.seq
    }

    fn open(&mut self) -> bool {
        self.opens += 1;
        if self.busy_opens > 0 {
            self.busy_opens -= 1;
            return false;
        }
        assert!(!self.is_open, "opened twice");
        self.is_open = true;
        true
    }

    fn empty(&mut self) -> bool {
        assert!(self.is_open, "EmptyClipboard on a closed clipboard");
        self.empties += 1;
        self.seq += 1;
        self.text = None;
        self.excluded = false;
        true
    }

    fn set_unicode(&mut self, text: &str) -> Option<bool> {
        assert!(self.is_open, "SetClipboardData on a closed clipboard");
        self.writes += 1;
        self.seq += 1;
        self.text = Some(text.to_string());
        self.excluded = self.exclusion_ok;
        Some(self.exclusion_ok)
    }

    fn close(&mut self) {
        assert!(self.is_open, "CloseClipboard without OpenClipboard");
        self.is_open = false;
    }

    fn wait(&mut self, _ms: u64) {
        self.waits += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn begin_empties_the_clipboard_at_once() {
        let mut os = FakeClip::new();
        os.user_copy("a password");
        let mut r = ReplyClip::new(Permit::Allowed);
        let b = r.begin(&mut os);
        assert_eq!(b, Begin::Emptied(os.seq));
        assert_eq!(os.text, None, "an early Ctrl+V pastes nothing");
        assert!(!os.is_open, "closed again");
        // finish writes the final text, kept out of the history
        assert_eq!(r.finish(&mut os, "Brechen wir in 5 Minuten auf?"), ClipOutcome::Copied { cut: false });
        assert_eq!(os.text.as_deref(), Some("Brechen wir in 5 Minuten auf?"));
        assert!(os.excluded && r.history_excluded);
        assert!(!os.is_open);
        assert_eq!(r.outcome(), Some(ClipOutcome::Copied { cut: false }));
    }

    #[test]
    fn something_copied_meanwhile_is_left_alone() {
        let mut os = FakeClip::new();
        let mut r = ReplyClip::new(Permit::Allowed);
        r.begin(&mut os);
        os.user_copy("the player's own copy");
        let writes = os.writes;
        assert_eq!(r.finish(&mut os, "translation"), ClipOutcome::Changed);
        assert_eq!(os.text.as_deref(), Some("the player's own copy"));
        assert_eq!(os.writes, writes, "nothing written");
        // and a release never empties what is not ours
        assert!(!r.release(&mut os));
        assert_eq!(os.text.as_deref(), Some("the player's own copy"));
    }

    #[test]
    fn busy_after_three_tries() {
        let mut os = FakeClip::new();
        let mut r = ReplyClip::new(Permit::Allowed);
        r.begin(&mut os);
        os.busy_opens = 3;
        let opens = os.opens;
        assert_eq!(r.finish(&mut os, "translation"), ClipOutcome::Busy);
        assert_eq!(os.opens - opens, 3, "3 tries");
        assert_eq!(os.waits, 2, "30 ms apart (2 waits between 3 tries)");
        assert_eq!(os.text, None);
        // two failed opens then success: written
        let mut os = FakeClip::new();
        let mut r = ReplyClip::new(Permit::Allowed);
        r.begin(&mut os);
        os.busy_opens = 2;
        assert_eq!(r.finish(&mut os, "translation"), ClipOutcome::Copied { cut: false });
    }

    #[test]
    fn a_busy_begin_still_copies_when_nothing_changed() {
        let mut os = FakeClip::new();
        os.user_copy("old");
        os.busy_opens = 3;
        let mut r = ReplyClip::new(Permit::Allowed);
        assert_eq!(r.begin(&mut os), Begin::Busy);
        assert_eq!(os.text.as_deref(), Some("old"), "left as it was");
        assert_eq!(r.finish(&mut os, "new"), ClipOutcome::Copied { cut: false });
        assert_eq!(os.text.as_deref(), Some("new"));
        // ...but not when the player copied after the failed empty
        let mut os = FakeClip::new();
        os.busy_opens = 3;
        let mut r = ReplyClip::new(Permit::Allowed);
        r.begin(&mut os);
        os.user_copy("mine");
        assert_eq!(r.finish(&mut os, "new"), ClipOutcome::Changed);
    }

    #[test]
    fn no_consent_and_switch_off_touch_nothing() {
        for (permit, want) in [(Permit::NoConsent, ClipOutcome::NotAllowed), (Permit::Off, ClipOutcome::Off)] {
            let mut os = FakeClip::new();
            os.user_copy("kept");
            let mut r = ReplyClip::new(permit);
            assert_eq!(r.begin(&mut os), Begin::NotAllowed);
            assert_eq!(r.finish(&mut os, "translation"), want);
            assert!(!r.release(&mut os));
            assert_eq!((os.opens, os.text.as_deref()), (0, Some("kept")), "{permit:?}");
        }
        assert_eq!(Permit::new(false, true), Permit::NoConsent);
        assert_eq!(Permit::new(false, false), Permit::NoConsent);
        assert_eq!(Permit::new(true, false), Permit::Off);
        assert_eq!(Permit::new(true, true), Permit::Allowed);
    }

    #[test]
    fn release_only_when_own() {
        let mut os = FakeClip::new();
        let mut r = ReplyClip::new(Permit::Allowed);
        r.begin(&mut os);
        let empties = os.empties;
        assert!(!r.release(&mut os), "nothing written yet: nothing to release");
        assert_eq!(os.empties, empties);
        r.finish(&mut os, "translation");
        assert!(r.release(&mut os), "still ours: emptied");
        assert_eq!(os.text, None);
        assert!(!r.release(&mut os), "once");
        // pasted and then copied something else: not ours any more
        let mut os = FakeClip::new();
        let mut r = ReplyClip::new(Permit::Allowed);
        r.begin(&mut os);
        r.finish(&mut os, "translation");
        os.user_copy("later copy");
        assert!(!r.release(&mut os));
        assert_eq!(os.text.as_deref(), Some("later copy"));
    }

    #[test]
    fn finish_runs_once() {
        let mut os = FakeClip::new();
        let mut r = ReplyClip::new(Permit::Allowed);
        r.begin(&mut os);
        assert_eq!(r.finish(&mut os, "one"), ClipOutcome::Copied { cut: false });
        let writes = os.writes;
        assert_eq!(r.finish(&mut os, "two"), ClipOutcome::Copied { cut: false });
        assert_eq!((os.writes, os.text.as_deref()), (writes, Some("one")));
    }

    #[test]
    fn every_exclusion_format_is_tried_even_after_a_failure() {
        // review finding: `Iterator::all` stopped at the first failed format
        let mut tried = Vec::new();
        let all = set_every_exclusion(&[11, 12, 13], |f| {
            tried.push(f);
            f != 11
        });
        assert!(!all, "one failure is reported");
        assert_eq!(tried, vec![11, 12, 13], "the cloud and monitor formats are still set");
        let mut tried = Vec::new();
        assert!(!set_every_exclusion(&[0, 12, 13], |f| {
            tried.push(f);
            true
        }));
        assert_eq!(tried, vec![12, 13], "a format that failed to register is skipped, the others set");
        assert!(set_every_exclusion(&[11, 12, 13], |_| true));
    }

    #[test]
    fn own_write_follows_the_write_and_the_release() {
        let mut os = FakeClip::new();
        let mut r = ReplyClip::new(Permit::Allowed);
        r.begin(&mut os);
        assert_eq!(r.own_write(), None);
        assert!(r.finish(&mut os, "Hallo").copied());
        assert_eq!(r.own_write(), Some(os.seq));
        assert!(r.release(&mut os));
        assert_eq!(r.own_write(), None);
    }

    #[test]
    fn missing_exclusion_formats_are_reported() {
        let mut os = FakeClip::new();
        os.exclusion_ok = false;
        let mut r = ReplyClip::new(Permit::Allowed);
        r.begin(&mut os);
        assert_eq!(r.finish(&mut os, "x"), ClipOutcome::Copied { cut: false });
        assert!(!r.history_excluded);
    }

    #[test]
    fn one_line_255_rules() {
        assert_eq!(one_line_255("  Salut,\r\nça va ?\tOui  "), ("Salut, ça va ? Oui".to_string(), false));
        assert_eq!(one_line_255("\u{2}[Linen Cloth]\u{3} x20"), ("[Linen Cloth] x20".to_string(), false));
        assert_eq!(one_line_255("\n\n"), (String::new(), false));
        // exactly 255 bytes fits
        let a = "a".repeat(255);
        assert_eq!(one_line_255(&a), (a.clone(), false));
        // a word boundary
        let words = "word ".repeat(60); // 300 bytes
        let (s, cut) = one_line_255(&words);
        assert!(cut && s.len() <= 255 && s.ends_with("word") && !s.ends_with(' '), "{s}");
        assert_eq!(s.len(), 254, "51 whole words and 50 spaces");
        // one long word: cut at a character boundary
        let (s, cut) = one_line_255(&"x".repeat(400));
        assert!(cut && s.len() == 255);
        // UTF-8: never split a character (é = 2 bytes)
        let (s, cut) = one_line_255(&"é".repeat(200));
        assert!(cut && s.len() == 254 && s.chars().all(|c| c == 'é'));
        let (s, _) = one_line_255(&"Привет ".repeat(30));
        assert!(s.len() <= 255 && s.ends_with("Привет"));
        // pipes count 2 (the addon escapes them)
        let p = "|".repeat(128);
        let (s, cut) = one_line_255(&p);
        assert!(cut && s.len() == 127, "127 pipes = 254 wire bytes");
        let (s, cut) = one_line_255(&format!("{} a", "|".repeat(126)));
        assert!(!cut && s.len() == 128);
    }
}
