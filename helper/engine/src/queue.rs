//! FIFO of 8 waiting jobs and 1 in flight: duplicate merge, overload and age drops, cancel and clear.
//!
//! "In flight" means the job the worker is serving now, from server start to result. Followers
//! (later jobs with the same cache key) ride along and get the same updates.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::{ChatText, FailKind, JobKey, Status};

pub const MAX_WAITING: usize = 8;
pub const MAX_AGE: Duration = Duration::from_secs(30);

/// Cloneable form of [`Status`], used to send one update to several keys. No Debug: it carries
/// translations.
#[derive(Clone)]
pub enum St {
    Queued { ahead: u8 },
    Loading { first_gpu_use: bool },
    Translating,
    Partial(String),
    Done(String),
    Same,
    Nothing,
    Failed(FailKind),
    LowMemory,
    NotInstalled,
}

impl St {
    pub fn to_status(&self) -> Status {
        match self {
            St::Queued { ahead } => Status::Queued { ahead: *ahead },
            St::Loading { first_gpu_use } => Status::Loading { first_gpu_use: *first_gpu_use },
            St::Translating => Status::Translating,
            St::Partial(s) => Status::Partial(ChatText::new(s.clone())),
            St::Done(s) => Status::Done(ChatText::new(s.clone())),
            St::Same => Status::Same,
            St::Nothing => Status::Nothing,
            St::Failed(k) => Status::Failed(*k),
            St::LowMemory => Status::LowMemory,
            St::NotInstalled => Status::NotInstalled,
        }
    }

    /// Terminal statuses end a job.
    #[cfg(test)]
    pub fn is_final(&self) -> bool {
        !matches!(self, St::Queued { .. } | St::Loading { .. } | St::Translating | St::Partial(_))
    }
}

/// What a job asks for (chat text stays in [`ChatText`]).
pub struct Work {
    pub target: String,
    pub text: ChatText,
    pub pairs: Vec<(ChatText, ChatText)>,
}

pub struct Entry {
    pub keys: Vec<JobKey>,
    pub work: Work,
    pub cache_key: u64,
    pub submitted: Instant,
    /// A reply the user waits for: waits ahead of every entry that is not urgent.
    pub urgent: bool,
    last_ahead: Option<u8>,
}

impl Entry {
    pub fn new(key: JobKey, work: Work, cache_key: u64, now: Instant) -> Entry {
        Entry { keys: vec![key], work, cache_key, submitted: now, urgent: false, last_ahead: None }
    }

    /// Marks the entry urgent (a reply, `Job::urgent`).
    pub fn urgent(mut self, urgent: bool) -> Entry {
        self.urgent = urgent;
        self
    }
}

pub type Emit = Vec<(JobKey, St)>;

pub enum Pushed {
    /// Waiting; a Queued update was emitted.
    Waiting,
    /// Merged into the job in flight: the caller sends it the flight's current status.
    MergedInFlight,
    /// The key is already waiting or in flight: nothing changed.
    Duplicate,
}

pub struct Queue {
    waiting: VecDeque<Entry>,
    flight: Option<Entry>,
    max_waiting: usize,
    max_age: Duration,
}

impl Default for Queue {
    fn default() -> Self {
        Queue::new(MAX_WAITING, MAX_AGE)
    }
}

impl Queue {
    pub fn new(max_waiting: usize, max_age: Duration) -> Queue {
        Queue { waiting: VecDeque::new(), flight: None, max_waiting: max_waiting.max(1), max_age }
    }

    pub fn contains(&self, k: JobKey) -> bool {
        self.flight.iter().chain(self.waiting.iter()).any(|e| e.keys.contains(&k))
    }

    pub fn waiting_len(&self) -> usize {
        self.waiting.len()
    }

    pub fn is_idle(&self) -> bool {
        self.waiting.is_empty() && self.flight.is_none()
    }

    pub fn flight(&self) -> Option<&Entry> {
        self.flight.as_ref()
    }

    pub fn flight_keys(&self) -> Vec<JobKey> {
        self.flight.as_ref().map(|e| e.keys.clone()).unwrap_or_default()
    }

