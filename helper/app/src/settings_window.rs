//! The "Translation engine..." window (tray menu, 2026-10-20, user decision: players may use their
//! own API keys for a remote translation service). Built in code (Win32 controls, no resource
//! file), localized en/fr, opened only by the player's own click in the tray menu, so it may take
//! the focus. The keys are entered here only, never in WoW.
//!
//! Layout, top to bottom: the service list (the local translator, DeepL, OpenAI-compatible, Google
//! Gemini, Anthropic Claude), the API key (masked; a saved key is never shown: the field stays
//! empty and an empty field keeps it; "Remove saved key" deletes it), the model (an editable list
//! with the engine's suggestions), the base URL (OpenAI-compatible only), the daily character
//! limit (0 = none, with today's count), the privacy notice, the required consent box ("Send the
//! text of chat messages and glossary terms to this service"), Test (one synthetic sentence, the
//! translation or the error), Save and Cancel.
//!
//! - [`Form`] is the window's state machine, pure and tested: what each control holds and whether
//!   it is enabled, what Save writes, what Test sends. The consent belongs to one service (the
//!   engine's `service_id`: the provider, and the host for an OpenAI-compatible server), so
//!   another service or another host unticks the box.
//! - [`Backend`] is what the window needs from outside: the key file (`keys.bin`, DPAPI, the
//!   engine's key store), the Test request (`tp_engine::remote::test_translation` on its own
//!   thread) and today's count. Tests use a fake.
//! - The window posts [`WM_APP_SETTINGS`] to the main window, which takes the
//!   [`SettingsEvent`]s ([`take_events`]): Save writes settings.json there (the UI thread's copy
//!   of the settings, so a tray switch changed meanwhile is kept) and hands the new service to the
//!   engine (`EngineHandle::set_remote`).
//! - Keyboard: the UI thread's message loop passes the window's messages through
//!   `IsDialogMessageW` ([`dialog_message`]): Tab, Enter (Save) and Esc (Cancel) work as in a
//!   dialog. Controls get the common controls 6 look through an activation context made from the
//!   manifest Windows ships in shell32.dll (resource 124); without it they keep the classic look.
//! - Privacy: the key goes from the edit control into a `Secret` (zeroed on drop) only for Save and
//!   Test; nothing typed here is logged (the log gets the action, the provider id and ok).

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use tp_engine::remote::keystore::{self, Keys};
use tp_engine::remote::{self, Fallback, Provider, RemoteConfig, RemoteSettings, Secret, Transport, Url, OPENAI_DEFAULT_BASE};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::ApplicationInstallationAndServicing::{ActivateActCtx, CreateActCtxW, DeactivateActCtx, ACTCTXW};
use windows::Win32::UI::Controls::{CB_SETCUEBANNER, EM_LIMITTEXT, EM_SETCUEBANNER};
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForMonitor, GetDpiForWindow, SystemParametersInfoForDpi, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::i18n::{self, Lang};
use crate::overlay::window::hinstance;
use crate::remote::{config_error_key, provider_name, reason, Choice};

pub const SETTINGS_CLASS: PCWSTR = w!("TPHelperSettings");
/// Posted to the main window when [`take_events`] has something.
pub const WM_APP_SETTINGS: u32 = WM_APP + 31;
/// Posted to the settings window by the Test thread.
const WM_APP_TEST_DONE: u32 = WM_APP + 32;

// Control ids (Save and Cancel are IDOK and IDCANCEL, for Enter and Esc).
pub const ID_PROVIDER: i32 = 101;
pub const ID_KEY: i32 = 111;
pub const ID_REMOVE_KEY: i32 = 112;
pub const ID_KEY_HINT: i32 = 113;
pub const ID_MODEL: i32 = 121;
pub const ID_BASE_URL: i32 = 131;
pub const ID_CAP: i32 = 141;
pub const ID_CAP_UNIT: i32 = 142;
pub const ID_PRIVACY: i32 = 150;
pub const ID_CONSENT: i32 = 151;
pub const ID_TEST: i32 = 160;
pub const ID_NOTE: i32 = 161;
pub const ID_SAVE: i32 = 1; // IDOK
pub const ID_CANCEL: i32 = 2; // IDCANCEL
const ID_LABELS: [i32; 5] = [100, 110, 120, 130, 140];
/// The largest daily limit accepted (characters).
pub const MAX_DAILY_CHARS: u32 = 1_000_000_000;
/// `SS_LEFT | SS_NOPREFIX` (declared here instead of enabling `Win32_System_SystemServices`).
const SS_LEFT_NOPREFIX: u32 = 0x0080;

// ---------------------------------------------------------------------------------------------
// The state machine

/// The colour of the message line under Test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteTone {
    Info,
    Ok,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub text: String,
    pub tone: NoteTone,
}

/// Which controls take input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Enabled {
    pub key: bool,
    pub remove_key: bool,
    pub model: bool,
    pub base_url: bool,
    pub cap: bool,
    pub consent: bool,
    pub test: bool,
}

/// What the window shows and what Save and Test do (no Win32).
#[derive(Clone, Debug)]
pub struct Form {
    pub choice: Choice,
    /// The model field (empty: the service's default).
    pub model: String,
    /// The base URL field (shown for the OpenAI-compatible service only, kept across choices).
    pub base_url: String,
    /// The daily limit field (characters, 0 or empty: no limit).
    pub cap: String,
    /// The service id the consent box was ticked for (the saved consent at first).
    consent_for: String,
    /// The key field holds something (its text stays in the control until Save or Test).
    pub key_typed: bool,
    /// The service ids the key file holds a key for (a key belongs to one service: the provider,
    /// or one OpenAI-compatible server and route).
    has_key: BTreeSet<String>,
    /// The key file could not be read (or this run never touches it: tests, the echo
    /// translator): keys can be neither typed, saved nor removed, so nothing is overwritten.
    pub keys_locked: bool,
    saved: RemoteSettings,
    /// The Test request running (its number), 0 when none.
    pub testing: u64,
    pub note: Option<Note>,
}

impl Form {
    /// The form for the saved choice; `slots`: the service ids the key file holds a key for.
    pub fn open<'a>(saved: &RemoteSettings, slots: impl IntoIterator<Item = &'a str>) -> Form {
        let mut f = Form {
            choice: Choice::Local,
            model: String::new(),
            base_url: String::new(),
            cap: saved.daily_char_cap.to_string(),
            consent_for: saved.consent.clone(),
            key_typed: false,
            has_key: slots.into_iter().map(str::to_string).collect(),
            keys_locked: false,
            saved: saved.clone(),
            testing: 0,
            note: None,
        };
        f.base_url = if saved.base_url.trim().is_empty() { OPENAI_DEFAULT_BASE.to_string() } else { saved.base_url.trim().to_string() };
        f.select(Choice::of(saved));
        f
    }

    /// Another service in the list: its saved model (the default model for another service; for
    /// an OpenAI-compatible server other than OpenAI's, none: that server's model must be typed);
    /// the key field is emptied (a typed key belongs to the service it was typed for).
    pub fn select(&mut self, c: Choice) {
        self.choice = c;
        let same = Choice::of(&self.saved) == c;
        self.model = match c.provider() {
            None => String::new(),
            Some(_) if same => self.saved.model.trim().to_string(),
            Some(Provider::OpenAi) if !self.openai_default_host() => String::new(),
            Some(p) => p.default_model().to_string(),
        };
        self.key_typed = false;
        self.note = None;
    }

    /// The base URL field names OpenAI's own server.
    fn openai_default_host(&self) -> bool {
        let base = if self.base_url.trim().is_empty() { OPENAI_DEFAULT_BASE } else { self.base_url.trim() };
        Url::parse(base).is_some_and(|u| u.host == "api.openai.com")
    }

    /// The player edited the base URL. A model still at OpenAI's default is cleared once the URL
    /// names another server (that server's model must be typed: Save asks for it).
    pub fn set_base_url(&mut self, s: String) {
        self.base_url = s;
        if self.choice == Choice::Remote(Provider::OpenAi) && !self.openai_default_host() && self.model.trim() == Provider::OpenAi.default_model() {
            self.model.clear();
        }
    }

    /// The base URL as saved: empty for the default.
    fn base_for_settings(&self) -> String {
        let t = self.base_url.trim();
        if t.trim_end_matches('/') == OPENAI_DEFAULT_BASE {
            String::new()
        } else {
            t.to_string()
        }
    }

    /// The fields as settings (the consent as it was ticked, the cap as saved when the field is
    /// not a number).
    fn current(&self) -> RemoteSettings {
        RemoteSettings {
            provider: self.choice.id().to_string(),
            model: if self.choice == Choice::Local { String::new() } else { self.model.trim().to_string() },
            base_url: self.base_for_settings(),
            consent: self.consent_for.clone(),
            daily_char_cap: self.cap_value().unwrap_or(self.saved.daily_char_cap),
        }
    }

    /// What the consent box is about: the provider, and the host for an OpenAI-compatible
    /// server; empty for the local translator or a base URL that is not one.
    pub fn service_id(&self) -> String {
        self.current().service_id()
    }

    pub fn consent_checked(&self) -> bool {
        let id = self.service_id();
        !id.is_empty() && self.consent_for == id
    }

    /// The player ticked (or unticked) the box: for the service shown now.
    pub fn set_consent(&mut self, on: bool) {
        self.consent_for = if on { self.service_id() } else { String::new() };
    }

    fn cap_value(&self) -> Option<u32> {
        let t = self.cap.trim();
        if t.is_empty() {
            return Some(0);
        }
        t.parse::<u32>().ok().filter(|v| *v <= MAX_DAILY_CHARS)
    }

    /// A service that needs a key: every one but an OpenAI-compatible server on this computer or
    /// on a private network address (`tp_engine::remote::key_needed`).
    pub fn key_needed(&self) -> bool {
        match self.choice.provider() {
            None => false,
            Some(p) => remote::key_needed(p, &self.base_for_settings()),
        }
    }

    /// The key file holds a key for the service shown (for an OpenAI-compatible server: this
    /// very server and route; another URL has no saved key until one is typed for it).
    pub fn has_stored_key(&self) -> bool {
        let id = self.service_id();
        !id.is_empty() && self.has_key.contains(&id)
    }

    /// The key file changed (Save, Remove) for a service id.
    pub fn set_stored(&mut self, slot: &str, on: bool) {
        if on {
            self.has_key.insert(slot.to_string());
        } else {
            self.has_key.remove(slot);
        }
    }

    pub fn enabled(&self) -> Enabled {
        let remote = self.choice.provider().is_some();
        Enabled {
            key: remote && !self.keys_locked,
            remove_key: remote && !self.keys_locked && self.has_stored_key(),
            model: remote,
            base_url: self.choice == Choice::Remote(Provider::OpenAi),
            cap: remote,
            consent: remote && !self.service_id().is_empty(),
            test: remote && self.testing == 0 && (self.key_typed || self.has_stored_key() || !self.key_needed()),
        }
    }

    /// The line under the key field (a locale key), None for the local translator.
    pub fn key_hint(&self) -> Option<&'static str> {
        match self.choice.provider() {
            None => None,
            Some(_) if self.keys_locked => Some("settings.keys_unavailable"),
            Some(_) if !self.key_needed() => Some("settings.key_not_needed"),
            Some(_) if self.has_stored_key() => Some("settings.key_saved"),
            Some(_) => Some("settings.key_none"),
        }
    }

    /// The privacy notice for the service shown.
    pub fn privacy(&self, lang: Lang) -> String {
        match self.choice.provider() {
            None => i18n::tr(lang, "settings.privacy_local").to_string(),
            Some(p) => i18n::tr_fmt(lang, "settings.privacy_remote", &[("provider", provider_name(lang, p))]),
        }
    }

    /// Save: the settings to write, or the message (a locale key) of what is wrong. `key` is the
    /// key field's text when it holds one, else the saved key (checked, never stored here).
    pub fn check_save(&self, key: Secret) -> Result<RemoteSettings, &'static str> {
        let cap = self.cap_value().ok_or("settings.err_cap")?;
        let mut s = self.current();
        s.daily_char_cap = cap;
        let Some(p) = self.choice.provider() else { return Ok(s) };
        if s.service_id().is_empty() {
            return Err("settings.err_url");
        }
        if !self.consent_checked() {
            return Err("settings.err_consent");
        }
        RemoteConfig::new(p, &s.model, &s.base_url, key, cap).map_err(config_error_key)?;
        s.give_consent();
        Ok(s)
    }

    /// Test: the service as the fields describe it (no consent needed: only the synthetic
    /// sentence is sent).
    pub fn test_config(&self, key: Secret) -> Result<RemoteConfig, &'static str> {
        let Some(p) = self.choice.provider() else { return Err("settings.err_key") };
        RemoteConfig::new(p, self.model.trim(), &self.base_for_settings(), key, 0).map_err(config_error_key)
    }
}

