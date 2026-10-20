//! The app's side of the remote translation services (2026-10-20, the player's own API keys;
//! the engine side is `tp_engine::remote`, docs\ENGINE.md section 13).
//!
//! - [`Choice`]: what the settings window's service list offers (the local translator, then the
//!   engine's providers) and their localized labels.
//! - [`load_keys`] and [`engine_remote`]: the key file (`keys.bin` next to settings.json, DPAPI,
//!   the engine's key store) and the engine's remote configuration built from the saved choice,
//!   its consent and its key (`RemoteConfig::from_settings`).
//! - [`tip_text`] and [`reason`]: the fallback badge's tooltip and the Test button's failure, from
//!   a `Fallback` (provider, kind, HTTP status, the service's sanitized message). Never logged.

use std::path::{Path, PathBuf};

use tp_engine::remote::keystore::{self, KeyStoreError, Keys};
use tp_engine::remote::{ConfigError, Fallback, FallbackKind, Provider, RemoteConfig, RemoteSettings};

use crate::badge::TipText;
use crate::i18n::{self, Lang};

/// One entry of the settings window's service list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Local,
    Remote(Provider),
}

impl Choice {
    /// The list, in its order (the local translator first: it is the default).
    pub const ALL: [Choice; 5] = [
        Choice::Local,
        Choice::Remote(Provider::DeepL),
        Choice::Remote(Provider::OpenAi),
        Choice::Remote(Provider::Gemini),
        Choice::Remote(Provider::Claude),
    ];

    /// The settings id (`RemoteSettings::provider`).
    pub fn id(self) -> &'static str {
        match self {
            Choice::Local => "local",
            Choice::Remote(p) => p.id(),
        }
    }

    /// The choice the settings name (anything unknown is local, as the engine reads it).
    pub fn of(s: &RemoteSettings) -> Choice {
        s.provider().map_or(Choice::Local, Choice::Remote)
    }

    pub fn provider(self) -> Option<Provider> {
        match self {
            Choice::Local => None,
            Choice::Remote(p) => Some(p),
        }
    }

    /// Index in [`Choice::ALL`].
    pub fn index(self) -> usize {
        Choice::ALL.iter().position(|c| *c == self).unwrap_or(0)
    }

    /// The service list's label.
    pub fn label(self, lang: Lang) -> &'static str {
        i18n::tr(lang, &format!("provider.{}", self.id()))
    }
}

/// The service's short name in sentences ("DeepL", "Claude", "OpenAI-compatible service").
pub fn provider_name(lang: Lang, p: Provider) -> &'static str {
    i18n::tr(lang, &format!("provider.name.{}", p.id()))
}

/// `keys.bin` next to settings.json.
pub fn keys_path(settings_json: &Path) -> PathBuf {
    keystore::path_next_to(settings_json)
}

/// The key store's error as a fixed name for the log.
pub fn key_error_name(e: KeyStoreError) -> &'static str {
    match e {
        KeyStoreError::Io => "io",
        KeyStoreError::Corrupt => "corrupt",
        KeyStoreError::Decrypt => "decrypt",
        KeyStoreError::Encrypt => "encrypt",
    }
}

/// Reads the keys (none without a settings path, or when the file is missing). A file that cannot
/// be read or decrypted gives no keys and the error's name.
pub fn load_keys(settings_json: Option<&Path>) -> (Keys, Option<&'static str>) {
    let Some(p) = settings_json else { return (Keys::default(), None) };
    match keystore::load(&keys_path(p)) {
        Ok(k) => (k, None),
        Err(e) => (Keys::default(), Some(key_error_name(e))),
    }
}

/// The engine's remote configuration for the saved choice: None for the local translator, and
/// None with the reason when the service cannot be used (no consent, no key, a bad URL...): the
/// engine then translates locally.
pub fn engine_remote(s: &RemoteSettings, keys: &Keys) -> (Option<RemoteConfig>, Option<ConfigError>) {
    match RemoteConfig::from_settings(s, keys) {
        Ok(c) => (c, None),
        Err(e) => (None, Some(e)),
    }
}

/// The settings window's message for a configuration error (a locale key).
pub fn config_error_key(e: ConfigError) -> &'static str {
    match e {
        ConfigError::NoConsent | ConfigError::Local => "settings.err_consent",
        ConfigError::NoKey => "settings.err_key",
        ConfigError::BadUrl | ConfigError::InsecureUrl => "settings.err_url",
        ConfigError::ModelRequired => "settings.err_model",
        ConfigError::BadModel => "settings.err_model_bad",
    }
}