    pub fn push(&mut self, e: Entry, out: &mut Emit) -> Pushed {
        if e.keys.iter().any(|&k| self.contains(k)) {
            return Pushed::Duplicate;
        }
        if let Some(f) = self.flight.as_mut() {
            if f.cache_key == e.cache_key {
                f.keys.extend(e.keys);
                return Pushed::MergedInFlight;
            }
        }
        if let Some(i) = self.waiting.iter().position(|w| w.cache_key == e.cache_key) {
            let urgent = e.urgent;
            let w = &mut self.waiting[i];
            w.keys.extend(e.keys);
            // Re-announce the position to every key (the new one has none yet).
            w.last_ahead = None;
            // An urgent key makes the shared entry urgent: it moves ahead of the others.
            if urgent && !w.urgent {
                if let Some(mut w) = self.waiting.remove(i) {
                    w.urgent = true;
                    let at = self.urgent_end();
                    self.waiting.insert(at, w);
                }
            }
            self.announce(out);
            return Pushed::Waiting;
        }
        if self.waiting.len() >= self.max_waiting {
            // The oldest job that is not urgent is dropped (a reply never makes room for a chat
            // line's translation by being dropped itself).
            let i = self.waiting.iter().position(|w| !w.urgent).unwrap_or(0);
            if let Some(old) = self.waiting.remove(i) {
                for k in old.keys {
                    out.push((k, St::Failed(FailKind::Overloaded)));
                }
            }
        }
        if e.urgent {
            let at = self.urgent_end();
            self.waiting.insert(at, e);
        } else {
            self.waiting.push_back(e);
        }
        self.announce(out);
        Pushed::Waiting
    }

    /// Index after the last waiting urgent entry (urgent entries wait at the front, in order).
    fn urgent_end(&self) -> usize {
        self.waiting.iter().take_while(|w| w.urgent).count()
    }

    /// Emits Queued{ahead} for every waiting entry whose position changed.
    pub fn announce(&mut self, out: &mut Emit) {
        let busy = usize::from(self.flight.is_some());
        for (i, w) in self.waiting.iter_mut().enumerate() {
            let ahead = (i + busy).min(255) as u8;
            if w.last_ahead != Some(ahead) {
                w.last_ahead = Some(ahead);
                for &k in &w.keys {
                    out.push((k, St::Queued { ahead }));
                }
            }
        }
    }

    /// Drops waiting jobs older than the age limit with Failed(Timeout).
    pub fn expire(&mut self, now: Instant, out: &mut Emit) {
        let max_age = self.max_age;
        let mut dropped = false;
        self.waiting.retain(|w| {
            let keep = now.duration_since(w.submitted) <= max_age;
            if !keep {
                dropped = true;
                for &k in &w.keys {
                    out.push((k, St::Failed(FailKind::Timeout)));
                }
            }
            keep
        });
        if dropped {
            self.announce(out);
        }
    }

    /// Moves the oldest waiting job in flight (after age drops). None when busy or empty.
    pub fn start_next(&mut self, now: Instant, out: &mut Emit) -> Option<&Entry> {
        if self.flight.is_some() {
            return None;
        }
        self.expire(now, out);
        let e = self.waiting.pop_front()?;
        self.flight = Some(e);
        self.announce(out);
        self.flight.as_ref()
    }

    /// Ends the job in flight; its remaining keys get the caller's final status.
    pub fn finish_flight(&mut self, out: &mut Emit) -> Option<Entry> {
        let f = self.flight.take();
        self.announce(out);
        f
    }

    /// Cancels one key: a waiting job is removed (when no other key shares it); a job in flight
    /// keeps running but this key gets nothing more.
    pub fn cancel(&mut self, k: JobKey, out: &mut Emit) -> bool {
        let mut hit = false;
        if let Some(f) = self.flight.as_mut() {
            let before = f.keys.len();
            f.keys.retain(|&x| x != k);
            hit |= f.keys.len() != before;
        }
        for w in self.waiting.iter_mut() {
            let before = w.keys.len();
            w.keys.retain(|&x| x != k);
            hit |= w.keys.len() != before;
        }
        self.waiting.retain(|w| !w.keys.is_empty());
        if hit {
            self.announce(out);
        }
        hit
    }

    /// Cancels every key of one addon session; returns how many keys were cancelled.
    pub fn clear_session(&mut self, session: u16, out: &mut Emit) -> usize {
        let mut n = 0;
        for e in self.flight.iter_mut().chain(self.waiting.iter_mut()) {
            let before = e.keys.len();
            e.keys.retain(|k| k.session != session);
            n += before - e.keys.len();
        }
        self.waiting.retain(|w| !w.keys.is_empty());
        if n > 0 {
            self.announce(out);
        }
        n
    }