// ---------------------------------------------------------------------------------------------
// What the window needs from outside

/// A finished Test: the translation, or the reason; and how long it took.
pub type TestResult = (Result<String, Fallback>, u64);

pub trait Backend {
    /// The keys as they are in the key file (empty when it was not read).
    fn keys(&self) -> Keys;
    /// False when the key file was not read (an unreadable file, or a run that never touches
    /// it): nothing may then be written over it.
    fn writable(&self) -> bool;
    /// The saved key of a service id (`tp_engine::remote::service_id`).
    fn stored_key(&self, slot: &str) -> Option<Secret> {
        self.keys().get(slot).cloned()
    }
    /// Saves (`Some`) or removes (`None`) a service's key in the key file; the keys as they are
    /// on disk now, or Err when the file could not (or may not) be written (the window shows
    /// the same message for every cause, and nothing else is logged).
    #[allow(clippy::result_unit_err)]
    fn store_key(&mut self, slot: &str, key: Option<Secret>) -> Result<Keys, ()>;
    /// Runs one Test request off the UI thread; `done` is called on that thread.
    fn run_test(&mut self, cfg: RemoteConfig, target: String, done: Box<dyn FnOnce(TestResult) + Send>);
    /// Characters sent to the remote services today (the daily cap's count).
    fn used_today(&self) -> u64;
}

/// The real backend: the key file next to settings.json, WinHTTP for Test, the engine's usage
/// file.
pub struct KeyFileBackend {
    path: PathBuf,
    /// The keys read from the file; None when it was not read (nothing is ever written then).
    keys: Option<Keys>,
    data_dir: PathBuf,
    transport: Option<Arc<dyn Transport>>,
}

impl KeyFileBackend {
    /// `keys_path`: `keys.bin`; `keys`: what it holds now, as read (None when it could not be
    /// read, or must not be: a test run or the echo translator); `data_dir`: the engine's data
    /// folder.
    pub fn new(keys_path: PathBuf, keys: Option<Keys>, data_dir: PathBuf) -> KeyFileBackend {
        KeyFileBackend { path: keys_path, keys, data_dir, transport: None }
    }
}

impl Backend for KeyFileBackend {
    fn keys(&self) -> Keys {
        self.keys.clone().unwrap_or_default()
    }

    fn writable(&self) -> bool {
        self.keys.is_some()
    }

    fn store_key(&mut self, slot: &str, key: Option<Secret>) -> Result<Keys, ()> {
        // a key set that was never read would erase the other services' keys
        let Some(mut k) = self.keys.clone() else { return Err(()) };
        match key {
            Some(s) => k.set(slot, s),
            None => k.remove(slot),
        }
        keystore::save(&self.path, &k).map_err(|_| ())?;
        self.keys = Some(k.clone());
        Ok(k)
    }

    fn run_test(&mut self, cfg: RemoteConfig, target: String, done: Box<dyn FnOnce(TestResult) + Send>) {
        let t = self.transport.get_or_insert_with(remote::system_transport).clone();
        let spawned = std::thread::Builder::new().name("tp-test".into()).spawn(move || {
            let t0 = Instant::now();
            let r = remote::test_translation(&cfg, &target, t.as_ref());
            done((r, t0.elapsed().as_millis() as u64));
        });
        if spawned.is_err() {
            // no thread: nothing will answer; the window stays usable (the button comes back on
            // the next open)
        }
    }

    fn used_today(&self) -> u64 {
        remote::usage::read_today(&self.data_dir)
    }
}

/// What the window tells the UI thread.
pub enum SettingsEvent {
    /// Saved: the new choice (consent included) and the keys as they are on disk now.
    Saved { translation: RemoteSettings, keys: Keys },
    /// "Remove saved key": the keys as they are on disk now.
    KeysChanged(Keys),
    /// For the log: the action ("open", "save", "cancel", "test", "remove_key"), the provider id,
    /// and whether it worked. Never any text the player typed.
    Log { action: &'static str, provider: &'static str, ok: bool },
    Closed,
}

thread_local! {
    static EVENTS: RefCell<Vec<SettingsEvent>> = const { RefCell::new(Vec::new()) };
    static WINDOW: RefCell<Option<SettingsWindow>> = const { RefCell::new(None) };
    /// The open window (0: none), for `IsDialogMessageW` and re-entrant messages.
    static OPEN: Cell<isize> = const { Cell::new(0) };
    /// The message line and its colour, read by WM_CTLCOLORSTATIC without borrowing the window.
    static NOTE: Cell<(isize, u8)> = const { Cell::new((0, 0)) };
}

/// The events since the last call (the main window gets [`WM_APP_SETTINGS`] for them).
pub fn take_events() -> Vec<SettingsEvent> {
    EVENTS.with(|e| std::mem::take(&mut *e.borrow_mut()))
}

fn push_event(notify: isize, ev: SettingsEvent) {
    EVENTS.with(|e| e.borrow_mut().push(ev));
    if notify != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(notify as *mut core::ffi::c_void)), WM_APP_SETTINGS, WPARAM(0), LPARAM(0));
        }
    }
}

/// The open settings window, if any.
pub fn current() -> Option<isize> {
    let h = OPEN.with(Cell::get);
    (h != 0).then_some(h)
}

/// Keyboard navigation of the open window (Tab, Enter, Esc): true when `msg` was handled.
pub fn dialog_message(msg: &MSG) -> bool {
    let h = OPEN.with(Cell::get);
    h != 0 && unsafe { IsDialogMessageW(HWND(h as *mut core::ffi::c_void), msg) }.as_bool()
}

