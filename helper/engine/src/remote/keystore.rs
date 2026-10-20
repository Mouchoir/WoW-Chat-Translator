//! API keys on disk: `keys.bin` next to settings.json (`%APPDATA%\TranslateProbe\keys.bin`),
//! encrypted with DPAPI for the current Windows user (`CryptProtectData`, an application
//! entropy, no UI). Written only by the companion itself (the user's own process).
//!
//! File: the 4 bytes `TPK1`, then the DPAPI blob of a JSON object
//! `{"version":2,"keys":{"deepl":"...","openai:api.openai.com/v1":"..."}}`, one key per service id
//! (`super::service_id`): a key saved for one OpenAI-compatible server is never sent to another.
//! The clear JSON exists only in memory and is zeroed after use (best effort). Keys are never in
//! settings.json and never logged.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::{Provider, Secret};

const MAGIC: &[u8; 4] = b"TPK1";
/// Extra DPAPI entropy. It is a public constant (this repository is open source), so it does not
/// keep out software running as the same Windows user: DPAPI protects the file against other
/// users, other computers and a copy of the file taken elsewhere, not against the user's own
/// programs.
const ENTROPY: &[u8] = b"TranslateProbe.keys.v1";

/// `keys.bin` in the folder of settings.json.
pub fn path_next_to(settings_json: &Path) -> PathBuf {
    settings_json.parent().map(|d| d.join("keys.bin")).unwrap_or_else(|| PathBuf::from("keys.bin"))
}

/// The keys the player entered, one per service id (`deepl`, `gemini`, `claude`,
/// `openai:<host[:port]><path>`). No Debug of the values.
#[derive(Clone, Default)]
pub struct Keys(BTreeMap<String, Secret>);

impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ids: Vec<&str> = self.0.keys().map(String::as_str).collect();
        write!(f, "Keys({ids:?})")
    }
}

/// A service id the key store accepts: a provider id other than `openai`, or `openai:` and a
/// server.
pub fn valid_slot(slot: &str) -> bool {
    match slot.strip_prefix("openai:") {
        Some(server) => !server.is_empty() && !server.chars().any(|c| c.is_control() || c.is_whitespace()),
        None => Provider::from_id(slot).is_some_and(|p| p != Provider::OpenAi),
    }
}

impl Keys {
    /// The key saved for a service id.
    pub fn get(&self, slot: &str) -> Option<&Secret> {
        self.0.get(slot).filter(|s| !s.is_empty())
    }

    /// An empty key (or an invalid service id) removes the entry.
    pub fn set(&mut self, slot: &str, key: Secret) {
        if key.is_empty() || !valid_slot(slot) {
            self.0.remove(slot);
        } else {
            self.0.insert(slot.to_string(), key);
        }
    }

    pub fn remove(&mut self, slot: &str) {
        self.0.remove(slot);
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The service ids that have a key.
    pub fn slots(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }
}

/// Zeroes every string of a parsed or built JSON value before it is dropped (best effort).
fn zero_strings(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::String(s) => {
            // SAFETY: zeros are valid UTF-8.
            unsafe { s.as_mut_vec() }.fill(0);
        }
        serde_json::Value::Array(a) => a.iter_mut().for_each(zero_strings),
        serde_json::Value::Object(o) => o.values_mut().for_each(zero_strings),
        _ => {}
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyStoreError {
    Io,
    /// Not a key file (bad magic) or not the expected JSON after decryption.
    Corrupt,
    /// DPAPI refused (another user or machine, or a damaged blob).
    Decrypt,
    Encrypt,
}

/// Reads the keys; a missing file is an empty set.
pub fn load(path: &Path) -> Result<Keys, KeyStoreError> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Keys::default()),
        Err(_) => return Err(KeyStoreError::Io),
    };
    let Some(blob) = bytes.strip_prefix(MAGIC.as_slice()) else { return Err(KeyStoreError::Corrupt) };
    let mut clear = dpapi::unprotect(blob, ENTROPY).ok_or(KeyStoreError::Decrypt)?;
    let parsed = parse(&clear);
    clear.fill(0);
    parsed
}

