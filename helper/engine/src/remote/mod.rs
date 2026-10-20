//! Remote translation services with the player's own API key (2026-10-20, docs/ENGINE.md section
//! 13): DeepL, any OpenAI-compatible endpoint, Google Gemini and Anthropic Claude. The local
//! model stays the default; a job sent to a service that fails (network, quota, invalid key,
//! refusal, timeout, daily cap) is translated locally and its result carries a [`Fallback`].
//!
//! Privacy: a request carries only the protected message text (placeholders for items, IDs and
//! URLs) and the glossary pairs; never a sender name, a channel or a realm. Keys live in
//! [`keystore`] (DPAPI) and in [`Secret`] (no Debug, no Display, zeroed on drop). Log events
//! carry the provider, a model label, HTTP status, latency and character counts only.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::pipeline::Prepared;
use crate::prompt::Variant;

mod claude;
mod deepl;
mod gemini;
pub mod http;
pub mod keystore;
mod openai;
pub mod usage;
#[cfg(windows)]
pub mod winhttp;

#[cfg(test)]
pub(crate) mod fake;
#[cfg(test)]
mod tests;

pub use http::{HttpRequest, HttpResponse, Timeouts, Transport, TransportError, Url};

/// A remote translation service.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Provider {
    DeepL,
    /// Any server speaking OpenAI's `/chat/completions` (OpenAI, OpenRouter, Mistral, Groq,
    /// DeepSeek, LM Studio, Ollama...).
    OpenAi,
    Gemini,
    Claude,
}

impl Provider {
    pub const ALL: [Provider; 4] = [Provider::DeepL, Provider::OpenAi, Provider::Gemini, Provider::Claude];

    /// Settings, key store and log id.
    pub fn id(self) -> &'static str {
        match self {
            Provider::DeepL => "deepl",
            Provider::OpenAi => "openai",
            Provider::Gemini => "gemini",
            Provider::Claude => "claude",
        }
    }

    pub fn from_id(s: &str) -> Option<Provider> {
        Provider::ALL.into_iter().find(|p| p.id() == s)
    }

    /// The service's name as shown in the companion (a product name, the same in every
    /// language; the OpenAI-compatible label is the app's to localize).
    pub fn display_name(self) -> &'static str {
        match self {
            Provider::DeepL => "DeepL",
            Provider::OpenAi => "OpenAI-compatible",
            Provider::Gemini => "Google Gemini",
            Provider::Claude => "Anthropic Claude",
        }
    }

    /// The model used when the field is empty. DeepL: none (the service's default model);
    /// OpenAI-compatible: only for the OpenAI host (another server needs a model name).
    pub fn default_model(self) -> &'static str {
        match self {
            Provider::DeepL => "",
            Provider::OpenAi => "gpt-6-luna",
            Provider::Gemini => "gemini-3.5-flash-lite",
            Provider::Claude => "claude-opus-5-5",
        }
    }

    /// Suggestions for the model combo (the first one is the default; DeepL's are its
    /// `model_type` values).
    pub fn model_suggestions(self) -> &'static [&'static str] {
        match self {
            Provider::DeepL => &["latency_optimized", "quality_optimized", "prefer_quality_optimized"],
            Provider::OpenAi => &["gpt-6-luna", "gpt-6.1-sol", "gpt-6-astra"],
            Provider::Gemini => &["gemini-3.5-flash-lite", "gemini-3.8-flash", "gemini-3.1-flash-lite"],
            Provider::Claude => &["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"],
        }
    }

    /// Only the OpenAI-compatible provider has a base URL field.
    pub fn uses_base_url(self) -> bool {
        self == Provider::OpenAi
    }
}

/// `https://api.openai.com/v1`, the OpenAI-compatible base URL when the field is empty.
pub const OPENAI_DEFAULT_BASE: &str = "https://api.openai.com/v1";

/// The model as written in log events: a known suggestion or `"custom"` (events carry
/// `&'static str` only).
pub fn model_label(provider: Provider, model: &str) -> &'static str {
    provider.model_suggestions().iter().copied().find(|m| *m == model).unwrap_or(if model.is_empty() { "default" } else { "custom" })
}

