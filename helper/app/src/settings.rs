//! `%APPDATA%\TranslateProbe\settings.json` (design D.6): numbers, flags and paths only.
//! Created with defaults on first run; invalid values are replaced by their defaults.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub version: u32,
    /// auto | cpu | vulkan | cuda
    pub backend: String,
    pub gpu_margin_mb: u32,
    /// 0 = clamp(physical cores / 2, 2, 4)
    pub cpu_threads: u32,
    pub idle_unload_minutes: u32,
    pub keep_loaded: bool,
    /// auto | wgc | gdi
    pub capture: String,
    /// Capture rate while a card is visible and WoW is in front (20-60).
    pub capture_hz_active: u32,
    pub hide_from_recordings: bool,
    /// Translation text colour, 6 hex digits (RRGGBB). Default `DEFAULT_TEXT_COLOR`, a soft light
    /// blue that reads as a translation next to the chat's own colours.
    pub text_color: String,
    pub font_scale: f32,
    pub runtime_dir: String,
    pub model: String,
    /// error | warn | info | debug
    pub log_level: String,
    /// Tray item "Copy translated replies to the clipboard" (on by default, decision D6): the
    /// Helper copies a reply only when the player allowed it in the game AND this is on.
    pub reply_clipboard: bool,
    /// Tray item "Sound when a reply is copied" (`MessageBeep(MB_OK)`, off by default).
    pub reply_sound: bool,
}

/// Default translation text colour (native look, 2026-10-18).
pub const DEFAULT_TEXT_COLOR: &str = "A8D8FF";
/// The default before the native look: a file still holding it gets the new default (it was
/// written with the defaults on first run, not chosen).
pub const OLD_DEFAULT_TEXT_COLOR: &str = "9FD4FF";

impl Default for Settings {
    fn default() -> Self {
        Settings {
            version: 1,
            backend: "auto".into(),
            gpu_margin_mb: 1500,
            cpu_threads: 0,
            idle_unload_minutes: 10,
            keep_loaded: false,
            capture: "auto".into(),
            capture_hz_active: 30,
            hide_from_recordings: false,
            text_color: DEFAULT_TEXT_COLOR.into(),
            font_scale: 1.0,
            runtime_dir: String::new(),
            model: String::new(),
            log_level: "info".into(),
            reply_clipboard: true,
            reply_sound: false,
        }
    }
}

/// What `load` did, for the log (no paths or contents).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadOutcome {
    /// The file did not exist; defaults were written.
    Created,
    Loaded,
    /// The file was not valid JSON; defaults are used and the file is left untouched.
    Unreadable,
}

impl Settings {
    /// Replaces invalid values by their defaults; returns the names of the fields it fixed.
    pub fn validate(&mut self) -> Vec<&'static str> {
        let d = Settings::default();
        let mut fixed = Vec::new();
        if self.version != 1 {
            self.version = 1;
            fixed.push("version");
        }
        if !["auto", "cpu", "vulkan", "cuda"].contains(&self.backend.as_str()) {
            self.backend = d.backend.clone();
            fixed.push("backend");
        }
        if !(256..=16384).contains(&self.gpu_margin_mb) {
            self.gpu_margin_mb = d.gpu_margin_mb;
            fixed.push("gpuMarginMb");
        }
        if self.cpu_threads > 64 {
            self.cpu_threads = d.cpu_threads;
            fixed.push("cpuThreads");
        }
        if !(1..=1440).contains(&self.idle_unload_minutes) {
            self.idle_unload_minutes = d.idle_unload_minutes;
            fixed.push("idleUnloadMinutes");
        }
        if !["auto", "wgc", "gdi"].contains(&self.capture.as_str()) {
            self.capture = d.capture.clone();
            fixed.push("capture");
        }
        if !(20..=60).contains(&self.capture_hz_active) {
            self.capture_hz_active = self.capture_hz_active.clamp(20, 60);
            fixed.push("captureHzActive");
        }
        if parse_hex_rgb(&self.text_color).is_none() {
            self.text_color = d.text_color.clone();
            fixed.push("textColor");
        } else if self.text_color.eq_ignore_ascii_case(OLD_DEFAULT_TEXT_COLOR) {
            // not an invalid value: the old default follows the new one (not reported)
            self.text_color = d.text_color.clone();
        }
        if !(self.font_scale.is_finite() && (0.5..=2.0).contains(&self.font_scale)) {
            self.font_scale = d.font_scale;
            fixed.push("fontScale");
        }
        if !["error", "warn", "info", "debug"].contains(&self.log_level.as_str()) {
            self.log_level = d.log_level.clone();
            fixed.push("logLevel");
        }
        fixed
    }

    pub fn text_rgb(&self) -> [u8; 3] {
        parse_hex_rgb(&self.text_color).or_else(|| parse_hex_rgb(DEFAULT_TEXT_COLOR)).unwrap_or([0xA8, 0xD8, 0xFF])
    }

    /// Reads and validates the file; writes defaults when it does not exist.
    pub fn load(path: &Path) -> (Settings, LoadOutcome, Vec<&'static str>) {
        match std::fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str::<Settings>(&text) {
                Ok(mut s) => {
                    let fixed = s.validate();
                    (s, LoadOutcome::Loaded, fixed)
                }
                Err(_) => (Settings::default(), LoadOutcome::Unreadable, Vec::new()),
            },
            Err(_) => {
                let s = Settings::default();
                let _ = s.save(path);
                (s, LoadOutcome::Created, Vec::new())
            }
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, text)
    }

    pub fn runtime_dir_opt(&self) -> Option<PathBuf> {
        (!self.runtime_dir.trim().is_empty()).then(|| PathBuf::from(self.runtime_dir.trim()))
    }

    pub fn model_opt(&self) -> Option<PathBuf> {
        (!self.model.trim().is_empty()).then(|| PathBuf::from(self.model.trim()))
    }
}

