//! TranslateProbe Helper: reads the addon's pixel bridge from the WoW window, translates with the
//! local engine (tp_engine) and draws the translations over the chat rows the addon reserved.
//! Design: docs/HELPER.md (design sections D.3 and D.6, amendments A1 and A2).
//!
//! Chat text only travels inside `tp_bridge::msg::ChatText` (no Debug, no Display); the log
//! accepts typed events with numbers, enums and fixed names only.

pub mod app;
pub mod backdrop;
pub mod capture;
pub mod cli;
pub mod i18n;
pub mod log;
pub mod overlay;
pub mod perf;
pub mod scene;
pub mod settings;
pub mod tagscan;
pub mod toast;
pub mod tray;
pub mod wow;