/// An API key. No Debug, Display or Serialize; the bytes are zeroed when it is dropped.
pub struct Secret(String);

impl Secret {
    /// Leading and trailing whitespace (a pasted key) is removed.
    pub fn new(s: &str) -> Secret {
        Secret(s.trim().to_string())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Clone for Secret {
    fn clone(&self) -> Self {
        Secret(self.0.clone())
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        // SAFETY: zeros are valid UTF-8.
        unsafe { self.0.as_mut_vec() }.fill(0);
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(..)")
    }
}

/// The translation engine choice in settings.json (embedded by the app; numbers, ids and a URL
/// only: the key lives in the key store). camelCase like the rest of the file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RemoteSettings {
    /// `local` (default), `deepl`, `openai`, `gemini` or `claude`.
    pub provider: String,
    /// Empty: the provider's default model.
    pub model: String,
    /// OpenAI-compatible only; empty: `OPENAI_DEFAULT_BASE`.
    pub base_url: String,
    /// The service the player agreed to send chat text and glossary terms to
    /// ([`RemoteSettings::service_id`] at the time of the consent); empty: no consent. Another
    /// provider or another OpenAI-compatible host needs a new consent.
    pub consent: String,
    /// Characters of chat text and glossary terms sent per local day; 0 = no cap.
    pub daily_char_cap: u32,
}

impl Default for RemoteSettings {
    fn default() -> Self {
        RemoteSettings { provider: "local".into(), model: String::new(), base_url: String::new(), consent: String::new(), daily_char_cap: 0 }
    }
}

impl RemoteSettings {
    /// None for `local` (or anything unknown).
    pub fn provider(&self) -> Option<Provider> {
        Provider::from_id(&self.provider)
    }

    /// What a consent (and a saved key) is given to: see [`service_id`]. Empty for local or an
    /// invalid base URL.
    pub fn service_id(&self) -> String {
        self.provider().map(|p| service_id(p, &self.base_url)).unwrap_or_default()
    }

    pub fn has_consent(&self) -> bool {
        let id = self.service_id();
        !id.is_empty() && self.consent == id
    }

    /// Records the consent for the current provider (and host).
    pub fn give_consent(&mut self) {
        self.consent = self.service_id();
    }

    /// Replaces invalid values by their defaults; returns the names of the fields it fixed.
    pub fn validate(&mut self) -> Vec<&'static str> {
        let mut fixed = Vec::new();
        if self.provider != "local" && self.provider().is_none() {
            self.provider = "local".into();
            fixed.push("provider");
        }
        if self.model.chars().count() > 128 || self.model.chars().any(|c| c.is_control()) {
            self.model.clear();
            fixed.push("model");
        }
        if !self.base_url.trim().is_empty() && base_url(&self.base_url).is_none() {
            self.base_url.clear();
            fixed.push("baseUrl");
        }
        fixed
    }
}

/// The OpenAI-compatible base URL (empty: the default).
fn base_url(s: &str) -> Option<Url> {
    Url::parse(if s.trim().is_empty() { OPENAI_DEFAULT_BASE } else { s })
}

/// What a consent and a saved key belong to: `deepl`, `gemini`, `claude`, or for an
/// OpenAI-compatible server `openai:<host[:port]><base path>` (`openai:api.openai.com/v1`). Another
/// provider, server or route behind the same host asks for the consent again, and a saved key is
/// only ever sent to the server it was saved for (the key store is keyed by this id). Empty for
/// an invalid base URL (`base` empty: the OpenAI default).
pub fn service_id(provider: Provider, base: &str) -> String {
    match provider {
        Provider::OpenAi => base_url(base).map(|u| openai_id(&u)).unwrap_or_default(),
        p => p.id().to_string(),
    }
}

/// `openai:` + the authority + the base path (no query, no trailing `/`, no
/// `/chat/completions`), the same for a base URL and for the endpoint made from it.
fn openai_id(u: &Url) -> String {
    let path = u.path.split('?').next().unwrap_or("").trim_end_matches('/');
    let path = path.strip_suffix("/chat/completions").unwrap_or(path).trim_end_matches('/');
    format!("openai:{}{}", u.authority(), path)
}