/// Closes the window without saving (the Helper exits).
pub fn close() {
    if let Some(h) = current() {
        unsafe {
            let _ = DestroyWindow(HWND(h as *mut core::ffi::c_void));
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Layout (physical px), pure

/// Every control's rectangle [x, y, w, h] in client px and the client size.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub rects: Vec<(i32, [i32; 4])>,
    pub w: i32,
    pub h: i32,
}

impl Layout {
    pub fn rect(&self, id: i32) -> [i32; 4] {
        self.rects.iter().find(|(i, _)| *i == id).map(|(_, r)| *r).unwrap_or([0; 4])
    }
}

/// Combo boxes are given their field height plus their list's.
const LIST_DIP: i32 = 180;

/// The layout at `dpi`. `text_h(text_id, width)` is the height in px of a wrapping text (the
/// privacy notice, the consent box's label) at a width; `line_h` one line of the font.
pub fn layout(dpi: u32, line_h: i32, text_h: &mut dyn FnMut(i32, i32) -> i32) -> Layout {
    let s = |v: i32| (v * dpi as i32 + 48) / 96;
    let (w, m, lbl, gap) = (s(560), s(16), s(118), s(8));
    let row = (line_h + s(10)).max(s(24));
    let step = row + s(10);
    let fx = m + lbl + gap;
    let fw = w - fx - m;
    let mut r: Vec<(i32, [i32; 4])> = Vec::new();
    let label_dy = (row - line_h) / 2;
    let mut y = m;
    // service
    r.push((ID_LABELS[0], [m, y + label_dy, lbl, line_h]));
    r.push((ID_PROVIDER, [fx, y, fw, row + s(LIST_DIP)]));
    y += step;
    // key, its button, its hint (two lines)
    let bw = s(150);
    r.push((ID_LABELS[1], [m, y + label_dy, lbl, line_h]));
    r.push((ID_KEY, [fx, y, fw - bw - gap, row]));
    r.push((ID_REMOVE_KEY, [fx + fw - bw, y, bw, row]));
    y += row + s(4);
    let hint_h = 2 * line_h;
    r.push((ID_KEY_HINT, [fx, y, fw, hint_h]));
    y += hint_h + s(8);
    // model, base URL
    r.push((ID_LABELS[2], [m, y + label_dy, lbl, line_h]));
    r.push((ID_MODEL, [fx, y, fw, row + s(LIST_DIP)]));
    y += step;
    r.push((ID_LABELS[3], [m, y + label_dy, lbl, line_h]));
    r.push((ID_BASE_URL, [fx, y, fw, row]));
    y += step;
    // daily limit and its unit
    let cw = s(100);
    r.push((ID_LABELS[4], [m, y + label_dy, lbl, line_h]));
    r.push((ID_CAP, [fx, y, cw, row]));
    r.push((ID_CAP_UNIT, [fx + cw + gap, y + label_dy, fw - cw - gap, line_h]));
    y += step + s(4);
    // privacy notice, consent box
    let pw = w - 2 * m;
    let ph = text_h(ID_PRIVACY, pw).max(line_h);
    r.push((ID_PRIVACY, [m, y, pw, ph]));
    y += ph + s(10);
    let box_text = s(20);
    let ch = text_h(ID_CONSENT, pw - box_text).max(line_h) + s(4);
    r.push((ID_CONSENT, [m, y, pw, ch]));
    y += ch + s(14);
    // Test and its message (three lines)
    let tw = s(110);
    let bh = row + s(4);
    let nh = 3 * line_h;
    r.push((ID_TEST, [m, y, tw, bh]));
    r.push((ID_NOTE, [m + tw + s(12), y + (bh - line_h) / 2, pw - tw - s(12), nh]));
    y += bh.max(nh + (bh - line_h) / 2) + s(14);
    // Save, Cancel
    let ow = s(110);
    r.push((ID_SAVE, [w - m - 2 * ow - gap, y, ow, bh]));
    r.push((ID_CANCEL, [w - m - ow, y, ow, bh]));
    y += bh + m;
    Layout { rects: r, w, h: y }
}

// ---------------------------------------------------------------------------------------------
// The window

/// What `open` needs.
pub struct OpenArgs {
    pub lang: Lang,
    pub saved: RemoteSettings,
    pub backend: Box<dyn Backend>,
    /// The main window (gets [`WM_APP_SETTINGS`]).
    pub notify: isize,
    /// The Test's target language.
    pub test_target: String,
    /// Show it and give it the focus (the player opened it). False: created hidden (tests).
    pub show: bool,
    /// The app icon for the title bar and the taskbar.
    pub icon: Option<HICON>,
}

struct SettingsWindow {
    hwnd: HWND,
    ctl: Vec<(i32, HWND)>,
    font: HFONT,
    dpi: u32,
    lang: Lang,
    form: Form,
    backend: Box<dyn Backend>,
    notify: isize,
    test_target: String,
    gen: u64,
    test_slot: Arc<Mutex<Option<(u64, TestResult)>>>,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The common controls 6 activation context (visual styles), made once from the manifest Windows
/// ships in shell32.dll (resource 124). None when it cannot be made: classic controls.
fn visual_styles() -> Option<HANDLE> {
    static CTX: OnceLock<isize> = OnceLock::new();
    let h = *CTX.get_or_init(|| {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let path = std::path::Path::new(&root).join("System32").join("shell32.dll");
        let src = wide(&path.to_string_lossy());
        const ACTCTX_FLAG_RESOURCE_NAME_VALID: u32 = 0x8;
        let ctx = ACTCTXW {
            cbSize: std::mem::size_of::<ACTCTXW>() as u32,
            dwFlags: ACTCTX_FLAG_RESOURCE_NAME_VALID,
            lpSource: PCWSTR(src.as_ptr()),
            lpResourceName: PCWSTR(124usize as *const u16),
            ..Default::default()
        };
        match unsafe { CreateActCtxW(&ctx) } {
            Ok(h) if !h.is_invalid() => h.0 as isize,
            _ => 0,
        }
    });
    (h != 0).then_some(HANDLE(h as *mut core::ffi::c_void))
}

/// True when the visual styles context could be made (for the log and the tests).
pub fn visual_styles_available() -> bool {
    visual_styles().is_some()
}

fn message_font(dpi: u32) -> HFONT {
    unsafe {
        let mut ncm = NONCLIENTMETRICSW { cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32, ..Default::default() };
        let ok = SystemParametersInfoForDpi(SPI_GETNONCLIENTMETRICS.0, ncm.cbSize, Some(&mut ncm as *mut _ as *mut core::ffi::c_void), 0, dpi).is_ok();
        if ok {
            CreateFontIndirectW(&ncm.lfMessageFont)
        } else {
            let mut lf = LOGFONTW { lfHeight: -((9 * dpi as i32 + 36) / 72), lfWeight: FW_NORMAL.0 as i32, ..Default::default() };
            let face = wide("Segoe UI");
            lf.lfFaceName[..face.len()].copy_from_slice(&face);
            CreateFontIndirectW(&lf)
        }
    }
}

/// One line's height and a wrapping text's height at a width, with `font`.
fn measure(font: HFONT, text: &str, width: i32) -> (i32, i32) {
    unsafe {
        let dc = CreateCompatibleDC(None);
        let old = SelectObject(dc, font.into());
        let mut tm = TEXTMETRICW::default();
        let _ = GetTextMetricsW(dc, &mut tm);
        let mut rc = RECT { left: 0, top: 0, right: width.max(1), bottom: 0 };
        let mut t: Vec<u16> = text.encode_utf16().collect();
        let h = if t.is_empty() { 0 } else { DrawTextW(dc, &mut t, &mut rc, DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX) };
        SelectObject(dc, old);
        let _ = DeleteDC(dc);
        (tm.tmHeight, h)
    }
}

fn create_control(parent: HWND, class: PCWSTR, text: &str, style: WINDOW_STYLE, ex: WINDOW_EX_STYLE, id: i32) -> HWND {
    let t = wide(text);
    unsafe {
        CreateWindowExW(ex, class, PCWSTR(t.as_ptr()), WS_CHILD | WS_VISIBLE | style, 0, 0, 10, 10, Some(parent), Some(HMENU(id as isize as *mut core::ffi::c_void)), Some(hinstance()), None)
            .unwrap_or_default()
    }
}

fn set_text(h: HWND, s: &str) {
    let t = wide(s);
    unsafe {
        let _ = SetWindowTextW(h, PCWSTR(t.as_ptr()));
    }
}

fn get_text(h: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(h).max(0) as usize;
        let mut buf = vec![0u16; n + 1];
        let got = GetWindowTextW(h, &mut buf).max(0) as usize;
        let s = String::from_utf16_lossy(&buf[..got.min(n)]);
        buf.fill(0);
        s
    }
}

/// The key field's text as a `Secret`; the temporary copies are zeroed.
fn read_key(h: HWND) -> Secret {
    let mut s = get_text(h);
    let secret = Secret::new(&s);
    // SAFETY: zeros are valid UTF-8.
    unsafe { s.as_mut_vec() }.fill(0);
    secret
}

fn send(h: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    unsafe { SendMessageW(h, msg, Some(WPARAM(wp)), Some(LPARAM(lp))) }.0
}

impl SettingsWindow {
    fn ctl(&self, id: i32) -> HWND {
        self.ctl.iter().find(|(i, _)| *i == id).map(|(_, h)| *h).unwrap_or_default()
    }

    fn tr(&self, key: &str) -> &'static str {
        i18n::tr(self.lang, key)
    }

    fn create_controls(&mut self) {
        let h = self.hwnd;
        let static_ = w!("STATIC");
        let (edit, button, combo) = (w!("EDIT"), w!("BUTTON"), w!("COMBOBOX"));
        let tab = WS_TABSTOP;
        let none = WINDOW_EX_STYLE(0);
        let mut add = |id: i32, class: PCWSTR, text: &str, style: WINDOW_STYLE, ex: WINDOW_EX_STYLE| {
            let c = create_control(h, class, text, style, ex, id);
            self.ctl.push((id, c));
        };
        let lang = self.lang;
        let tr = |k: &str| i18n::tr(lang, k);
        let label = WINDOW_STYLE(SS_LEFT_NOPREFIX);
        // creation order = tab order
        add(ID_LABELS[0], static_, tr("settings.provider"), label, none);
        add(ID_PROVIDER, combo, "", tab | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWNLIST as u32), none);
        add(ID_LABELS[1], static_, tr("settings.key"), label, none);
        add(ID_KEY, edit, "", tab | WINDOW_STYLE((ES_PASSWORD | ES_AUTOHSCROLL) as u32), WS_EX_CLIENTEDGE);
        add(ID_REMOVE_KEY, button, tr("settings.remove_key"), tab | WINDOW_STYLE(BS_PUSHBUTTON as u32), none);
        add(ID_KEY_HINT, static_, "", label, none);
        add(ID_LABELS[2], static_, tr("settings.model"), label, none);
        add(ID_MODEL, combo, "", tab | WS_VSCROLL | WINDOW_STYLE((CBS_DROPDOWN | CBS_AUTOHSCROLL) as u32), none);
        add(ID_LABELS[3], static_, tr("settings.base_url"), label, none);
        add(ID_BASE_URL, edit, "", tab | WINDOW_STYLE(ES_AUTOHSCROLL as u32), WS_EX_CLIENTEDGE);
        add(ID_LABELS[4], static_, tr("settings.cap"), label, none);
        add(ID_CAP, edit, "", tab | WINDOW_STYLE((ES_NUMBER | ES_AUTOHSCROLL) as u32), WS_EX_CLIENTEDGE);
        add(ID_CAP_UNIT, static_, "", label, none);
        add(ID_PRIVACY, static_, "", label, none);
        add(ID_CONSENT, button, tr("settings.consent"), tab | WINDOW_STYLE((BS_AUTOCHECKBOX | BS_MULTILINE) as u32), none);
        add(ID_TEST, button, tr("settings.test"), tab | WINDOW_STYLE(BS_PUSHBUTTON as u32), none);
        add(ID_NOTE, static_, "", label, none);
        add(ID_SAVE, button, tr("settings.save"), tab | WINDOW_STYLE(BS_DEFPUSHBUTTON as u32), none);
        add(ID_CANCEL, button, tr("settings.cancel"), tab | WINDOW_STYLE(BS_PUSHBUTTON as u32), none);
        NOTE.with(|n| n.set((self.ctl(ID_NOTE).0 as isize, 0)));
        // input limits and hints
        send(self.ctl(ID_KEY), EM_LIMITTEXT, 512, 0);
        send(self.ctl(ID_BASE_URL), EM_LIMITTEXT, 512, 0);
        send(self.ctl(ID_CAP), EM_LIMITTEXT, 10, 0);
        send(self.ctl(ID_MODEL), CB_LIMITTEXT, 128, 0);
        let cue = wide(tr("settings.key_cue"));
        send(self.ctl(ID_KEY), EM_SETCUEBANNER, 0, cue.as_ptr() as isize);
        let mcue = wide(tr("settings.model_cue"));
        send(self.ctl(ID_MODEL), CB_SETCUEBANNER, 0, mcue.as_ptr() as isize);
        for c in Choice::ALL {
            let t = wide(c.label(lang));
            send(self.ctl(ID_PROVIDER), CB_ADDSTRING, 0, t.as_ptr() as isize);
        }
    }

    /// Places the controls for the window's DPI and sizes the window around them (centred in
    /// `center` when given: the work area of the monitor it opens on).
    fn apply_layout(&mut self, center: Option<RECT>, suggested: Option<RECT>) {
        let font = self.font;
        let lang = self.lang;
        // the longest privacy notice (a remote service) sets the height, so the window keeps its
        // size when the service changes
        let privacy = i18n::tr_fmt(lang, "settings.privacy_remote", &[("provider", provider_name(lang, Provider::OpenAi))]);
        let consent = i18n::tr(lang, "settings.consent").to_string();
        let (line_h, _) = measure(font, "Ag", 100);
        let l = layout(self.dpi, line_h, &mut |id, w| {
            let t = if id == ID_PRIVACY { privacy.as_str() } else { consent.as_str() };
            measure(font, t, w).1
        });
        for (id, r) in &l.rects {
            let c = self.ctl(*id);
            unsafe {
                let _ = MoveWindow(c, r[0], r[1], r[2], r[3], false);
                SendMessageW(c, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(0)));
            }
        }
        let style = unsafe { GetWindowLongW(self.hwnd, GWL_STYLE) } as u32;
        let ex = unsafe { GetWindowLongW(self.hwnd, GWL_EXSTYLE) } as u32;
        let mut rc = RECT { left: 0, top: 0, right: l.w, bottom: l.h };
        unsafe {
            let _ = AdjustWindowRectExForDpi(&mut rc, WINDOW_STYLE(style), false, WINDOW_EX_STYLE(ex), self.dpi);
        }
        let (ww, wh) = (rc.right - rc.left, rc.bottom - rc.top);
        let (x, y) = match (suggested, center) {
            (Some(s), _) => (s.left, s.top),
            (None, Some(a)) => (a.left + ((a.right - a.left) - ww).max(0) / 2, a.top + ((a.bottom - a.top) - wh).max(0) / 2),
            (None, None) => {
                let mut cur = RECT::default();
                unsafe {
                    let _ = GetWindowRect(self.hwnd, &mut cur);
                }
                (cur.left, cur.top)
            }
        };
        unsafe {
            let _ = SetWindowPos(self.hwnd, None, x, y, ww, wh, SWP_NOZORDER | SWP_NOACTIVATE);
            let _ = InvalidateRect(Some(self.hwnd), None, true);
        }
    }

    /// Shows the form in the controls. Notifications our own updates cause are ignored (the
    /// window's state is borrowed meanwhile).
    fn refresh(&mut self) {
        let f = self.form.clone();
        send(self.ctl(ID_PROVIDER), CB_SETCURSEL, f.choice.index(), 0);
        // the model list: the service's suggestions
        let model = self.ctl(ID_MODEL);
        send(model, CB_RESETCONTENT, 0, 0);
        if let Some(p) = f.choice.provider() {
            for m in p.model_suggestions() {
                let t = wide(m);
                send(model, CB_ADDSTRING, 0, t.as_ptr() as isize);
            }
        }
        set_text(model, &f.model);
        set_text(self.ctl(ID_BASE_URL), &f.base_url);
        set_text(self.ctl(ID_CAP), &f.cap);
        let used = self.backend.used_today();
        let unit = if used > 0 { i18n::tr_fmt(self.lang, "settings.cap_used", &[("n", &used.to_string())]) } else { self.tr("settings.cap_unit").to_string() };
        set_text(self.ctl(ID_CAP_UNIT), &unit);
        set_text(self.ctl(ID_PRIVACY), &f.privacy(self.lang));
        self.refresh_state();
    }

    /// The parts that follow typing: enabled controls, the consent box, the key hint, the note.
    fn refresh_state(&mut self) {
        let f = &self.form;
        let en = f.enabled();
        let set = |id: i32, on: bool| unsafe {
            let _ = EnableWindow(self.ctl(id), on);
        };
        set(ID_KEY, en.key);
        set(ID_LABELS[1], en.key);
        set(ID_REMOVE_KEY, en.remove_key);
        set(ID_MODEL, en.model);
        set(ID_LABELS[2], en.model);
        set(ID_BASE_URL, en.base_url);
        set(ID_LABELS[3], en.base_url);
        set(ID_CAP, en.cap);
        set(ID_LABELS[4], en.cap);
        set(ID_CAP_UNIT, en.cap);
        set(ID_CONSENT, en.consent);
        set(ID_TEST, en.test);
        send(self.ctl(ID_CONSENT), BM_SETCHECK, if f.consent_checked() { 1 } else { 0 }, 0);
        let hint = f.key_hint().map(|k| i18n::tr(self.lang, k)).unwrap_or("");
        set_text(self.ctl(ID_KEY_HINT), hint);
        let (text, tone) = match &f.note {
            Some(n) => (n.text.clone(), n.tone),
            None => (String::new(), NoteTone::Info),
        };
        NOTE.with(|n| n.set((self.ctl(ID_NOTE).0 as isize, tone as u8)));
        set_text(self.ctl(ID_NOTE), &text);
    }

    fn note(&mut self, key_or_text: String, tone: NoteTone) {
        self.form.note = Some(Note { text: key_or_text, tone });
        self.refresh_state();
    }

    fn log(&self, action: &'static str, ok: bool) {
        push_event(self.notify, SettingsEvent::Log { action, provider: self.form.choice.id(), ok });
    }

    /// The key to check or send: the field's text, else the key saved for this very service
    /// (another OpenAI-compatible URL has none until one is typed for it).
    fn key(&self) -> Secret {
        let typed = read_key(self.ctl(ID_KEY));
        if !typed.is_empty() {
            return typed;
        }
        let id = self.form.service_id();
        (!id.is_empty()).then(|| self.backend.stored_key(&id)).flatten().unwrap_or_else(|| Secret::new(""))
    }

    /// Returns true when the window must close.
    fn on_command(&mut self, id: i32, code: u32) -> bool {
        match (id, code) {
            (ID_PROVIDER, CBN_SELCHANGE) => {
                let i = send(self.ctl(ID_PROVIDER), CB_GETCURSEL, 0, 0);
                if let Some(c) = usize::try_from(i).ok().and_then(|i| Choice::ALL.get(i).copied()) {
                    self.form.select(c);
                    set_text(self.ctl(ID_KEY), "");
                    self.refresh();
                }
            }
            (ID_KEY, EN_CHANGE) => {
                self.form.key_typed = unsafe { GetWindowTextLengthW(self.ctl(ID_KEY)) } > 0;
                self.refresh_state();
            }
            (ID_MODEL, CBN_EDITCHANGE) => self.form.model = get_text(self.ctl(ID_MODEL)),
            (ID_MODEL, CBN_SELCHANGE) => {
                let c = self.ctl(ID_MODEL);
                let i = send(c, CB_GETCURSEL, 0, 0);
                if i >= 0 {
                    let n = send(c, CB_GETLBTEXTLEN, i as usize, 0).max(0) as usize;
                    let mut buf = vec![0u16; n + 1];
                    let got = send(c, CB_GETLBTEXT, i as usize, buf.as_mut_ptr() as isize).max(0) as usize;
                    self.form.model = String::from_utf16_lossy(&buf[..got.min(n)]);
                }
            }
            (ID_BASE_URL, EN_CHANGE) => {
                let before = self.form.model.clone();
                self.form.set_base_url(get_text(self.ctl(ID_BASE_URL)));
                if self.form.model != before {
                    set_text(self.ctl(ID_MODEL), &self.form.model);
                }
                self.refresh_state();
            }
            (ID_CAP, EN_CHANGE) => self.form.cap = get_text(self.ctl(ID_CAP)),
            (ID_CONSENT, BN_CLICKED) => {
                let on = send(self.ctl(ID_CONSENT), BM_GETCHECK, 0, 0) == 1;
                self.form.set_consent(on);
                self.refresh_state();
            }
            (ID_REMOVE_KEY, BN_CLICKED) => self.remove_key(),
            (ID_TEST, BN_CLICKED) => self.test(),
            (ID_SAVE, BN_CLICKED) => return self.save(),
            (ID_CANCEL, BN_CLICKED) => {
                self.log("cancel", true);
                return true;
            }
            _ => {}
        }
        false
    }

    fn remove_key(&mut self) {
        let id = self.form.service_id();
        if id.is_empty() || self.form.keys_locked {
            return;
        }
        match self.backend.store_key(&id, None) {
            Ok(keys) => {
                self.form.set_stored(&id, false);
                push_event(self.notify, SettingsEvent::KeysChanged(keys));
                self.log("remove_key", true);
                self.note(self.tr("settings.key_removed").to_string(), NoteTone::Info);
            }
            Err(()) => {
                self.log("remove_key", false);
                self.note(self.tr("settings.err_save").to_string(), NoteTone::Error);
            }
        }
    }

    fn test(&mut self) {
        let key = self.key();
        let cfg = match self.form.test_config(key) {
            Ok(c) => c,
            Err(k) => {
                self.note(self.tr(k).to_string(), NoteTone::Error);
                return;
            }
        };
        self.gen += 1;
        let gen = self.gen;
        self.form.testing = gen;
        self.note(self.tr("settings.testing").to_string(), NoteTone::Info);
        let slot = self.test_slot.clone();
        let hwnd = self.hwnd.0 as isize;
        self.backend.run_test(
            cfg,
            self.test_target.clone(),
            Box::new(move |r| {
                if let Ok(mut s) = slot.lock() {
                    *s = Some((gen, r));
                }
                unsafe {
                    let _ = PostMessageW(Some(HWND(hwnd as *mut core::ffi::c_void)), WM_APP_TEST_DONE, WPARAM(0), LPARAM(0));
                }
            }),
        );
    }

    fn on_test_done(&mut self) {
        let done = self.test_slot.lock().ok().and_then(|mut s| s.take());
        let Some((gen, (result, ms))) = done else { return };
        if gen != self.form.testing {
            return;
        }
        self.form.testing = 0;
        match result {
            Ok(text) => {
                self.log("test", true);
                let shown: String = text.chars().take(160).collect();
                self.note(i18n::tr_fmt(self.lang, "settings.test_ok", &[("ms", &ms.to_string()), ("text", &shown)]), NoteTone::Ok);
            }
            Err(f) => {
                self.log("test", false);
                let why = reason(&f, self.lang, 0);
                self.note(i18n::tr_fmt(self.lang, "settings.test_failed", &[("reason", &why)]), NoteTone::Error);
            }
        }
    }

    /// Returns true when saved (the window closes).
    fn save(&mut self) -> bool {
        let typed = read_key(self.ctl(ID_KEY));
        let key = self.key();
        let translation = match self.form.check_save(key) {
            Ok(s) => s,
            Err(k) => {
                self.log("save", false);
                self.note(self.tr(k).to_string(), NoteTone::Error);
                return false;
            }
        };
        // a typed key goes into the key file first, under this service's id; the settings follow
        // only when it is there
        let slot = translation.service_id();
        let keys = match self.form.choice.provider() {
            Some(_) if !typed.is_empty() => match self.backend.store_key(&slot, Some(typed)) {
                Ok(k) => {
                    self.form.set_stored(&slot, true);
                    Some(k)
                }
                Err(()) => {
                    self.log("save", false);
                    self.note(self.tr("settings.err_save").to_string(), NoteTone::Error);
                    return false;
                }
            },
            _ => None,
        };
        set_text(self.ctl(ID_KEY), "");
        let keys = keys.unwrap_or_else(|| self.backend.keys());
        self.log("save", true);
        push_event(self.notify, SettingsEvent::Saved { translation, keys });
        true
    }

    fn on_dpi(&mut self, dpi: u32, suggested: RECT) {
        self.dpi = dpi;
        let old = self.font;
        self.font = message_font(dpi);
        self.apply_layout(None, Some(suggested));
        unsafe {
            let _ = DeleteObject(old.into());
        }
    }
}