/// "RRGGBB" (an optional leading '#' is accepted) to [R, G, B].
pub fn parse_hex_rgb(s: &str) -> Option<[u8; 3]> {
    let s = s.strip_prefix('#').unwrap_or(s);
    if s.len() != 6 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

/// `%APPDATA%\TranslateProbe\settings.json`
pub fn default_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("TranslateProbe").join("settings.json"))
}

/// `%LOCALAPPDATA%\TranslateProbe` (logs\, run\)
pub fn data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("TranslateProbe")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tp_settings_test_{}_{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("settings.json")
    }

    #[test]
    fn default_json_matches_design() {
        let v = serde_json::to_value(Settings::default()).unwrap();
        let expected = serde_json::json!({"version":1,"backend":"auto","gpuMarginMb":1500,"cpuThreads":0,
            "idleUnloadMinutes":10,"keepLoaded":false,"capture":"auto","captureHzActive":30,"hideFromRecordings":false,
            "textColor":"A8D8FF","fontScale":1.0,"runtimeDir":"","model":"","logLevel":"info",
            "replyClipboard":true,"replySound":false});
        assert_eq!(v, expected);
    }

    #[test]
    fn reply_switches_default_and_round_trip() {
        // a settings file from before the conversation window gets the defaults (D6: copy on)
        let old: Settings = serde_json::from_str(r#"{"backend":"cpu"}"#).unwrap();
        assert!(old.reply_clipboard && !old.reply_sound);
        let path = temp_file("reply");
        let s = Settings { reply_clipboard: false, reply_sound: true, ..Settings::default() };
        s.save(&path).unwrap();
        let (back, _, fixed) = Settings::load(&path);
        assert!(fixed.is_empty());
        assert!(!back.reply_clipboard && back.reply_sound);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn round_trip_through_file() {
        let path = temp_file("rt");
        let (s, outcome, _) = Settings::load(&path);
        assert_eq!(outcome, LoadOutcome::Created);
        assert_eq!(s, Settings::default());
        assert!(path.exists());
        let mut custom = Settings::default();
        custom.backend = "cpu".into();
        custom.capture_hz_active = 45;
        custom.font_scale = 1.25;
        custom.text_color = "FFAA00".into();
        custom.keep_loaded = true;
        custom.save(&path).unwrap();
        let (back, outcome, fixed) = Settings::load(&path);
        assert_eq!(outcome, LoadOutcome::Loaded);
        assert!(fixed.is_empty());
        assert_eq!(back, custom);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn partial_and_unknown_fields() {
        let s: Settings = serde_json::from_str(r#"{"backend":"vulkan","futureField":3}"#).unwrap();
        assert_eq!(s.backend, "vulkan");
        assert_eq!(s.capture_hz_active, 30);
    }

    #[test]
    fn validation_fixes_bad_values() {
        let mut s: Settings = serde_json::from_str(
            r#"{"version":7,"backend":"gpu","gpuMarginMb":0,"cpuThreads":999,"idleUnloadMinutes":0,"capture":"dda",
                "captureHzActive":240,"textColor":"blue","fontScale":9.0,"logLevel":"loud"}"#,
        )
        .unwrap();
        let fixed = s.validate();
        assert_eq!(
            fixed,
            vec!["version", "backend", "gpuMarginMb", "cpuThreads", "idleUnloadMinutes", "capture", "captureHzActive",
                 "textColor", "fontScale", "logLevel"]
        );
        assert_eq!(s.capture_hz_active, 60);
        let mut d = Settings::default();
        d.capture_hz_active = 60;
        assert_eq!(s, d);
        let mut low = Settings { capture_hz_active: 5, ..Settings::default() };
        low.validate();
        assert_eq!(low.capture_hz_active, 20);
        let mut nan = Settings { font_scale: f32::NAN, ..Settings::default() };
        assert_eq!(nan.validate(), vec!["fontScale"]);
    }

    #[test]
    fn unreadable_file_keeps_defaults_and_file() {
        let path = temp_file("bad");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{not json").unwrap();
        let (s, outcome, _) = Settings::load(&path);
        assert_eq!(outcome, LoadOutcome::Unreadable);
        assert_eq!(s, Settings::default());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{not json");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn the_old_default_text_colour_follows_the_new_default() {
        let mut old: Settings = serde_json::from_str(r#"{"textColor":"9fd4ff"}"#).unwrap();
        assert!(old.validate().is_empty(), "not reported as a fix");
        assert_eq!(old.text_color, DEFAULT_TEXT_COLOR);
        let mut chosen: Settings = serde_json::from_str(r#"{"textColor":"FFFFFF"}"#).unwrap();
        chosen.validate();
        assert_eq!(chosen.text_color, "FFFFFF", "a chosen colour is kept");
    }

    #[test]
    fn hex_colours() {
        assert_eq!(parse_hex_rgb("9FD4FF"), Some([0x9F, 0xD4, 0xFF]));
        assert_eq!(parse_hex_rgb("#e08080"), Some([0xE0, 0x80, 0x80]));
        assert_eq!(parse_hex_rgb("12345"), None);
        assert_eq!(parse_hex_rgb("GG0000"), None);
        assert_eq!(Settings::default().text_rgb(), [0xA8, 0xD8, 0xFF]);
    }

    #[test]
    fn optional_paths() {
        let mut s = Settings::default();
        assert_eq!(s.runtime_dir_opt(), None);
        s.runtime_dir = "  D:\\rt  ".into();
        assert_eq!(s.runtime_dir_opt(), Some(PathBuf::from("D:\\rt")));
    }
}