/// Whether a service needs an API key: every one but an OpenAI-compatible server on this PC or on
/// a private network address (Ollama or LM Studio on the PC or a NAS). A key never travels over
/// plain http to another computer (`ConfigError::InsecureUrl`).
pub fn key_needed(provider: Provider, base: &str) -> bool {
    match provider {
        Provider::OpenAi => !base_url(base).is_some_and(|u| keyless_host(&u)),
        _ => true,
    }
}

fn keyless_host(u: &Url) -> bool {
    u.is_loopback() || u.is_private_lan()
}

/// Why a provider cannot be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// The settings name `local` (not an error for `from_settings`, which returns None).
    Local,
    /// The player has not agreed to send chat text to this service.
    NoConsent,
    /// No API key (only an OpenAI-compatible server on this PC or a private network address works
    /// without one).
    NoKey,
    /// The base URL is not an http(s) URL.
    BadUrl,
    /// Plain `http://` to a host that is not this PC, with a key: refused (the key would travel
    /// in clear).
    InsecureUrl,
    /// An OpenAI-compatible server other than OpenAI's needs a model name.
    ModelRequired,
    /// The model name holds characters a URL path or a JSON id cannot carry.
    BadModel,
}

impl ConfigError {
    /// Stable code for the app's locale table.
    pub fn code(self) -> &'static str {
        match self {
            ConfigError::Local => "local",
            ConfigError::NoConsent => "no_consent",
            ConfigError::NoKey => "no_key",
            ConfigError::BadUrl => "bad_url",
            ConfigError::InsecureUrl => "insecure_url",
            ConfigError::ModelRequired => "model_required",
            ConfigError::BadModel => "bad_model",
        }
    }
}

/// A usable remote service: what the engine (`EngineHandle::set_remote`) and the settings
/// window's Test button need. No Debug (it holds the key).
#[derive(Clone)]
pub struct RemoteConfig {
    pub provider: Provider,
    /// The model sent (the default applied). DeepL: its `model_type`, may be empty.
    pub model: String,
    /// OpenAI-compatible: the `/chat/completions` URL. Others: the provider's fixed endpoint.
    pub endpoint: Url,
    pub key: Secret,
    /// Characters per local day, 0 = no cap.
    pub daily_char_cap: u32,
}

