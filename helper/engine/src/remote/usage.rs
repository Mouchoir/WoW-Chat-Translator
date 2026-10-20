//! The daily character cap: characters of chat text and glossary terms sent to a remote service
//! per local day. Persisted as numbers only in `<data dir>\remote_usage.json`:
//! `{"version":1,"day":"2026-10-20","chars":1234}`. A new local day starts from 0.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const FILE: &str = "remote_usage.json";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Doc {
    version: u32,
    day: String,
    chars: u64,
}

/// Today's count, kept in memory and written on every change.
pub struct Usage {
    path: Option<PathBuf>,
    doc: Doc,
}

/// The local date as `YYYY-MM-DD`.
pub fn today() -> String {
    #[cfg(windows)]
    {
        // SAFETY: no arguments, returns a value.
        let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
        format!("{:04}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay)
    }
    #[cfg(not(windows))]
    {
        let days = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() / 86_400).unwrap_or(0);
        format!("day-{days}")
    }
}

impl Usage {
    /// Reads `<data_dir>\remote_usage.json` (missing or unreadable: 0 today). None: memory only.
    pub fn load(data_dir: Option<&Path>) -> Usage {
        let path = data_dir.map(|d| d.join(FILE));
        let doc = path.as_ref().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice::<Doc>(&b).ok()).unwrap_or_default();
        Usage { path, doc }
    }

    fn roll(&mut self, day: &str) {
        if self.doc.day != day {
            self.doc = Doc { version: 1, day: day.to_string(), chars: 0 };
        }
    }

    /// Characters sent on `day` so far.
    pub fn used(&mut self, day: &str) -> u64 {
        self.roll(day);
        self.doc.chars
    }

    /// True when `n` more characters stay within `cap` (0 = no cap).
    pub fn allows(&mut self, day: &str, cap: u32, n: u32) -> bool {
        cap == 0 || self.used(day) + u64::from(n) <= u64::from(cap)
    }

    /// Counts `n` characters sent on `day` and writes the file.
    pub fn add(&mut self, day: &str, n: u32) {
        self.roll(day);
        self.doc.chars += u64::from(n);
        self.doc.version = 1;
        if let Some(p) = &self.path {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(text) = serde_json::to_vec(&self.doc) {
                let _ = std::fs::write(p, text);
            }
        }
    }
}

/// Today's count as written by the engine (for the settings window).
pub fn read_today(data_dir: &Path) -> u64 {
    Usage::load(Some(data_dir)).used(&today())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;

    #[test]
    fn counts_persist_per_day_as_numbers() {
        let dir = temp_dir("usage");
        let mut u = Usage::load(Some(&dir));
        assert!(u.allows("2026-10-20", 100, 100));
        u.add("2026-10-20", 60);
        assert!(u.allows("2026-10-20", 100, 40));
        assert!(!u.allows("2026-10-20", 100, 41));
        assert!(u.allows("2026-10-20", 0, 1_000_000), "0 = no cap");
        let text = std::fs::read_to_string(dir.join(FILE)).unwrap();
        assert_eq!(text, r#"{"version":1,"day":"2026-10-20","chars":60}"#);
        // read back by another instance (an engine restart)
        let mut again = Usage::load(Some(&dir));
        assert_eq!(again.used("2026-10-20"), 60);
        // a new day starts from 0
        assert_eq!(again.used("2026-10-21"), 0);
        assert!(again.allows("2026-10-21", 100, 100));
        std::fs::write(dir.join(FILE), b"garbage").unwrap();
        assert_eq!(Usage::load(Some(&dir)).used("2026-10-20"), 0);
        assert_eq!(today().len(), 10);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