    /// Fails every waiting job (not the one in flight) with one status.
    pub fn fail_waiting(&mut self, st: St, out: &mut Emit) {
        for w in self.waiting.drain(..) {
            for k in w.keys {
                out.push((k, st.clone()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(session: u16, req_id: u16) -> JobKey {
        JobKey { session, req_id }
    }

    fn entry(k: JobKey, cache_key: u64, now: Instant) -> Entry {
        Entry::new(k, Work { target: "fr".into(), text: ChatText::new("x".into()), pairs: vec![] }, cache_key, now)
    }

    fn queued(out: &Emit, k: JobKey) -> Option<u8> {
        out.iter().rev().find_map(|(x, s)| match s {
            St::Queued { ahead } if *x == k => Some(*ahead),
            _ => None,
        })
    }

    #[test]
    fn fifo_positions_and_flight() {
        let now = Instant::now();
        let mut q = Queue::default();
        let mut out = Emit::new();
        q.push(entry(key(1, 1), 1, now), &mut out);
        q.push(entry(key(1, 2), 2, now), &mut out);
        assert_eq!(queued(&out, key(1, 1)), Some(0));
        assert_eq!(queued(&out, key(1, 2)), Some(1));
        out.clear();
        let f = q.start_next(now, &mut out).unwrap();
        assert_eq!(f.keys, vec![key(1, 1)]);
        // Job 2 is still 1 ahead (job 1 in flight): no new update.
        assert!(out.is_empty());
        assert!(q.start_next(now, &mut out).is_none(), "one in flight at most");
        q.push(entry(key(1, 3), 3, now), &mut out);
        assert_eq!(queued(&out, key(1, 3)), Some(2));
        out.clear();
        q.finish_flight(&mut out).unwrap();
        assert_eq!(queued(&out, key(1, 2)), Some(0));
        assert_eq!(queued(&out, key(1, 3)), Some(1));
    }

    #[test]
    fn duplicates_merge() {
        let now = Instant::now();
        let mut q = Queue::default();
        let mut out = Emit::new();
        q.push(entry(key(1, 1), 7, now), &mut out);
        assert!(matches!(q.push(entry(key(1, 1), 7, now), &mut out), Pushed::Duplicate));
        q.push(entry(key(1, 2), 9, now), &mut out);
        out.clear();
        // Same cache key as a waiting job: merged, announced at that position.
        assert!(matches!(q.push(entry(key(2, 5), 9, now), &mut out), Pushed::Waiting));
        assert_eq!(q.waiting_len(), 2);
        assert_eq!(queued(&out, key(2, 5)), Some(1));
        q.start_next(now, &mut out);
        // Same cache key as the job in flight.
        assert!(matches!(q.push(entry(key(3, 1), 7, now), &mut out), Pushed::MergedInFlight));
        assert_eq!(q.flight_keys(), vec![key(1, 1), key(3, 1)]);
    }

    #[test]
    fn overload_drops_oldest() {
        let now = Instant::now();
        let mut q = Queue::default();
        let mut out = Emit::new();
        q.push(entry(key(1, 100), 100, now), &mut out);
        q.start_next(now, &mut out);
        for i in 0..MAX_WAITING as u16 {
            q.push(entry(key(1, i), i as u64, now), &mut out);
        }
        out.clear();
        q.push(entry(key(1, 50), 50, now), &mut out);
        assert!(out.iter().any(|(k, s)| *k == key(1, 0) && matches!(s, St::Failed(FailKind::Overloaded))));
        assert_eq!(q.waiting_len(), MAX_WAITING);
        assert!(!q.contains(key(1, 0)));
        assert_eq!(queued(&out, key(1, 50)), Some(MAX_WAITING as u8));
    }

    #[test]
    fn age_drop() {
        let t0 = Instant::now();
        let mut q = Queue::default();
        let mut out = Emit::new();
        q.push(entry(key(1, 1), 1, t0), &mut out);
        q.push(entry(key(1, 2), 2, t0 + Duration::from_secs(20)), &mut out);
        out.clear();
        let f = q.start_next(t0 + Duration::from_secs(31), &mut out).unwrap();
        assert_eq!(f.keys, vec![key(1, 2)]);
        assert!(out.iter().any(|(k, s)| *k == key(1, 1) && matches!(s, St::Failed(FailKind::Timeout))));
    }

    #[test]
    fn cancel_and_clear() {
        let now = Instant::now();
        let mut q = Queue::default();
        let mut out = Emit::new();
        q.push(entry(key(1, 1), 1, now), &mut out);
        q.push(entry(key(1, 2), 2, now), &mut out);
        q.push(entry(key(2, 2), 2, now), &mut out); // merged with (1,2)
        q.push(entry(key(2, 3), 3, now), &mut out);
        q.start_next(now, &mut out);
        out.clear();
        // Cancel in flight: the key is detached, the flight continues.
        assert!(q.cancel(key(1, 1), &mut out));
        assert!(q.flight().is_some() && q.flight_keys().is_empty());
        // Cancel one of two merged keys: the entry stays.
        assert!(q.cancel(key(1, 2), &mut out));
        assert_eq!(q.waiting_len(), 2);
        assert!(!q.cancel(key(9, 9), &mut out));
        assert_eq!(q.clear_session(2, &mut out), 2);
        assert_eq!(q.waiting_len(), 0);
        assert!(q.finish_flight(&mut out).is_some());
        assert!(q.is_idle());
    }

    fn urgent(k: JobKey, cache_key: u64, now: Instant) -> Entry {
        entry(k, cache_key, now).urgent(true)
    }

    fn order(q: &Queue) -> Vec<JobKey> {
        q.waiting.iter().flat_map(|e| e.keys.clone()).collect()
    }

    #[test]
    fn urgent_jobs_wait_ahead_of_the_others() {
        let now = Instant::now();
        let mut q = Queue::default();
        let mut out = Emit::new();
        q.push(entry(key(1, 1), 1, now), &mut out);
        q.start_next(now, &mut out);
        q.push(entry(key(1, 2), 2, now), &mut out);
        q.push(entry(key(1, 3), 3, now), &mut out);
        out.clear();
        // A reply goes ahead of the waiting chat lines, never ahead of the job in flight.
        q.push(urgent(key(1, 4), 4, now), &mut out);
        assert_eq!(order(&q), vec![key(1, 4), key(1, 2), key(1, 3)]);
        assert_eq!(q.flight_keys(), vec![key(1, 1)]);
        assert_eq!(queued(&out, key(1, 4)), Some(1));
        assert_eq!(queued(&out, key(1, 2)), Some(2), "moved back one place");
        assert_eq!(queued(&out, key(1, 3)), Some(3));
        // A second urgent job waits after the first one (urgent jobs keep their order).
        q.push(urgent(key(1, 5), 5, now), &mut out);
        assert_eq!(order(&q), vec![key(1, 4), key(1, 5), key(1, 2), key(1, 3)]);
        // A later chat line still goes to the back.
        q.push(entry(key(1, 6), 6, now), &mut out);
        assert_eq!(order(&q).last(), Some(&key(1, 6)));
        q.finish_flight(&mut out);
        assert_eq!(q.start_next(now, &mut out).map(|e| e.keys.clone()), Some(vec![key(1, 4)]));
    }

    #[test]
    fn an_urgent_key_moves_a_shared_entry_forward() {
        let now = Instant::now();
        let mut q = Queue::default();
        let mut out = Emit::new();
        q.push(entry(key(1, 1), 1, now), &mut out);
        q.start_next(now, &mut out);
        q.push(entry(key(1, 2), 2, now), &mut out);
        q.push(entry(key(1, 3), 3, now), &mut out);
        // the reply's text is the same as a waiting chat line's (same cache key)
        q.push(urgent(key(1, 9), 3, now), &mut out);
        assert_eq!(order(&q), vec![key(1, 3), key(1, 9), key(1, 2)]);
        assert!(q.waiting[0].urgent);
    }

    #[test]
    fn overload_never_drops_an_urgent_job_first() {
        let now = Instant::now();
        let mut q = Queue::default();
        let mut out = Emit::new();
        q.push(entry(key(1, 100), 100, now), &mut out);
        q.start_next(now, &mut out);
        q.push(urgent(key(1, 50), 50, now), &mut out);
        for i in 0..(MAX_WAITING as u16 - 1) {
            q.push(entry(key(1, i), u64::from(i), now), &mut out);
        }
        out.clear();
        q.push(entry(key(1, 60), 60, now), &mut out);
        assert!(q.contains(key(1, 50)), "the reply stays");
        assert!(out.iter().any(|(k, s)| *k == key(1, 0) && matches!(s, St::Failed(FailKind::Overloaded))), "the oldest chat line goes");
        assert_eq!(q.waiting_len(), MAX_WAITING);
    }

    #[test]
    fn st_conversion() {
        assert!(matches!(St::Done("a".into()).to_status(), Status::Done(t) if t.as_str() == "a"));
        assert!(St::Same.is_final() && St::Failed(FailKind::Http).is_final() && St::NotInstalled.is_final());
        assert!(!St::Queued { ahead: 1 }.is_final() && !St::Partial("x".into()).is_final());
    }
}