impl RemoteConfig {
    /// Checks one provider's fields (the consent is the caller's: see `from_settings`).
    pub fn new(provider: Provider, model: &str, base: &str, key: Secret, daily_char_cap: u32) -> Result<RemoteConfig, ConfigError> {
        let model = model.trim();
        if model.chars().any(|c| c.is_control() || c.is_whitespace()) || model.chars().count() > 128 {
            return Err(ConfigError::BadModel);
        }
        let endpoint = match provider {
            Provider::DeepL => {
                // Free keys end in ":fx" (api-free.deepl.com); paid keys use api.deepl.com.
                let host = if key.expose().ends_with(":fx") { "https://api-free.deepl.com" } else { "https://api.deepl.com" };
                Url::parse(host).expect("fixed url").join("v2/translate")
            }
            Provider::OpenAi => {
                let mut u = base_url(base).ok_or(ConfigError::BadUrl)?;
                if !u.https && !u.is_loopback() && !key.is_empty() {
                    return Err(ConfigError::InsecureUrl);
                }
                if !u.path.trim_end_matches('/').ends_with("/chat/completions") {
                    u = u.join("chat/completions");
                }
                u
            }
            Provider::Gemini => {
                let m = if model.is_empty() { provider.default_model() } else { model };
                let m = m.strip_prefix("models/").unwrap_or(m);
                if !m.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')) {
                    return Err(ConfigError::BadModel);
                }
                Url::parse("https://generativelanguage.googleapis.com").expect("fixed url").join(&format!("v1beta/models/{m}:generateContent"))
            }
            Provider::Claude => Url::parse("https://api.anthropic.com").expect("fixed url").join("v1/messages"),
        };
        if key.is_empty() && !(provider == Provider::OpenAi && keyless_host(&endpoint)) {
            return Err(ConfigError::NoKey);
        }
        let model = if !model.is_empty() {
            model.strip_prefix("models/").filter(|_| provider == Provider::Gemini).unwrap_or(model).to_string()
        } else if provider == Provider::OpenAi && endpoint.host != "api.openai.com" {
            return Err(ConfigError::ModelRequired);
        } else {
            provider.default_model().to_string()
        };
        Ok(RemoteConfig { provider, model, endpoint, key, daily_char_cap })
    }

    /// The service the settings name, when it has the player's consent and a key: `Ok(None)` for
    /// `local`. The key is the one saved for this very service ([`service_id`]): a key saved for
    /// one OpenAI-compatible server is never sent to another.
    pub fn from_settings(s: &RemoteSettings, keys: &keystore::Keys) -> Result<Option<RemoteConfig>, ConfigError> {
        let Some(p) = s.provider() else { return Ok(None) };
        if !s.has_consent() {
            return Err(ConfigError::NoConsent);
        }
        let key = keys.get(&s.service_id()).cloned().unwrap_or_else(|| Secret::new(""));
        RemoteConfig::new(p, &s.model, &s.base_url, key, s.daily_char_cap).map(Some)
    }

    /// The consent and key id of this service ([`service_id`]).
    pub fn service_id(&self) -> String {
        match self.provider {
            Provider::OpenAi => openai_id(&self.endpoint),
            p => p.id().to_string(),
        }
    }

    /// Part of the cache key: provider, server and route (OpenAI-compatible) and model, so a
    /// result of one service is never served for another (docs/ENGINE.md section 5).
    pub fn identity(&self) -> String {
        format!("remote:{}:{}", self.service_id(), self.model)
    }

    pub fn model_label(&self) -> &'static str {
        model_label(self.provider, &self.model)
    }
}

/// Why a job that was sent to a service was translated locally instead. Shown by the badge
/// tooltip; never logged (its Debug prints the provider, kind and status only).
#[derive(Clone)]
pub struct Fallback {
    pub provider: Provider,
    pub kind: FallbackKind,
    /// The HTTP status when the service answered with an error.
    pub status: Option<u16>,
    /// The service's own short message (English as the service wrote it), sanitized: no API key,
    /// no long token, no control character, at most `MESSAGE_MAX` characters. May be empty. For
    /// `FallbackKind::DailyCap`: the cap in force when the line was kept local, in characters
    /// (decimal digits), so an old badge keeps showing the limit that applied.
    pub message: String,
}

impl std::fmt::Debug for Fallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Fallback {{ provider: {:?}, kind: {:?}, status: {:?} }}", self.provider, self.kind, self.status)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FallbackKind {
    /// The service answered with an error status (`status` holds it): invalid key, quota,
    /// unknown model, rate limit after the retry, server error...
    Http,
    /// No complete answer within the time budget (15 s).
    Timeout,
    /// Name resolution, connection or TLS failure.
    Network,
    /// The service declined to translate (Claude `refusal`, Gemini safety block, OpenAI refusal
    /// or content filter).
    Refused,
    /// The answer was cut by the service's output limit.
    Truncated,
    /// An answer the engine cannot use (no text, broken placeholders after the retry).
    BadAnswer,
    /// The player's daily character cap is reached: nothing was sent.
    DailyCap,
    /// Privacy: the line holds more links, IDs or addresses than placeholders can hide (more than
    /// `protect::MAX_SPANS`), or the service's answer lost the placeholders of a line holding a
    /// URL, an e-mail address or an ID (the follow-up would carry them in clear). Nothing more was
    /// sent.
    KeptLocal,
}

impl FallbackKind {
    /// Stable code for the app's locale table and the log.
    pub fn code(self) -> &'static str {
        match self {
            FallbackKind::Http => "http",
            FallbackKind::Timeout => "timeout",
            FallbackKind::Network => "network",
            FallbackKind::Refused => "refused",
            FallbackKind::Truncated => "truncated",
            FallbackKind::BadAnswer => "bad_answer",
            FallbackKind::DailyCap => "daily_cap",
            FallbackKind::KeptLocal => "kept_local",
        }
    }
}

