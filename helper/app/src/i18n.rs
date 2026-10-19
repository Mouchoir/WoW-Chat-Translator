//! User-visible strings of the Helper (`i18n\en.json`, `i18n\fr.json`, embedded at build time).
//! Status strings follow the request's target language when it is en or fr, else English.

use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lang {
    En,
    Fr,
}

pub const EN_JSON: &str = include_str!("../i18n/en.json");
pub const FR_JSON: &str = include_str!("../i18n/fr.json");

/// Every key the code looks up (checked against both tables by the tests).
pub const USED_KEYS: &[&str] = &[
    "status.queued",
    "status.loading",
    "status.loading_gpu_first",
    "status.translating",
    "status.same",
    "status.nothing",
    "status.failed",
    "status.missing",
    "status.lowmem",
    "status.notinstalled",
    "status.overlay_ok",
    "reply.ready",
    "reply.ready_cut",
    "reply.same",
    "reply.same_not_copied",
    "reply.nothing",
    "reply.nothing_not_copied",
    "reply.not_copied",
    "reply.off",
    "reply.changed",
    "reply.busy",
    "reply.failed",
    "reply.missing",
    "window.missing",
    "window.failed",
    "toast.bridge_ok",
    "toast.addon_old",
    "toast.helper_old",
    "balloon.title",
    "balloon.weak_signal",
    "balloon.switched_cpu",
    "balloon.server_failed",
    "balloon.fullscreen",
    "balloon.notinstalled",
    "tray.tip",
    "tray.state.notinstalled",
    "tray.state.unloaded",
    "tray.state.loading",
    "tray.state.ready_gpu",
    "tray.state.ready_cpu",
    "tray.state.busy",
    "tray.state.error",
    "tray.unload",
    "tray.open_logs",
    "tray.copy_replies",
    "tray.reply_sound",
    "tray.exit",
];

/// Target codes with a `lang.<code>` entry (the addon's `Lang.TARGETS`).
pub const TARGETS: &[&str] = &["en", "fr", "de", "es", "it", "pt", "ru", "ko", "zh", "zh-TW"];

struct Tables {
    en: HashMap<String, String>,
    fr: HashMap<String, String>,
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| Tables {
        en: serde_json::from_str(EN_JSON).unwrap_or_default(),
        fr: serde_json::from_str(FR_JSON).unwrap_or_default(),
    })
}

/// The string for `key`: the language's table, else English, else the key itself.
pub fn tr(lang: Lang, key: &str) -> &'static str {
    let t = tables();
    let primary = match lang {
        Lang::En => &t.en,
        Lang::Fr => &t.fr,
    };
    if let Some(s) = primary.get(key).or_else(|| t.en.get(key)) {
        return s.as_str();
    }
    // Unknown key (the tests check that every key the code uses exists).
    "?"
}

/// `tr` with `{name}` placeholders replaced.
pub fn tr_fmt(lang: Lang, key: &str, args: &[(&str, &str)]) -> String {
    let mut s = tr(lang, key).to_string();
    for (name, value) in args {
        s = s.replace(&format!("{{{name}}}"), value);
    }
    s
}

/// The language name of a target code, in `lang` ("French", "anglais").
pub fn language_name(lang: Lang, target: &str) -> &'static str {
    let key = format!("lang.{target}");
    let t = tables();
    let table = match lang {
        Lang::En => &t.en,
        Lang::Fr => &t.fr,
    };
    table.get(&key).or_else(|| t.en.get(&key)).map(|s| s.as_str()).unwrap_or("?")
}

/// Status language for a request's target: French for "fr", else English.
pub fn for_target(target: &str) -> Lang {
    if target == "fr" {
        Lang::Fr
    } else {
        Lang::En
    }
}