/// The reason of a fallback in one line: "HTTP 401 - invalid x-api-key", "no answer in time",
/// "refused by the service - <message>"; for the daily cap, the cap that applied (the engine puts
/// it in the message; `cap`, the current setting, only when the message holds no number).
pub fn reason(f: &Fallback, lang: Lang, cap: u32) -> String {
    let mut parts: Vec<String> = Vec::new();
    match f.kind {
        FallbackKind::Http => {}
        FallbackKind::DailyCap => {
            let applied = f.message.trim().parse::<u32>().unwrap_or(cap);
            parts.push(i18n::tr_fmt(lang, "fallback.kind.daily_cap", &[("n", &applied.to_string())]));
        }
        k => parts.push(i18n::tr(lang, &format!("fallback.kind.{}", k.code())).to_string()),
    }
    if let Some(s) = f.status {
        parts.insert(0, i18n::tr_fmt(lang, "fallback.http", &[("status", &s.to_string())]));
    }
    let msg = f.message.trim();
    if !msg.is_empty() && f.kind != FallbackKind::DailyCap && !parts.iter().any(|p| p.eq_ignore_ascii_case(msg)) {
        parts.push(msg.to_string());
    }
    parts.join(" - ")
}

/// The badge's tooltip: that the line was translated locally because of the service's error, and
/// the error (or that the daily limit is reached, or that the line was kept on this PC).
pub fn tip_text(f: &Fallback, lang: Lang, cap: u32) -> TipText {
    let name = provider_name(lang, f.provider);
    let key = match f.kind {
        FallbackKind::DailyCap => "fallback.title_cap",
        FallbackKind::KeptLocal => "fallback.title_kept",
        _ => "fallback.title",
    };
    TipText { title: i18n::tr_fmt(lang, key, &[("provider", name)]), reason: reason(f, lang, cap) }
}