/// The longest service message kept in a [`Fallback`].
pub const MESSAGE_MAX: usize = 160;

/// One remote request, built from a prepared job.
pub struct Message<'a> {
    pub target: &'a str,
    /// System message (LLM providers).
    pub system: &'a str,
    /// User message: glossary and `Message:` text (LLM providers).
    pub user: &'a str,
    /// The protected message text (DeepL).
    pub text: &'a str,
    pub variant: Variant,
    /// The glossary pairs the prompt lists (DeepL sends them as context).
    pub pairs: &'a [(String, String)],
}

impl<'a> Message<'a> {
    pub fn of(p: &'a Prepared) -> Message<'a> {
        Message { target: &p.target, system: &p.system, user: &p.user, text: &p.protected.text, variant: p.protected.variant, pairs: &p.listed }
    }

    /// What the daily cap counts: the characters of the message text and of the glossary pairs
    /// sent (the fixed instructions are not counted).
    pub fn counted_chars(&self) -> u32 {
        let pairs: usize = self.pairs.iter().map(|(t, m)| t.chars().count() + m.chars().count()).sum();
        (self.text.chars().count() + pairs).min(u32::MAX as usize) as u32
    }
}

/// An owned copy of a [`Message`] for the request thread. No Debug: it holds chat text.
pub struct OwnedMessage {
    target: String,
    system: String,
    user: String,
    text: String,
    variant: Variant,
    pairs: Vec<(String, String)>,
}

impl OwnedMessage {
    pub fn of(p: &Prepared) -> OwnedMessage {
        OwnedMessage { target: p.target.clone(), system: p.system.clone(), user: p.user.clone(), text: p.protected.text.clone(), variant: p.protected.variant, pairs: p.listed.clone() }
    }

    pub fn message(&self) -> Message<'_> {
        Message { target: &self.target, system: &self.system, user: &self.user, text: &self.text, variant: self.variant, pairs: &self.pairs }
    }
}

/// A provider's verdict on one HTTP answer.
pub(crate) enum Parsed {
    Text(String),
    Fail(Failure),
}

/// A failed attempt (before it becomes a [`Fallback`]).
pub(crate) struct Failure {
    pub kind: FallbackKind,
    pub status: Option<u16>,
    /// Raw service message (sanitized later).
    pub message: String,
    pub retryable: bool,
    pub retry_after: Option<Duration>,
}

impl Failure {
    pub fn new(kind: FallbackKind, message: &str) -> Failure {
        Failure { kind, status: None, message: message.to_string(), retryable: false, retry_after: None }
    }

    /// An error status: the message from the JSON body, retryable for 429, 5xx and 529.
    pub fn http(status: u16, body: &[u8], retry_after: Option<Duration>) -> Failure {
        Failure { kind: FallbackKind::Http, status: Some(status), message: error_message(body), retryable: retryable_status(status), retry_after }
    }
}

/// 408, 429, 500, 502, 503, 504 and 529 are worth one quick retry.
pub fn retryable_status(status: u16) -> bool {
    matches!(status, 408 | 429 | 500 | 502 | 503 | 504 | 529)
}

/// The service's error message from a JSON error body: `error.message` (OpenAI, Gemini, Claude,
/// DeepL infrastructure), `message` (DeepL, Mistral), `error` as a string (Ollama), `detail`.
/// Empty when the body is not JSON (an HTML error page is never shown).
pub(crate) fn error_message(body: &[u8]) -> String {
    let Ok(v) = serde_json::from_slice::<Value>(body) else { return String::new() };
    for pick in [v.pointer("/error/message"), v.get("message"), v.get("error"), v.get("detail"), v.pointer("/0/error/message")].into_iter().flatten() {
        if let Some(s) = pick.as_str() {
            return s.to_string();
        }
    }
    String::new()
}

/// A service message made safe to show: control characters become spaces, whitespace is
/// collapsed, any word holding a piece of the key (6 characters or more), a masked key (`*`) or
/// a long token (24 characters or more of letters, digits, `-` and `_`) becomes `[redacted]`,
/// and the result is cut to `MESSAGE_MAX` characters (with "...").
pub fn sanitize_message(raw: &str, key: &str) -> String {
    sanitize_message_keeping(raw, key, "")
}