fn parse(clear: &[u8]) -> Result<Keys, KeyStoreError> {
    let mut v: serde_json::Value = serde_json::from_slice(clear).map_err(|_| KeyStoreError::Corrupt)?;
    let got = match v.get("keys").and_then(|k| k.as_object()) {
        Some(map) => {
            let mut keys = Keys::default();
            for (id, val) in map {
                // unknown or malformed ids (the provider-only "openai" of no released version)
                // are dropped
                if let Some(s) = val.as_str() {
                    keys.set(id, Secret::new(s));
                }
            }
            Ok(keys)
        }
        None => Err(KeyStoreError::Corrupt),
    };
    zero_strings(&mut v);
    got
}

/// Encrypts and writes the keys (a temporary file, then a rename). An empty set deletes the file.
pub fn save(path: &Path, keys: &Keys) -> Result<(), KeyStoreError> {
    if keys.is_empty() {
        return match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(KeyStoreError::Io),
        };
    }
    let mut obj = serde_json::Map::new();
    let mut room = 64;
    for (slot, s) in &keys.0 {
        obj.insert(slot.clone(), serde_json::Value::String(s.expose().to_string()));
        room += slot.len() + 6 * s.expose().len() + 8;
    }
    let mut doc = serde_json::json!({"version": 2, "keys": obj});
    // one allocation large enough for the worst escaping: no growth leaves a copy behind
    let mut clear = Vec::with_capacity(room);
    let written = serde_json::to_writer(&mut clear, &doc);
    zero_strings(&mut doc);
    drop(doc);
    if written.is_err() {
        clear.fill(0);
        return Err(KeyStoreError::Encrypt);
    }
    let blob = dpapi::protect(&clear, ENTROPY);
    clear.fill(0);
    let blob = blob.ok_or(KeyStoreError::Encrypt)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|_| KeyStoreError::Io)?;
    }
    let tmp = path.with_extension("bin.tmp");
    let mut data = Vec::with_capacity(MAGIC.len() + blob.len());
    data.extend_from_slice(MAGIC);
    data.extend_from_slice(&blob);
    std::fs::write(&tmp, &data).map_err(|_| KeyStoreError::Io)?;
    std::fs::rename(&tmp, path).map_err(|_| {
        let _ = std::fs::remove_file(&tmp);
        KeyStoreError::Io
    })
}

#[cfg(windows)]
mod dpapi {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB};

    fn blob(b: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB { cbData: b.len() as u32, pbData: b.as_ptr() as *mut u8 }
    }

    /// Copies DPAPI's output and frees it (zeroing it first when it holds clear text).
    fn take(out: CRYPT_INTEGER_BLOB, zero: bool) -> Vec<u8> {
        if out.pbData.is_null() {
            return Vec::new();
        }
        // SAFETY: DPAPI returned cbData bytes at pbData, allocated with LocalAlloc.
        let v = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
        if zero {
            // SAFETY: same buffer, still owned by us until LocalFree.
            unsafe { std::ptr::write_bytes(out.pbData, 0, out.cbData as usize) };
        }
        // SAFETY: the buffer came from DPAPI (LocalAlloc).
        unsafe { LocalFree(Some(HLOCAL(out.pbData as *mut _))) };
        v
    }

    pub fn protect(clear: &[u8], entropy: &[u8]) -> Option<Vec<u8>> {
        let input = blob(clear);
        let ent = blob(entropy);
        let mut out = CRYPT_INTEGER_BLOB::default();
        let desc: Vec<u16> = "TranslateProbe API keys\0".encode_utf16().collect();
        // SAFETY: valid input blobs and an out blob DPAPI fills.
        unsafe { CryptProtectData(&input, PCWSTR(desc.as_ptr()), Some(&ent), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out) }.ok()?;
        Some(take(out, false))
    }

    pub fn unprotect(sealed: &[u8], entropy: &[u8]) -> Option<Vec<u8>> {
        let input = blob(sealed);
        let ent = blob(entropy);
        let mut out = CRYPT_INTEGER_BLOB::default();
        // SAFETY: valid input blobs and an out blob DPAPI fills.
        unsafe { CryptUnprotectData(&input, None, Some(&ent), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out) }.ok()?;
        Some(take(out, true))
    }
}