/// The language the Test button translates into: the player's latest target when it is not
/// English (the test sentence is English), else French for a French Windows and German otherwise.
pub fn test_target(lang: Lang, latest: Option<&str>) -> String {
    match latest {
        Some(t) if t != "en" && i18n::TARGETS.contains(&t) => t.to_string(),
        _ if lang == Lang::Fr => "fr".to_string(),
        _ => "de".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tp_engine::remote::Secret;

    fn fb(provider: Provider, kind: FallbackKind, status: Option<u16>, message: &str) -> Fallback {
        Fallback { provider, kind, status, message: message.into() }
    }

    #[test]
    fn choices_follow_the_engine_ids_and_have_labels() {
        assert_eq!(Choice::ALL.map(Choice::id), ["local", "deepl", "openai", "gemini", "claude"]);
        for c in Choice::ALL {
            assert_eq!(Choice::ALL[c.index()], c);
            for lang in [Lang::En, Lang::Fr] {
                assert_ne!(c.label(lang), "?", "{c:?}");
                if let Some(p) = c.provider() {
                    assert_ne!(provider_name(lang, p), "?");
                }
            }
        }
        let mut s = RemoteSettings::default();
        assert_eq!(Choice::of(&s), Choice::Local);
        s.provider = "claude".into();
        assert_eq!(Choice::of(&s), Choice::Remote(Provider::Claude));
        s.provider = "babelfish".into();
        assert_eq!(Choice::of(&s), Choice::Local);
        assert_eq!(Choice::Local.label(Lang::En), "Local translator (Qwen3, on this computer)");
        assert_eq!(Choice::Remote(Provider::OpenAi).label(Lang::Fr), "Compatible OpenAI (OpenAI, OpenRouter, Mistral, Groq...)");
    }

    #[test]
    fn tooltips_name_the_service_the_status_and_the_message() {
        let t = tip_text(&fb(Provider::Claude, FallbackKind::Http, Some(401), "invalid x-api-key"), Lang::En, 0);
        assert_eq!(t.title, "Translated locally after an error from Claude");
        assert_eq!(t.reason, "HTTP 401 - invalid x-api-key");
        let t = tip_text(&fb(Provider::DeepL, FallbackKind::Http, Some(456), ""), Lang::Fr, 0);
        assert_eq!((t.title.as_str(), t.reason.as_str()), ("Traduit localement après une erreur de DeepL", "HTTP 456"));
        let t = tip_text(&fb(Provider::Gemini, FallbackKind::Timeout, None, ""), Lang::En, 0);
        assert_eq!(t.reason, "no answer in time");
        let t = tip_text(&fb(Provider::Claude, FallbackKind::Refused, Some(200), "Refused by the service"), Lang::En, 0);
        assert_eq!(t.reason, "HTTP 200 - refused by the service", "the same words once");
        let t = tip_text(&fb(Provider::Gemini, FallbackKind::Refused, None, "blocked: SAFETY"), Lang::En, 0);
        assert_eq!(t.reason, "refused by the service - blocked: SAFETY");
        let t = tip_text(&fb(Provider::OpenAi, FallbackKind::Network, None, "could not connect"), Lang::En, 0);
        assert_eq!(t.title, "Translated locally after an error from OpenAI-compatible service");
        assert_eq!(t.reason, "the service could not be reached - could not connect");
        let t = tip_text(&fb(Provider::DeepL, FallbackKind::DailyCap, None, "ignored"), Lang::En, 50_000);
        assert_eq!(t.title, "Translated locally: the daily limit for DeepL is reached");
        assert_eq!(t.reason, "50000 characters per day");
        // the cap that applied (the engine's message), not the setting as it is now
        let t = tip_text(&fb(Provider::DeepL, FallbackKind::DailyCap, None, "20000"), Lang::En, 0);
        assert_eq!(t.reason, "20000 characters per day");
        let t = tip_text(&fb(Provider::Claude, FallbackKind::KeptLocal, None, ""), Lang::En, 0);
        assert_eq!(t.title, "Translated locally: not sent to Claude");
        assert_eq!(t.reason, "the message holds links, IDs or addresses that are never sent in clear");
        assert_eq!(tip_text(&fb(Provider::Claude, FallbackKind::KeptLocal, None, ""), Lang::Fr, 0).title, "Traduit localement : non envoyé à Claude");
        for k in [FallbackKind::Truncated, FallbackKind::BadAnswer, FallbackKind::KeptLocal] {
            for lang in [Lang::En, Lang::Fr] {
                assert!(!reason(&fb(Provider::Claude, k, None, ""), lang, 0).contains('?'), "{k:?} {lang:?}");
            }
        }
    }

    #[test]
    fn configuration_errors_have_messages() {
        for e in [ConfigError::Local, ConfigError::NoConsent, ConfigError::NoKey, ConfigError::BadUrl, ConfigError::InsecureUrl, ConfigError::ModelRequired, ConfigError::BadModel] {
            for lang in [Lang::En, Lang::Fr] {
                assert_ne!(i18n::tr(lang, config_error_key(e)), "?", "{e:?}");
            }
        }
    }

    #[test]
    fn the_engine_gets_a_service_only_with_consent_and_key() {
        let mut s = RemoteSettings { provider: "deepl".into(), ..RemoteSettings::default() };
        let mut keys = Keys::default();
        assert!(matches!(engine_remote(&s, &keys), (None, Some(ConfigError::NoConsent))));
        s.give_consent();
        assert!(matches!(engine_remote(&s, &keys), (None, Some(ConfigError::NoKey))));
        keys.set("deepl", Secret::new("abc:fx"));
        let (cfg, err) = engine_remote(&s, &keys);
        assert!(err.is_none());
        assert_eq!(cfg.map(|c| c.provider), Some(Provider::DeepL));
        assert!(matches!(engine_remote(&RemoteSettings::default(), &keys), (None, None)), "local");
    }

    #[test]
    fn keys_load_from_a_temp_folder_only() {
        assert!(load_keys(None).0.is_empty());
        let dir = std::env::temp_dir().join(format!("tp_app_keys_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let settings = dir.join("settings.json");
        assert_eq!(keys_path(&settings), dir.join("keys.bin"));
        let (k, err) = load_keys(Some(&settings));
        assert!(k.is_empty() && err.is_none(), "missing file: no keys, no error");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("keys.bin"), b"nope").unwrap();
        assert_eq!(load_keys(Some(&settings)).1, Some("corrupt"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_test_translates_into_a_language_other_than_english() {
        assert_eq!(test_target(Lang::En, Some("es")), "es");
        assert_eq!(test_target(Lang::En, Some("en")), "de");
        assert_eq!(test_target(Lang::Fr, None), "fr");
        assert_eq!(test_target(Lang::En, Some("xx")), "de");
    }
}