/// [`sanitize_message`], where two kinds of long words are not taken for tokens: the configured
/// `model` name (a 404 names it) and an all-caps enum name (`MISSING_THOUGHT_SIGNATURE`). A word
/// holding a piece of the key is redacted all the same.
pub fn sanitize_message_keeping(raw: &str, key: &str, model: &str) -> String {
    let key = key.trim();
    let words: Vec<String> = raw
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|w| !w.is_empty())
        .map(|w| if secret_like(w, key, model) { "[redacted]".to_string() } else { w.to_string() })
        .collect();
    let s = words.join(" ");
    if s.chars().count() <= MESSAGE_MAX {
        return s;
    }
    let cut: String = s.chars().take(MESSAGE_MAX - 3).collect();
    format!("{}...", cut.trim_end())
}

/// An all-caps enum name: `A_B` or longer, letters and underscores only.
fn enum_name(w: &str) -> bool {
    w.contains('_') && !w.starts_with('_') && !w.ends_with('_') && w.bytes().all(|b| b.is_ascii_uppercase() || b == b'_')
}

fn secret_like(word: &str, key: &str, model: &str) -> bool {
    if word.contains("**") || word.contains("...") && word.len() > 12 {
        return true;
    }
    let core = word.trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | '.' | ',' | ':' | ';' | '(' | ')' | '[' | ']'));
    let core_model = core.strip_prefix("models/").unwrap_or(core);
    let named = enum_name(core) || (!model.is_empty() && core_model == model.strip_prefix("models/").unwrap_or(model));
    let run = word.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_')).map(str::len).max().unwrap_or(0);
    if run >= 24 && !named {
        return true;
    }
    let lw = word.to_ascii_lowercase();
    if lw.starts_with("sk-") || lw.contains("sk-ant-") || lw.starts_with("aiza") && word.len() > 12 {
        return true;
    }
    // Any 6-character piece of the key.
    if key.len() >= 6 && word.len() >= 6 {
        let kb = key.as_bytes();
        let wb = word.as_bytes();
        for w in kb.windows(6) {
            if wb.windows(6).any(|x| x == w) {
                return true;
            }
        }
    }
    false
}

/// Time limits of one remote job (the lead's numbers: connect 5 s, total 15 s; one quick retry
/// for 429/5xx/529 inside the total).
#[derive(Clone, Copy, Debug)]
pub struct Policy {
    pub connect: Duration,
    pub total: Duration,
    /// Wait before the retry when the service gives no `retry-after`.
    pub retry_wait: Duration,
    /// The retry is skipped when the wait would leave less than this for the second request.
    pub retry_floor: Duration,
}

impl Default for Policy {
    fn default() -> Self {
        Policy { connect: Duration::from_secs(5), total: Duration::from_secs(15), retry_wait: Duration::from_millis(500), retry_floor: Duration::from_secs(3) }
    }
}

/// A successful call.
pub struct Answer {
    /// The raw translation text (before `pipeline::finish`). No Debug.
    pub text: String,
    pub status: u16,
    pub ms: u64,
    /// 1, or 2 after a retry.
    pub attempts: u8,
}

impl std::fmt::Debug for Answer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Answer {{ status: {}, ms: {}, attempts: {}, chars: {} }}", self.status, self.ms, self.attempts, self.text.chars().count())
    }
}

/// A failed call: the fallback reason and numbers for the log.
#[derive(Debug)]
pub struct CallError {
    pub fallback: Fallback,
    pub ms: u64,
    pub attempts: u8,
}

fn request_for(cfg: &RemoteConfig, msg: &Message) -> HttpRequest {
    match cfg.provider {
        Provider::DeepL => deepl::request(cfg, msg),
        Provider::OpenAi => openai::request(cfg, msg),
        Provider::Gemini => gemini::request(cfg, msg),
        Provider::Claude => claude::request(cfg, msg),
    }
}

