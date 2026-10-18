//! Message reassembly from strip frames.
//!
//! Messages are keyed by (session, msgId). Chunks of one message must agree on count, total and
//! msgCrc. A completed message must match its total length and CRC-32, then goes through
//! [`msg::parse_message`]. A done-set of 512 ids drops repeats, partial messages expire after
//! 5 s, and a new session clears all state.

use std::collections::{HashMap, VecDeque};

use crate::{crc, msg, strip};

/// Counters only (no text), for the log and `--diag-json`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReasmStats {
    /// Frames passed to `push`.
    pub frames: u64,
    /// CHUNK records seen.
    pub chunks: u64,
    /// Chunks of a message that was already delivered.
    pub duplicates: u64,
    /// Chunks whose count, total or msgCrc disagreed with earlier chunks (the partial restarts).
    pub conflicts: u64,
    /// Messages delivered.
    pub messages: u64,
    /// Completed messages refused for a total length or msgCrc mismatch.
    pub bad_crc: u64,
    /// Complete messages refused by `msg::parse_message`.
    pub parse_errors: u64,
    /// Partial messages dropped after the 5 s expiry.
    pub expired: u64,
    /// msgIds skipped: still missing 3 s after a later id completed.
    pub missed: u64,
    /// Session id changes seen.
    pub session_changes: u64,
}

/// Partial messages expire after this long without completing.
pub const PARTIAL_TTL_MS: u64 = 5000;
/// Delivered msgIds remembered to drop repeats.
pub const DONE_KEEP: usize = 512;
/// A skipped msgId counts as missed when still missing this long after a later id completed.
pub const MISSED_GRACE_MS: u64 = 3000;
/// Gaps wider than this between consecutive msgIds are not tracked (an addon reload restarts at 1).
const MAX_GAP: u16 = 64;

struct Partial {
    count: u8,
    total: u16,
    crc: u32,
    parts: Vec<Option<Vec<u8>>>,
    have: usize,
    t0: u64,
}

pub struct Reassembler {
    session: Option<u16>,
    session_change: Option<u16>,
    stats: ReasmStats,
    partial: HashMap<u16, Partial>,
    done: VecDeque<u16>,
    max_mid: Option<u16>,
    missing: HashMap<u16, u64>,
}

impl Default for Reassembler {
    fn default() -> Self {
        Self::new()
    }
}

impl Reassembler {
    pub fn new() -> Self {
        Reassembler {
            session: None,
            session_change: None,
            stats: ReasmStats::default(),
            partial: HashMap::new(),
            done: VecDeque::with_capacity(DONE_KEEP + 1),
            max_mid: None,
            missing: HashMap::new(),
        }
    }

    /// Feeds one decoded frame; returns the messages it completed, in completion order.
    pub fn push(&mut self, f: &strip::Frame, now_ms: u64) -> Vec<msg::Message> {
        let st = &mut self.stats;
        st.frames += 1;
        if self.session != Some(f.session) {
            if let Some(old) = self.session {
                st.session_changes += 1;
                self.session_change = Some(old);
            }
            self.session = Some(f.session);
            self.partial.clear();
            self.done.clear();
            self.missing.clear();
            self.max_mid = None;
        }
        let before = self.partial.len();
        self.partial.retain(|_, p| now_ms.saturating_sub(p.t0) <= PARTIAL_TTL_MS);
        st.expired += (before - self.partial.len()) as u64;
        let before = self.missing.len();
        self.missing.retain(|_, t| now_ms.saturating_sub(*t) <= MISSED_GRACE_MS);
        st.missed += (before - self.missing.len()) as u64;
        let mut out = Vec::new();
        for c in &f.chunks {
            st.chunks += 1;
            if self.done.contains(&c.msg_id) {
                st.duplicates += 1;
                continue;
            }
            let restart = match self.partial.get(&c.msg_id) {
                Some(p) => (p.count, p.total, p.crc) != (c.count, c.total, c.msg_crc),
                None => true,
            };
            if restart {
                if self.partial.contains_key(&c.msg_id) {
                    st.conflicts += 1;
                }
                self.partial.insert(
                    c.msg_id,
                    Partial {
                        count: c.count,
                        total: c.total,
                        crc: c.msg_crc,
                        parts: vec![None; usize::from(c.count)],
                        have: 0,
                        t0: now_ms,
                    },
                );
            }
            let p = self.partial.get_mut(&c.msg_id).expect("partial");
            let slot = &mut p.parts[usize::from(c.idx)];
            if slot.is_none() {
                p.have += 1;
            }
            *slot = Some(c.data.clone());
            if p.have < usize::from(p.count) {
                continue;
            }
            let p = self.partial.remove(&c.msg_id).expect("partial");
            let mut m = Vec::with_capacity(usize::from(p.total));
            for part in p.parts.into_iter().flatten() {
                m.extend_from_slice(&part);
            }
            if m.len() != usize::from(p.total) || crc::crc32(&m) != p.crc {
                st.bad_crc += 1;
                continue;
            }
            self.done.push_back(c.msg_id);
            if self.done.len() > DONE_KEEP {
                self.done.pop_front();
            }
            self.missing.remove(&c.msg_id);
            let mid = c.msg_id;
            match self.max_mid {
                None => self.max_mid = Some(mid),
                Some(prev) => {
                    let ahead = mid.wrapping_sub(prev);
                    if ahead > 0 && ahead <= 32768 {
                        if ahead <= MAX_GAP {
                            let mut g = prev.wrapping_add(1);
                            while g != mid {
                                if g != 0 && !self.done.contains(&g) {
                                    self.missing.entry(g).or_insert(now_ms);
                                }
                                g = g.wrapping_add(1);
                            }
                        }
                        self.max_mid = Some(mid);
                    }
                }
            }
            match msg::parse_message(&m) {
                Ok(parsed) => {
                    st.messages += 1;
                    out.push(parsed);
                }
                Err(_) => st.parse_errors += 1,
            }
        }
        out
    }

    /// Session id of the latest frame, None before the first frame.
    pub fn session(&self) -> Option<u16> {
        self.session
    }

    /// After the session changed from one id to another, returns the previous id once
    /// (so the caller can clear that session's work); None otherwise.
    pub fn take_session_change(&mut self) -> Option<u16> {
        self.session_change.take()
    }

    pub fn stats(&self) -> ReasmStats {
        self.stats
    }

    /// msgIds currently known to be skipped (a later id completed, this one not yet).
    pub fn pending_missing(&self) -> usize {
        self.missing.len()
    }
}