impl Drop for SettingsWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.font.into());
        }
    }
}

/// Runs `f` on the open window's state; None when there is none or it is busy (a notification
/// our own update caused).
fn with_window<R>(f: impl FnOnce(&mut SettingsWindow) -> R) -> Option<R> {
    WINDOW.with(|w| w.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

unsafe extern "system" fn settings_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let (id, code) = ((wp.0 & 0xFFFF) as i32, ((wp.0 >> 16) & 0xFFFF) as u32);
            // Enter and Esc arrive as IDOK and IDCANCEL with code 0 (BN_CLICKED)
            if with_window(|w| w.on_command(id, code)) == Some(true) {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            }
            LRESULT(0)
        }
        WM_APP_TEST_DONE => {
            with_window(|w| w.on_test_done());
            LRESULT(0)
        }
        // the title bar's close button and Alt+F4: like Cancel
        WM_CLOSE => {
            with_window(|w| w.log("cancel", true));
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_CTLCOLORSTATIC => {
            let (note, tone) = NOTE.with(Cell::get);
            let dc = HDC(wp.0 as *mut core::ffi::c_void);
            if lp.0 == note && note != 0 {
                let c = match tone {
                    1 => COLORREF(0x0000_7A1E), // green (0x1E7A00 as BGR)
                    2 => COLORREF(0x0020_20C0), // red
                    _ => unsafe { COLORREF(GetSysColor(COLOR_WINDOWTEXT)) },
                };
                unsafe {
                    SetTextColor(dc, c);
                    SetBkColor(dc, COLORREF(GetSysColor(COLOR_3DFACE)));
                    return LRESULT(GetSysColorBrush(COLOR_3DFACE).0 as isize);
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
        WM_DPICHANGED => {
            let dpi = (wp.0 & 0xFFFF) as u32;
            let suggested = unsafe { *(lp.0 as *const RECT) };
            with_window(|w| w.on_dpi(dpi, suggested));
            LRESULT(0)
        }
        // Moving the window runs a modal loop that dispatches (and so drops) the main window's
        // messages: once it ends, the main window catches up (wParam 1).
        WM_EXITSIZEMOVE | WM_EXITMENULOOP => {
            if let Some(n) = with_window(|w| w.notify).filter(|n| *n != 0) {
                unsafe {
                    let _ = PostMessageW(Some(HWND(n as *mut core::ffi::c_void)), WM_APP_SETTINGS, WPARAM(1), LPARAM(0));
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
        // IsDialogMessage asks for the default button (Enter)
        DM_GETDEFID => LRESULT(((DC_HASDEFID as isize) << 16) | ID_SAVE as isize),
        WM_DESTROY => {
            let notify = with_window(|w| w.notify).unwrap_or(0);
            OPEN.with(|o| o.set(0));
            NOTE.with(|n| n.set((0, 0)));
            push_event(notify, SettingsEvent::Closed);
            LRESULT(0)
        }
        WM_NCDESTROY => {
            // the state goes once nothing borrows it any more
            WINDOW.with(|w| {
                if let Ok(mut g) = w.try_borrow_mut() {
                    *g = None;
                }
            });
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

fn register_class(icon: Option<HICON>) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(settings_proc),
            hInstance: hinstance(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH((COLOR_3DFACE.0 + 1) as usize as *mut core::ffi::c_void),
            hIcon: icon.unwrap_or_default(),
            hIconSm: icon.unwrap_or_default(),
            lpszClassName: SETTINGS_CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
    });
}

/// The monitor under the cursor (the tray icon was just clicked): its work area and DPI.
fn cursor_monitor() -> (RECT, u32) {
    unsafe {
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTOPRIMARY);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        (mi.rcWork, dx.max(48))
    }
}

/// Opens the window (or brings the open one to the front). Returns false when it could not be
/// created.
pub fn open(a: OpenArgs) -> bool {
    if let Some(h) = current() {
        if a.show {
            let h = HWND(h as *mut core::ffi::c_void);
            unsafe {
                let _ = ShowWindow(h, SW_SHOWNORMAL);
                let _ = SetForegroundWindow(h);
            }
        }
        return true;
    }
    register_class(a.icon);
    let (work, dpi) = if a.show { cursor_monitor() } else { (RECT { left: -32000, top: -32000, right: -31000, bottom: -31000 }, 96) };
    let title = wide(i18n::tr(a.lang, "settings.title"));
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN;
    let ctx = visual_styles();
    let mut cookie = 0usize;
    let active = ctx.is_some_and(|c| unsafe { ActivateActCtx(Some(c), &mut cookie) }.is_ok());
    let hwnd = unsafe {
        CreateWindowExW(WS_EX_CONTROLPARENT | WS_EX_APPWINDOW, SETTINGS_CLASS, PCWSTR(title.as_ptr()), style, work.left, work.top, 400, 300, None, None, Some(hinstance()), None)
    };
    let Ok(hwnd) = hwnd else {
        if active {
            unsafe {
                let _ = DeactivateActCtx(0, cookie);
            }
        }
        return false;
    };
    // the monitor the window opened on (the system gives a window its monitor's DPI)
    let dpi = if a.show { Some(unsafe { GetDpiForWindow(hwnd) }).filter(|d| *d >= 48).unwrap_or(dpi) } else { dpi };
    let backend = a.backend;
    let keys = backend.keys();
    let mut form = Form::open(&a.saved, keys.slots());
    form.keys_locked = !backend.writable();
    let mut used = SettingsWindow {
        hwnd,
        ctl: Vec::new(),
        font: message_font(dpi),
        dpi,
        lang: a.lang,
        form,
        backend,
        notify: a.notify,
        test_target: a.test_target,
        gen: 0,
        test_slot: Arc::new(Mutex::new(None)),
    };
    used.create_controls();
    if active {
        unsafe {
            let _ = DeactivateActCtx(0, cookie);
        }
    }
    used.apply_layout(Some(work), None);
    used.refresh();
    let notify = used.notify;
    OPEN.with(|o| o.set(hwnd.0 as isize));
    WINDOW.with(|w| *w.borrow_mut() = Some(used));
    push_event(notify, SettingsEvent::Log { action: "open", provider: with_window(|w| w.form.choice.id()).unwrap_or("local"), ok: true });
    if a.show {
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
            let _ = SetForegroundWindow(hwnd);
            if let Ok(c) = GetDlgItem(Some(hwnd), ID_PROVIDER) {
                let _ = SetFocus(Some(c));
            }
        }
    }
    true
}

/// Test access to the open window: its control `id`.
pub fn control(id: i32) -> Option<isize> {
    let h = current()?;
    unsafe { GetDlgItem(Some(HWND(h as *mut core::ffi::c_void)), id) }.ok().map(|c| c.0 as isize)
}

/// Test access to the open window's form.
pub fn form() -> Option<Form> {
    with_window(|w| w.form.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tp_engine::remote::FallbackKind;

    fn saved(provider: &str) -> RemoteSettings {
        RemoteSettings { provider: provider.into(), ..RemoteSettings::default() }
    }

    fn key(s: &str) -> Secret {
        Secret::new(s)
    }

    #[test]
    fn the_local_translator_needs_nothing() {
        let f = Form::open(&RemoteSettings::default(), []);
        assert_eq!(f.choice, Choice::Local);
        assert_eq!(f.enabled(), Enabled::default(), "every remote field off");
        assert_eq!(f.key_hint(), None);
        assert_eq!(f.privacy(Lang::En), "Translations are made on this computer. Nothing is sent anywhere.");
        let s = f.check_save(key("")).unwrap();
        assert_eq!(s.provider, "local");
        assert!(s.provider().is_none());
    }

    #[test]
    fn a_remote_service_asks_for_a_key_and_the_consent() {
        let mut f = Form::open(&RemoteSettings::default(), []);
        f.select(Choice::Remote(Provider::DeepL));
        let en = f.enabled();
        assert!(en.key && en.model && en.cap && en.consent && !en.base_url && !en.remove_key);
        assert!(!en.test, "no key yet");
        assert_eq!(f.key_hint(), Some("settings.key_none"));
        assert!(f.privacy(Lang::En).contains("sent to DeepL") && f.privacy(Lang::En).contains("The sender's name, the channel and the realm are never sent (names written inside a message are part of its text)"));
        assert!(f.privacy(Lang::Fr).contains("envoyés à DeepL"));
        f.key_typed = true;
        assert!(f.enabled().test);
        // Save: the consent first, then the key
        assert_eq!(f.check_save(key("abc:fx")).unwrap_err(), "settings.err_consent");
        f.set_consent(true);
        assert!(f.consent_checked());
        assert_eq!(f.check_save(key("")).unwrap_err(), "settings.err_key");
        let s = f.check_save(key("abc:fx")).unwrap();
        assert_eq!((s.provider.as_str(), s.consent.as_str(), s.daily_char_cap), ("deepl", "deepl", 0));
        assert!(s.has_consent());
        assert!(!format!("{s:?}").contains("abc:fx"), "the key is never in the settings");
    }

    #[test]
    fn a_saved_key_is_kept_and_can_be_removed() {
        let mut s = saved("claude");
        s.model = "claude-haiku-5-5".into();
        s.give_consent();
        let mut f = Form::open(&s, ["claude"]);
        assert_eq!(f.choice, Choice::Remote(Provider::Claude));
        assert_eq!(f.model, "claude-haiku-5-5");
        assert!(f.consent_checked());
        assert_eq!(f.key_hint(), Some("settings.key_saved"));
        let en = f.enabled();
        assert!(en.test && en.remove_key);
        // the saved key passes the check (the window hands it in when the field is empty)
        assert!(f.check_save(key("sk-ant-saved")).is_ok());
        f.set_stored("claude", false);
        assert!(!f.enabled().remove_key && !f.enabled().test);
        assert_eq!(f.key_hint(), Some("settings.key_none"));
    }

    #[test]
    fn switching_services_shows_their_model_and_their_consent() {
        let mut s = saved("claude");
        s.model = "claude-haiku-5-5".into();
        s.give_consent();
        let mut f = Form::open(&s, []);
        f.key_typed = true;
        f.select(Choice::Remote(Provider::Gemini));
        assert_eq!(f.model, Provider::Gemini.default_model());
        assert!(!f.consent_checked(), "another service, another consent");
        assert!(!f.key_typed, "a typed key belongs to the service it was typed for");
        f.select(Choice::Remote(Provider::Claude));
        assert_eq!(f.model, "claude-haiku-5-5", "the saved model again");
        assert!(f.consent_checked());
        f.select(Choice::Remote(Provider::DeepL));
        assert_eq!(f.model, "", "DeepL's default model type");
    }

    #[test]
    fn an_openai_compatible_server_is_its_own_service() {
        let mut f = Form::open(&RemoteSettings::default(), []);
        f.select(Choice::Remote(Provider::OpenAi));
        assert_eq!(f.base_url, OPENAI_DEFAULT_BASE, "the default URL is shown");
        assert!(f.enabled().base_url);
        assert_eq!(f.service_id(), "openai:api.openai.com/v1");
        f.set_consent(true);
        f.base_url = "https://api.groq.com/openai/v1".into();
        assert!(!f.consent_checked(), "another host: asked again");
        f.base_url = "https://api.openai.com/v1/".into();
        assert!(f.consent_checked(), "the same host");
        // saved as empty when it is the default
        let s = f.check_save(key("sk-test")).unwrap();
        assert_eq!((s.base_url.as_str(), s.model.as_str()), ("", Provider::OpenAi.default_model()));
        // a server on this computer needs no key, but a model name
        f.base_url = "http://localhost:11434/v1".into();
        f.model = String::new();
        assert!(!f.key_needed());
        assert_eq!(f.key_hint(), Some("settings.key_not_needed"));
        assert!(f.enabled().test, "Test works without a key there");
        f.set_consent(true);
        assert_eq!(f.check_save(key("")).unwrap_err(), "settings.err_model");
        f.model = "qwen3:4b".into();
        let s = f.check_save(key("")).unwrap();
        assert_eq!((s.consent.as_str(), s.base_url.as_str()), ("openai:localhost:11434/v1", "http://localhost:11434/v1"));
        // plain http elsewhere would carry the key in clear
        f.base_url = "http://192.168.1.20:1234/v1".into();
        f.set_consent(true);
        assert_eq!(f.check_save(key("sk-test")).unwrap_err(), "settings.err_url");
        f.base_url = "ftp://example.com".into();
        assert!(!f.enabled().consent, "no service to consent to");
        assert_eq!(f.check_save(key("sk-test")).unwrap_err(), "settings.err_url");
        f.base_url = "https://api.mistral.ai/v1".into();
        f.model = "mistral small".into();
        f.set_consent(true);
        assert_eq!(f.check_save(key("k")).unwrap_err(), "settings.err_model_bad");
    }

    #[test]
    fn the_daily_limit_is_a_whole_number() {
        let mut s = saved("gemini");
        s.give_consent();
        s.daily_char_cap = 20_000;
        let mut f = Form::open(&s, ["gemini"]);
        assert_eq!(f.cap, "20000");
        f.cap = "50 000".into();
        assert_eq!(f.check_save(key("AIzaTEST")).unwrap_err(), "settings.err_cap");
        f.cap = String::new();
        assert_eq!(f.check_save(key("AIzaTEST")).unwrap().daily_char_cap, 0, "empty: no limit");
        f.cap = " 75000 ".into();
        assert_eq!(f.check_save(key("AIzaTEST")).unwrap().daily_char_cap, 75_000);
        f.cap = "4000000000".into();
        assert_eq!(f.check_save(key("AIzaTEST")).unwrap_err(), "settings.err_cap");
        // going local keeps the consent and the limit for later
        f.cap = "1000".into();
        f.select(Choice::Local);
        let l = f.check_save(key("")).unwrap();
        assert_eq!((l.provider.as_str(), l.consent.as_str(), l.daily_char_cap), ("local", "gemini", 1000));
    }

    #[test]
    fn test_needs_no_consent() {
        let mut f = Form::open(&RemoteSettings::default(), []);
        f.select(Choice::Remote(Provider::Claude));
        assert!(!f.consent_checked());
        let cfg = f.test_config(key("sk-ant-x")).expect("a config without consent");
        assert_eq!((cfg.provider, cfg.model.as_str()), (Provider::Claude, "claude-opus-5-5"));
        assert_eq!(f.test_config(key("")).err(), Some("settings.err_key"));
    }

    #[test]
    fn a_saved_openai_key_belongs_to_its_server_only() {
        // review finding: a key saved for api.openai.com was sent to any base URL typed later
        let mut s = saved("openai");
        s.give_consent();
        let mut f = Form::open(&s, ["openai:api.openai.com/v1"]);
        assert!(f.has_stored_key() && f.enabled().test);
        assert_eq!(f.key_hint(), Some("settings.key_saved"));
        f.set_base_url("https://free-llm.example/v1".into());
        assert!(!f.has_stored_key(), "another server: no saved key");
        assert!(!f.enabled().test && !f.enabled().remove_key, "Test needs a key typed for this server");
        assert_eq!(f.key_hint(), Some("settings.key_none"));
        assert_eq!(f.model, "", "OpenAI's default model is not that server's");
        f.model = "m".into();
        f.set_consent(true);
        assert_eq!(f.check_save(key("")).unwrap_err(), "settings.err_key");
        // a server on the home network needs no key at all, and is not sent the saved one
        f.set_base_url("http://192.168.1.69:11434/v1".into());
        assert!(!f.key_needed() && !f.has_stored_key());
        assert_eq!(f.key_hint(), Some("settings.key_not_needed"));
        f.set_consent(true);
        assert_eq!(f.check_save(key("")).unwrap().consent, "openai:192.168.1.69:11434/v1");
        // back to OpenAI's server: its saved key again
        f.set_base_url(OPENAI_DEFAULT_BASE.into());
        assert!(f.has_stored_key());
    }

    #[test]
    fn another_openai_compatible_server_must_name_its_model() {
        // review finding: gpt-6-luna was filled in for Groq, OpenRouter or Ollama
        let mut f = Form::open(&RemoteSettings::default(), []);
        f.select(Choice::Remote(Provider::OpenAi));
        assert_eq!(f.model, "gpt-6-luna", "OpenAI's own server: its default model");
        f.set_base_url("https://api.groq.com/openai/v1".into());
        assert_eq!(f.model, "", "cleared once the URL names another server");
        f.set_consent(true);
        assert_eq!(f.check_save(key("gsk-x")).unwrap_err(), "settings.err_model");
        // a model the player typed stays
        f.model = "llama-3.3-70b-versatile".into();
        f.set_base_url("https://api.groq.com/openai/v1/".into());
        assert_eq!(f.model, "llama-3.3-70b-versatile");
        // choosing the service again with another server in the field: no model filled in
        f.select(Choice::Remote(Provider::DeepL));
        f.select(Choice::Remote(Provider::OpenAi));
        assert_eq!(f.model, "");
    }

    #[test]
    fn a_key_file_that_was_not_read_is_never_written() {
        // review finding: a backend built from an empty set erased the other services' keys
        let dir = std::env::temp_dir().join(format!("tp_app_keylock_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("keys.bin");
        let mut k = Keys::default();
        k.set("deepl", Secret::new("deepl-key:fx"));
        k.set("claude", Secret::new("sk-ant-claude"));
        keystore::save(&path, &k).unwrap();
        let mut locked = KeyFileBackend::new(path.clone(), None, dir.clone());
        assert!(!locked.writable());
        assert!(locked.store_key("gemini", Some(Secret::new("AIzaX"))).is_err());
        assert!(locked.store_key("claude", None).is_err());
        let on_disk = keystore::load(&path).unwrap();
        assert!(on_disk.get("deepl").is_some() && on_disk.get("claude").is_some(), "nothing erased");
        // read first: saving one key keeps the others
        let mut open = KeyFileBackend::new(path.clone(), Some(on_disk), dir.clone());
        let after = open.store_key("gemini", Some(Secret::new("AIzaX"))).unwrap();
        assert_eq!(after.slots().collect::<Vec<_>>(), ["claude", "deepl", "gemini"]);
        assert_eq!(keystore::load(&path).unwrap().slots().count(), 3);
        // a locked window: no key field, no Remove, and why
        let mut f = Form::open(&saved("claude"), []);
        f.keys_locked = true;
        assert!(!f.enabled().key && !f.enabled().remove_key);
        assert_eq!(f.key_hint(), Some("settings.keys_unavailable"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_layout_fits_and_nothing_overlaps() {
        for dpi in [96u32, 120, 144, 192] {
            let line = (16 * dpi as i32) / 96;
            let l = layout(dpi, line, &mut |id, w| if id == ID_PRIVACY { 5 * line * 400 / w.max(1) } else { line });
            assert!(l.w > 0 && l.h > 0);
            let field = |id: i32| {
                let r = l.rect(id);
                // a combo box's rectangle includes its list: its field is one row
                if id == ID_PROVIDER || id == ID_MODEL {
                    [r[0], r[1], r[2], l.rect(ID_BASE_URL)[3]]
                } else {
                    r
                }
            };
            let ids: Vec<i32> = l.rects.iter().map(|(i, _)| *i).collect();
            for &a in &ids {
                let r = field(a);
                assert!(r[0] >= 0 && r[1] >= 0 && r[0] + r[2] <= l.w && r[1] + r[3] <= l.h, "{a} {r:?} in {}x{} at {dpi}", l.w, l.h);
                for &b in &ids {
                    if a < b {
                        let q = field(b);
                        let overlap = r[0] < q[0] + q[2] && q[0] < r[0] + r[2] && r[1] < q[1] + q[3] && q[1] < r[1] + r[3];
                        assert!(!overlap, "{a} {r:?} and {b} {q:?} at {dpi}");
                    }
                }
            }
            // Save and Cancel at the bottom right, Cancel last
            let (save, cancel) = (l.rect(ID_SAVE), l.rect(ID_CANCEL));
            assert!(save[0] < cancel[0] && cancel[0] + cancel[2] < l.w && save[1] == cancel[1]);
            assert!(ids.iter().all(|i| l.rect(*i)[1] <= save[1]));
        }
    }

    // ---- the Win32 window, created hidden (never shown, the focus never moves) ----

    struct FakeBackend {
        keys: Keys,
        fail_store: bool,
        locked: bool,
        answer: Result<String, Fallback>,
        tests: Arc<Mutex<Vec<(Provider, String)>>>,
    }

    impl Backend for FakeBackend {
        fn keys(&self) -> Keys {
            self.keys.clone()
        }
        fn writable(&self) -> bool {
            !self.locked
        }
        fn store_key(&mut self, slot: &str, key: Option<Secret>) -> Result<Keys, ()> {
            if self.fail_store || self.locked {
                return Err(());
            }
            match key {
                Some(k) => self.keys.set(slot, k),
                None => self.keys.remove(slot),
            }
            Ok(self.keys.clone())
        }
        fn run_test(&mut self, cfg: RemoteConfig, target: String, done: Box<dyn FnOnce(TestResult) + Send>) {
            self.tests.lock().unwrap().push((cfg.provider, target));
            done((self.answer.clone(), 42));
        }
        fn used_today(&self) -> u64 {
            1234
        }
    }

    type Calls = Arc<Mutex<Vec<(Provider, String)>>>;

    fn fake(keys: Keys, answer: Result<String, Fallback>) -> (Box<FakeBackend>, Calls) {
        let tests = Arc::new(Mutex::new(Vec::new()));
        (Box::new(FakeBackend { keys, fail_store: false, locked: false, answer, tests: tests.clone() }), tests)
    }

    fn open_hidden(saved: RemoteSettings, backend: Box<dyn Backend>) -> HWND {
        let _ = take_events();
        assert!(open(OpenArgs { lang: Lang::En, saved, backend, notify: 0, test_target: "de".into(), show: false, icon: None }));
        HWND(current().expect("open") as *mut core::ffi::c_void)
    }

    fn ctl(id: i32) -> HWND {
        HWND(control(id).expect("control") as *mut core::ffi::c_void)
    }

    fn command(hwnd: HWND, id: i32, code: u32) {
        unsafe {
            SendMessageW(hwnd, WM_COMMAND, Some(WPARAM(((code as usize) << 16) | id as usize)), Some(LPARAM(ctl(id).0 as isize)));
        }
    }

    fn enabled(id: i32) -> bool {
        unsafe { windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(ctl(id)) }.as_bool()
    }

    fn pump(hwnd: HWND) {
        let mut msg = MSG::default();
        unsafe {
            while PeekMessageW(&mut msg, Some(hwnd), 0, 0, PM_REMOVE).as_bool() {
                DispatchMessageW(&msg);
            }
        }
    }

    fn actions(evs: &[SettingsEvent]) -> Vec<&'static str> {
        evs.iter()
            .map(|e| match e {
                SettingsEvent::Log { action, .. } => *action,
                SettingsEvent::Saved { .. } => "SAVED",
                SettingsEvent::KeysChanged(_) => "KEYS",
                SettingsEvent::Closed => "CLOSED",
            })
            .collect()
    }

    #[test]
    fn the_window_is_built_in_code_localized_and_never_takes_the_focus_in_tests() {
        let fg = unsafe { GetForegroundWindow() };
        let (b, _) = fake(Keys::default(), Ok(String::new()));
        let hwnd = open_hidden(RemoteSettings::default(), b);
        unsafe {
            assert!(!IsWindowVisible(hwnd).as_bool(), "created hidden");
            assert_eq!(GetForegroundWindow(), fg, "the focus never moved");
        }
        assert_eq!(get_text(hwnd), "Translation engine - Translate Companion");
        assert_eq!(send(ctl(ID_PROVIDER), CB_GETCOUNT, 0, 0), 5);
        assert_eq!(send(ctl(ID_PROVIDER), CB_GETCURSEL, 0, 0), 0, "local");
        assert_eq!(get_text(ctl(ID_SAVE)), "Save");
        assert_eq!(get_text(ctl(ID_CONSENT)), "Send the text of chat messages and glossary terms to this service");
        assert_eq!(get_text(ctl(ID_CAP_UNIT)), "characters per day (0 = no limit) - 1234 used today");
        assert!(!enabled(ID_KEY) && !enabled(ID_TEST) && !enabled(ID_CONSENT) && enabled(ID_SAVE));
        let style = unsafe { GetWindowLongW(ctl(ID_KEY), GWL_STYLE) } as u32;
        assert!(style & ES_PASSWORD as u32 != 0, "the key is masked");
        // a second open brings the same window
        let (b2, _) = fake(Keys::default(), Ok(String::new()));
        assert!(open(OpenArgs { lang: Lang::En, saved: RemoteSettings::default(), backend: b2, notify: 0, test_target: "de".into(), show: false, icon: None }));
        assert_eq!(current(), Some(hwnd.0 as isize));
        // Esc (IDCANCEL) closes without saving
        command(hwnd, ID_CANCEL, BN_CLICKED);
        assert_eq!(current(), None);
        let evs = take_events();
        assert_eq!(actions(&evs), vec!["open", "cancel", "CLOSED"]);
        unsafe {
            assert!(!IsWindow(Some(hwnd)).as_bool());
            assert_eq!(GetForegroundWindow(), fg);
        }
    }

    #[test]
    fn the_window_saves_a_service_with_its_key_and_consent() {
        let (b, tests) = fake(Keys::default(), Ok("Hallo! Hat jemand Lust auf einen Dungeon heute Abend?".into()));
        let hwnd = open_hidden(RemoteSettings::default(), b);
        // DeepL in the list
        send(ctl(ID_PROVIDER), CB_SETCURSEL, 1, 0);
        command(hwnd, ID_PROVIDER, CBN_SELCHANGE);
        assert_eq!(form().unwrap().choice, Choice::Remote(Provider::DeepL));
        assert!(enabled(ID_KEY) && enabled(ID_CONSENT) && !enabled(ID_TEST) && !enabled(ID_BASE_URL));
        assert_eq!(get_text(ctl(ID_KEY_HINT)), "No key saved yet. It is stored encrypted for your Windows account.");
        // typing the key (the edit control notifies its parent)
        set_text(ctl(ID_KEY), "  abc-123:fx ");
        assert!(form().unwrap().key_typed && enabled(ID_TEST));
        // Test: the synthetic sentence in the target language, no consent needed
        command(hwnd, ID_TEST, BN_CLICKED);
        pump(hwnd);
        assert_eq!(tests.lock().unwrap().as_slice(), &[(Provider::DeepL, "de".to_string())]);
        assert_eq!(form().unwrap().note.unwrap().tone, NoteTone::Ok);
        assert_eq!(get_text(ctl(ID_NOTE)), "It works (42 ms): Hallo! Hat jemand Lust auf einen Dungeon heute Abend?");
        // Save without the consent: refused, the window stays
        command(hwnd, ID_SAVE, BN_CLICKED);
        assert_eq!(get_text(ctl(ID_NOTE)), "Tick the box to allow sending chat text to this service.");
        assert_eq!(form().unwrap().note.unwrap().tone, NoteTone::Error);
        assert!(current().is_some());
        send(ctl(ID_CONSENT), BM_SETCHECK, 1, 0);
        command(hwnd, ID_CONSENT, BN_CLICKED);
        assert!(form().unwrap().consent_checked());
        command(hwnd, ID_SAVE, BN_CLICKED);
        assert_eq!(current(), None, "saved and closed");
        let evs = take_events();
        assert_eq!(actions(&evs), vec!["open", "test", "save", "save", "SAVED", "CLOSED"]);
        let saved = evs.iter().find_map(|e| match e {
            SettingsEvent::Saved { translation, keys } => Some((translation.clone(), keys.get("deepl").map(|k| k.expose().to_string()))),
            _ => None,
        });
        let (t, k) = saved.unwrap();
        assert_eq!((t.provider.as_str(), t.consent.as_str()), ("deepl", "deepl"));
        assert_eq!(k.as_deref(), Some("abc-123:fx"), "the key went to the key file, trimmed");
    }

    #[test]
    fn a_failed_test_shows_the_reason_and_a_key_can_be_removed() {
        let mut keys = Keys::default();
        keys.set("claude", Secret::new("sk-ant-old"));
        let mut s = saved("claude");
        s.give_consent();
        let fb = Fallback { provider: Provider::Claude, kind: FallbackKind::Http, status: Some(401), message: "invalid x-api-key".into() };
        let (b, _) = fake(keys, Err(fb));
        let hwnd = open_hidden(s, b);
        assert_eq!(send(ctl(ID_PROVIDER), CB_GETCURSEL, 0, 0), 4);
        assert_eq!(send(ctl(ID_CONSENT), BM_GETCHECK, 0, 0), 1, "the saved consent");
        assert!(enabled(ID_TEST) && enabled(ID_REMOVE_KEY), "the saved key is enough");
        assert_eq!(get_text(ctl(ID_KEY)), "", "a saved key is never shown");
        command(hwnd, ID_TEST, BN_CLICKED);
        pump(hwnd);
        assert_eq!(get_text(ctl(ID_NOTE)), "Test failed: HTTP 401 - invalid x-api-key");
        assert_eq!(form().unwrap().note.unwrap().tone, NoteTone::Error);
        command(hwnd, ID_REMOVE_KEY, BN_CLICKED);
        assert!(!enabled(ID_REMOVE_KEY) && !enabled(ID_TEST));
        assert_eq!(get_text(ctl(ID_NOTE)), "The saved key was removed.");
        // the title bar's close button: like Cancel
        unsafe {
            SendMessageW(hwnd, WM_CLOSE, None, None);
        }
        assert_eq!(current(), None);
        let evs = take_events();
        assert_eq!(actions(&evs), vec!["open", "test", "KEYS", "remove_key", "cancel", "CLOSED"]);
        assert!(evs.iter().any(|e| matches!(e, SettingsEvent::KeysChanged(k) if k.get("claude").is_none())));
    }

    #[test]
    fn a_key_file_that_cannot_be_written_keeps_the_window_open() {
        let (mut b, _) = fake(Keys::default(), Ok(String::new()));
        b.fail_store = true;
        let hwnd = open_hidden(RemoteSettings::default(), b);
        send(ctl(ID_PROVIDER), CB_SETCURSEL, 3, 0);
        command(hwnd, ID_PROVIDER, CBN_SELCHANGE);
        set_text(ctl(ID_KEY), "AIzaTESTKEY");
        send(ctl(ID_CONSENT), BM_SETCHECK, 1, 0);
        command(hwnd, ID_CONSENT, BN_CLICKED);
        command(hwnd, ID_SAVE, BN_CLICKED);
        assert!(current().is_some());
        assert_eq!(get_text(ctl(ID_NOTE)), "The settings could not be saved.");
        assert!(!take_events().iter().any(|e| matches!(e, SettingsEvent::Saved { .. })));
        close();
        assert_eq!(current(), None);
    }

    #[link(name = "user32")]
    extern "system" {
        #[link_name = "PrintWindow"]
        fn print_window(hwnd: HWND, dc: HDC, flags: u32) -> i32;
    }

    /// A picture of the window for a look at the layout (by hand: `TP_SNAPSHOT=<file.bmp>`). The
    /// window is shown far off-screen without activation and drawn with `PrintWindow`: nothing
    /// appears on screen and the focus never moves.
    #[test]
    #[ignore = "by hand: TP_SNAPSHOT=<file.bmp> cargo test --offline --lib settings_window_snapshot -- --ignored"]
    fn settings_window_snapshot() {
        let Some(out) = std::env::var_os("TP_SNAPSHOT") else { return };
        let fg = unsafe { GetForegroundWindow() };
        let mut keys = Keys::default();
        keys.set("claude", Secret::new("sk-ant-x"));
        let mut s = saved("claude");
        s.give_consent();
        s.daily_char_cap = 50_000;
        let lang = if std::env::var("TP_SNAPSHOT_LANG").as_deref() == Ok("fr") { Lang::Fr } else { Lang::En };
        let (b, _) = fake(keys, Ok("Hallo! Hat jemand Lust auf einen Dungeon heute Abend? Wir brauchen einen Heiler.".into()));
        let _ = take_events();
        assert!(open(OpenArgs { lang, saved: s, backend: b, notify: 0, test_target: "de".into(), show: false, icon: crate::tray::make_icon() }));
        let hwnd = HWND(current().unwrap() as *mut core::ffi::c_void);
        command(hwnd, ID_TEST, BN_CLICKED);
        pump(hwnd);
        unsafe {
            let _ = SetWindowPos(hwnd, None, -32000, -32000, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            pump(hwnd);
            let mut r = RECT::default();
            let _ = GetWindowRect(hwnd, &mut r);
            let (w, h) = (r.right - r.left, r.bottom - r.top);
            let screen = GetDC(None);
            let dc = CreateCompatibleDC(Some(screen));
            let bmp = CreateCompatibleBitmap(screen, w, h);
            let old = SelectObject(dc, bmp.into());
            assert!(print_window(hwnd, dc, 2) != 0, "PrintWindow");
            let mut bi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() },
                ..Default::default()
            };
            let mut px = vec![0u8; (w * h * 4) as usize];
            GetDIBits(dc, bmp, 0, h as u32, Some(px.as_mut_ptr() as *mut core::ffi::c_void), &mut bi, DIB_RGB_COLORS);
            SelectObject(dc, old);
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(dc);
            ReleaseDC(None, screen);
            let mut file = Vec::new();
            let size = 14 + 40 + px.len() as u32;
            file.extend_from_slice(b"BM");
            file.extend_from_slice(&size.to_le_bytes());
            file.extend_from_slice(&[0, 0, 0, 0]);
            file.extend_from_slice(&54u32.to_le_bytes());
            file.extend_from_slice(&40u32.to_le_bytes());
            file.extend_from_slice(&w.to_le_bytes());
            file.extend_from_slice(&h.to_le_bytes());
            file.extend_from_slice(&1u16.to_le_bytes());
            file.extend_from_slice(&32u16.to_le_bytes());
            file.extend_from_slice(&[0u8; 24]);
            file.extend_from_slice(&px);
            std::fs::write(&out, file).unwrap();
            assert_eq!(GetForegroundWindow(), fg, "the focus never moved");
        }
        close();
    }

    #[test]
    fn the_visual_styles_context_can_be_made() {
        // Windows 10 and 11 ship the common controls 6 manifest in shell32.dll (resource 124)
        assert!(visual_styles_available());
    }
}
