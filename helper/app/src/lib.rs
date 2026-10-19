//! Translate Companion (`WoWTranslateCompanion.exe`, called the Helper in the code and the dev
//! docs): reads the TranslateProbe addon's pixel bridge from the WoW window, translates with the
//! local engine (tp_engine) and draws the translations over the chat rows the addon reserved.
//! Design: docs/HELPER.md (design sections D.3 and D.6, amendments A1 and A2).
//!
//! Naming (2026-10-19, user request U2): only what users see says "Translate Companion" (the exe,
//! the tray, toasts, balloons, status strings, `--help`). Internal identifiers keep the old name:
//! the crate names, the single-instance mutex `Local\TranslateProbeHelper` (so an old
//! `TranslateProbeHelper.exe` and the new exe never run together), the window classes `TPHelper*`
//! and the `%APPDATA%\TranslateProbe` and `%LOCALAPPDATA%\TranslateProbe` folders (settings and
//! logs carry over).
//!
//! Chat text only travels inside `tp_bridge::msg::ChatText` (no Debug, no Display); the log
//! accepts typed events with numbers, enums and fixed names only.

/// The app's name, as users see it.
pub const PRODUCT_NAME: &str = "Translate Companion";
/// The exe's file name (Cargo `[[bin]]` WoWTranslateCompanion).
pub const EXE_NAME: &str = "WoWTranslateCompanion.exe";
/// Where new versions are published, as a macro so that `concat!` can use it in `--help`: the
/// Helper's one copy of the link (the addon has its own, user request U3).
#[macro_export]
macro_rules! releases_url {
    () => {
        "https://github.com/Mouchoir/WoW-Chat-Translator/releases"
    };
}
/// Where new versions are published (the tray's "Get updates", `--help`).
pub const RELEASES_URL: &str = releases_url!();

pub mod app;
pub mod backdrop;
pub mod capture;
pub mod cli;
pub mod clip;
pub mod i18n;
pub mod lane;
pub mod log;
pub mod overlay;
pub mod perf;
pub mod reply;
pub mod scene;
pub mod settings;
pub mod tagscan;
pub mod toast;
pub mod tray;
pub mod wow;