/// The Windows UI language, for strings with no request (tray, balloons, toasts before any request).
pub fn ui_lang() -> Lang {
    // LANG_FRENCH = 0x0C in the low 10 bits of the LANGID.
    let id = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
    if id & 0x3FF == 0x0C {
        Lang::Fr
    } else {
        Lang::En
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(json: &str) -> Vec<String> {
        let m: HashMap<String, String> = serde_json::from_str(json).expect("valid JSON");
        let mut k: Vec<String> = m.into_keys().collect();
        k.sort();
        k
    }

    #[test]
    fn en_and_fr_have_the_same_keys() {
        assert_eq!(keys(EN_JSON), keys(FR_JSON));
    }

    #[test]
    fn every_used_key_exists() {
        for k in USED_KEYS {
            assert!(tables().en.contains_key(*k), "en missing {k}");
            assert!(tables().fr.contains_key(*k), "fr missing {k}");
        }
        for t in TARGETS {
            assert_ne!(language_name(Lang::En, t), "?", "{t}");
            assert_ne!(language_name(Lang::Fr, t), "?", "{t}");
        }
    }

    #[test]
    fn design_strings() {
        assert_eq!(tr(Lang::En, "status.queued"), "Waiting ({n} ahead)...");
        assert_eq!(tr(Lang::En, "status.translating"), "Translating...");
        assert_eq!(tr(Lang::En, "status.failed"), "Translation failed - Shift-click [T] to retry");
        assert_eq!(tr(Lang::En, "status.missing"), "Not received - click [T] again");
        assert_eq!(tr_fmt(Lang::En, "status.queued", &[("n", "2")]), "Waiting (2 ahead)...");
        assert_eq!(tr_fmt(Lang::Fr, "status.same", &[("language", language_name(Lang::Fr, "fr"))]), "Déjà en français");
        assert_eq!(
            tr_fmt(Lang::En, "toast.bridge_ok", &[("mode", "dense"), ("m", "42"), ("capture", "WGC"), ("hz", "20")]),
            "TranslateProbe Helper: bridge OK (dense, margin 42, WGC 20 Hz)"
        );
        // the reply card's row 0 (design A.6.4) and the window wording (A.7)
        assert_eq!(tr(Lang::En, "reply.ready"), "Ready - Ctrl+V to paste");
        assert_eq!(tr(Lang::En, "reply.ready_cut"), "Ready - Ctrl+V to paste (shortened to 255 bytes)");
        assert_eq!(tr_fmt(Lang::En, "reply.same", &[("language", language_name(Lang::En, "de"))]), "Already in German - Ctrl+V to paste");
        assert_eq!(tr(Lang::En, "reply.not_copied"), "Ready (not copied)");
        assert_eq!(tr(Lang::En, "reply.off"), "Ready (not copied - off in the Helper menu)");
        assert_eq!(tr(Lang::En, "reply.changed"), "Not copied - you copied something else. Click Translate again");
        assert_eq!(tr(Lang::En, "reply.busy"), "Not copied - clipboard busy. Click Translate again");
        assert_eq!(tr(Lang::En, "reply.failed"), "Translation failed - click Translate again");
        assert_eq!(tr(Lang::En, "reply.missing"), "Not received - click Translate again");
        assert_eq!(tr(Lang::En, "window.missing"), "Not received - click the bubble");
        assert_eq!(tr(Lang::En, "window.failed"), "Translation failed - Shift-click the bubble");
        assert_eq!(tr(Lang::Fr, "reply.ready"), "Prêt - Ctrl+V pour coller");
        assert_eq!(tr(Lang::En, "tray.copy_replies"), "Copy translated replies to the clipboard");
    }

    #[test]
    fn no_forbidden_characters() {
        for json in [EN_JSON, FR_JSON] {
            assert!(!json.contains('\u{2014}'), "em dash");
            assert!(!json.contains('\u{2026}'), "ellipsis character");
        }
    }

    #[test]
    fn target_language() {
        assert_eq!(for_target("fr"), Lang::Fr);
        assert_eq!(for_target("en"), Lang::En);
        assert_eq!(for_target("de"), Lang::En);
        assert_eq!(tr(Lang::Fr, "no.such.key"), "?");
    }
}