fn parse_for(cfg: &RemoteConfig, resp: &HttpResponse) -> Parsed {
    if !(200..300).contains(&resp.status) {
        let mut f = Failure::http(resp.status, &resp.body, resp.retry_after);
        if cfg.provider == Provider::OpenAi && resp.status == 429 && openai::is_quota(&resp.body) {
            // "insufficient_quota": no retry will help.
            f.retryable = false;
        }
        return Parsed::Fail(f);
    }
    match cfg.provider {
        Provider::DeepL => deepl::parse(&resp.body),
        Provider::OpenAi => openai::parse(&resp.body),
        Provider::Gemini => gemini::parse(&resp.body),
        Provider::Claude => claude::parse(&resp.body),
    }
}

fn transport_failure(e: TransportError) -> Failure {
    match e {
        TransportError::Timeout => Failure::new(FallbackKind::Timeout, ""),
        TransportError::Connect => Failure { retryable: false, ..Failure::new(FallbackKind::Network, "could not connect") },
        TransportError::Tls => Failure::new(FallbackKind::Network, "secure connection failed"),
        TransportError::Other => Failure::new(FallbackKind::Network, "request failed"),
    }
}

/// Sends one job to the service: one request, and one quick retry for a retryable status when
/// the budget allows it. Blocking (the worker runs it on its own thread).
pub fn call(cfg: &RemoteConfig, msg: &Message, transport: &dyn Transport, policy: Policy) -> Result<Answer, CallError> {
    let start = Instant::now();
    let req = request_for(cfg, msg);
    let mut attempts = 0u8;
    let fail = loop {
        attempts += 1;
        let left = policy.total.saturating_sub(start.elapsed());
        let t = Timeouts { connect: policy.connect.min(left), total: left };
        let f = match transport.post(&req, t) {
            Ok(resp) => match parse_for(cfg, &resp) {
                Parsed::Text(text) => {
                    return Ok(Answer { text, status: resp.status, ms: start.elapsed().as_millis() as u64, attempts });
                }
                Parsed::Fail(f) => f,
            },
            Err(e) => transport_failure(e),
        };
        if attempts == 1 && f.retryable {
            let wait = f.retry_after.unwrap_or(policy.retry_wait);
            let left = policy.total.saturating_sub(start.elapsed());
            if wait + policy.retry_floor <= left {
                std::thread::sleep(wait);
                continue;
            }
        }
        break f;
    };
    let fallback = Fallback { provider: cfg.provider, kind: fail.kind, status: fail.status, message: sanitize_message_keeping(&fail.message, cfg.key.expose(), &cfg.model) };
    Err(CallError { fallback, ms: start.elapsed().as_millis() as u64, attempts })
}

/// The settings window's Test button: translates one synthetic sentence into `target` with the
/// shared pre- and post-processing. Ok holds the translation; Err the reason (as the badge
/// would show it). Blocking for up to the policy's total time: run it off the UI thread.
pub fn test_translation(cfg: &RemoteConfig, target: &str, transport: &dyn Transport) -> Result<String, Fallback> {
    let crate::pipeline::Prep::Ready(p) = crate::pipeline::prepare(target, TEST_SENTENCE, &[], crate::prompt::DEFAULT) else {
        unreachable!("the test sentence has letters")
    };
    let a = call(cfg, &Message::of(&p), transport, Policy::default()).map_err(|e| e.fallback)?;
    match crate::pipeline::finish(&p, &a.text) {
        crate::pipeline::Finish::Done(s) => Ok(tp_bridge::marks::plain(&s)),
        // An echo is a working answer too (target English).
        crate::pipeline::Finish::Same => Ok(TEST_SENTENCE.to_string()),
        _ => Err(Fallback { provider: cfg.provider, kind: FallbackKind::BadAnswer, status: None, message: String::new() }),
    }
}

/// The synthetic sentence of the Test button (no player data).
pub const TEST_SENTENCE: &str = "Hello! Anyone up for a dungeon run tonight? We need a healer.";

/// The real transport (WinHTTP, system proxy settings, TLS by the OS).
#[cfg(windows)]
pub fn system_transport() -> Arc<dyn Transport> {
    Arc::new(winhttp::WinHttp::new())
}
