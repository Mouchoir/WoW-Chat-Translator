//! In-memory LRU of 256 results; key = FNV-1a 64 of (PROMPT_VERSION, model digest, target, protected text, pairs).
//! Nothing is ever written to disk.

use std::collections::HashMap;

use crate::prompt::PROMPT_VERSION;

pub const CAPACITY: usize = 256;

#[derive(Clone)]
pub enum Cached {
    Done(String),
    Same,
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

struct Fnv(u64);

impl Fnv {
    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 ^= x as u64;
            self.0 = self.0.wrapping_mul(FNV_PRIME);
        }
    }
    /// Length-prefixed field, so ("ab","c") and ("a","bc") differ.
    fn field(&mut self, b: &[u8]) {
        self.bytes(&(b.len() as u32).to_le_bytes());
        self.bytes(b);
    }
}

pub fn fnv1a64(b: &[u8]) -> u64 {
    let mut h = Fnv(FNV_OFFSET);
    h.bytes(b);
    h.0
}

/// Cache key of one request.
pub fn key(model_digest: &str, target: &str, protected: &str, pairs: &[(String, String)]) -> u64 {
    let mut h = Fnv(FNV_OFFSET);
    h.bytes(&PROMPT_VERSION.to_le_bytes());
    h.field(model_digest.as_bytes());
    h.field(target.as_bytes());
    h.field(protected.as_bytes());
    h.bytes(&(pairs.len() as u32).to_le_bytes());
    for (t, m) in pairs {
        h.field(t.as_bytes());
        h.field(m.as_bytes());
    }
    h.0
}

pub struct Cache {
    map: HashMap<u64, (u64, Cached)>,
    tick: u64,
    cap: usize,
}

impl Cache {
    pub fn new(cap: usize) -> Cache {
        Cache { map: HashMap::new(), tick: 0, cap: cap.max(1) }
    }

    pub fn get(&mut self, k: u64) -> Option<Cached> {
        self.tick += 1;
        let t = self.tick;
        self.map.get_mut(&k).map(|e| {
            e.0 = t;
            e.1.clone()
        })
    }

    pub fn put(&mut self, k: u64, v: Cached) {
        self.tick += 1;
        if !self.map.contains_key(&k) && self.map.len() >= self.cap {
            if let Some((&oldest, _)) = self.map.iter().min_by_key(|(_, e)| e.0) {
                self.map.remove(&oldest);
            }
        }
        self.map.insert(k, (self.tick, v));
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    #[cfg(test)]
    pub fn clear(&mut self) {
        self.map.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_reference_values() {
        // Published FNV-1a 64 test vectors.
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn key_is_stable_and_field_sensitive() {
        let pairs = vec![("wts".to_string(), "vend".to_string())];
        let k = key("sha256:7485", "fr", "wts {1}", &pairs);
        // Stability: the same inputs always give the same key (a golden value guards against
        // accidental changes to the key layout; bump it only together with PROMPT_VERSION).
        assert_eq!(k, key("sha256:7485", "fr", "wts {1}", &pairs));
        assert_eq!(k, 0x3097_654c_7b69_b41a, "key layout changed: got {k:#x}");
        assert_ne!(k, key("sha256:7486", "fr", "wts {1}", &pairs));
        assert_ne!(k, key("sha256:7485", "de", "wts {1}", &pairs));
        assert_ne!(k, key("sha256:7485", "fr", "wts {2}", &pairs));
        assert_ne!(k, key("sha256:7485", "fr", "wts {1}", &[]));
        assert_ne!(
            key("d", "fr", "x", &[("ab".into(), "c".into())]),
            key("d", "fr", "x", &[("a".into(), "bc".into())])
        );
    }

    #[test]
    fn lru_eviction() {
        let mut c = Cache::new(3);
        c.put(1, Cached::Same);
        c.put(2, Cached::Done("b".into()));
        c.put(3, Cached::Same);
        assert!(c.get(1).is_some()); // 1 is now the most recent
        c.put(4, Cached::Same); // evicts 2
        assert!(c.get(2).is_none());
        assert!(c.get(1).is_some() && c.get(3).is_some() && c.get(4).is_some());
        assert_eq!(c.len(), 3);
        c.put(4, Cached::Done("d".into())); // overwrite does not evict
        assert_eq!(c.len(), 3);
        assert!(matches!(c.get(4), Some(Cached::Done(s)) if s == "d"));
        c.clear();
        assert_eq!(c.len(), 0);
    }
}