#[cfg(not(windows))]
mod dpapi {
    pub fn protect(_: &[u8], _: &[u8]) -> Option<Vec<u8>> {
        None
    }
    pub fn unprotect(_: &[u8], _: &[u8]) -> Option<Vec<u8>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;

    const SENTINEL: &str = "sk-TPSENTINEL-0123456789abcdefKEY";

    fn contains(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn round_trip_on_a_temp_folder_and_no_clear_key_on_disk() {
        let dir = temp_dir("keys");
        let path = path_next_to(&dir.join("settings.json"));
        assert_eq!(path, dir.join("keys.bin"));
        assert!(load(&path).unwrap().is_empty(), "missing file: no keys");
        let mut k = Keys::default();
        k.set("claude", Secret::new(&format!("  {SENTINEL}\n")));
        k.set("deepl", Secret::new("abcd-1234:fx"));
        save(&path, &k).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..4], b"TPK1");
        assert!(!contains(&bytes, SENTINEL.as_bytes()), "the key is not in clear (UTF-8)");
        let wide: Vec<u8> = SENTINEL.encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert!(!contains(&bytes, &wide), "the key is not in clear (UTF-16)");
        assert!(!contains(&bytes, b"claude"), "provider ids are inside the blob too");
        let back = load(&path).unwrap();
        assert_eq!(back.get("claude").map(Secret::expose), Some(SENTINEL), "pasted whitespace trimmed");
        assert_eq!(back.get("deepl").map(Secret::expose), Some("abcd-1234:fx"));
        assert!(back.get("gemini").is_none());
        assert_eq!(format!("{back:?}"), "Keys([\"claude\", \"deepl\"])");
        assert!(!dir.join("keys.bin.tmp").exists());
        // removing every key deletes the file
        let mut none = back.clone();
        none.remove("claude");
        none.set("deepl", Secret::new(""));
        save(&path, &none).unwrap();
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn damaged_files_are_errors() {
        let dir = temp_dir("keys_bad");
        let path = dir.join("keys.bin");
        std::fs::write(&path, b"nope").unwrap();
        assert_eq!(load(&path).unwrap_err(), KeyStoreError::Corrupt);
        let mut k = Keys::default();
        k.set("gemini", Secret::new("AIzaSyTEST"));
        save(&path, &k).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        let n = bytes.len();
        bytes[n - 5] ^= 0x5A;
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(load(&path).unwrap_err(), KeyStoreError::Decrypt);
        // a blob sealed without our entropy is refused too
        let other = dpapi::protect(br#"{"version":1,"keys":{"gemini":"x"}}"#, b"other").unwrap();
        let mut data = MAGIC.to_vec();
        data.extend_from_slice(&other);
        std::fs::write(&path, &data).unwrap();
        assert_eq!(load(&path).unwrap_err(), KeyStoreError::Decrypt);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_providers_are_ignored() {
        let k = parse(br#"{"version":2,"keys":{"deepl":"a","nope":"b","claude":5,"openai":"c","openai:":"d","openai:api.openai.com/v1":"e"}}"#).unwrap();
        assert!(k.get("deepl").is_some());
        assert!(k.get("claude").is_none());
        assert!(k.get("openai").is_none(), "an OpenAI-compatible key belongs to one server");
        assert!(k.get("openai:").is_none());
        assert_eq!(k.get("openai:api.openai.com/v1").map(Secret::expose), Some("e"));
        assert_eq!(k.slots().collect::<Vec<_>>(), ["deepl", "openai:api.openai.com/v1"]);
        assert_eq!(parse(b"{}").unwrap_err(), KeyStoreError::Corrupt);
        assert!(valid_slot("gemini") && valid_slot("openai:localhost:11434/v1") && !valid_slot("openai") && !valid_slot("openai:a b"));
    }
}
