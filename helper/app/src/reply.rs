//! The UI thread's replies (conversation window, design A.6.2-A.6.4): one [`ReplyBook`] per reply
//! request, in memory only, holding its clipboard state ([`ReplyClip`]) and its own text (copied
//! as it is when the engine finds it already in the target language).
//!
//! - [`Replies::start`] on `Action::ReplyStart`: `begin` (the clipboard is emptied at once when
//!   the copy bit and the tray switch allow it, decision D2).
//! - [`Replies::on_status`] on each engine update: a final result runs `finish` (the clipboard
//!   write, never for a partial result); the outcome then decides the row 0 status the overlay
//!   paints.
//! - [`Replies::end`] on `Action::ReplyEnd` and [`Replies::end_all`] at exit: `release` (the
//!   clipboard is emptied only when it still holds the Helper's own write).

use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use tp_engine::{ChatText, JobKey, Status};

use crate::clip::{Begin, ClipOs, ClipOutcome, Permit, ReplyClip};

/// Replies kept at most (each new reply ends the previous one, so normally one or two).
pub const MAX_REPLIES: usize = 16;

/// One reply request.
pub struct ReplyBook {
    pub clip: ReplyClip,
    /// The request's own text (the player's draft with its link marks).
    pub original: ChatText,
    pub started: Instant,
}

impl ReplyBook {
    pub fn outcome(&self) -> Option<ClipOutcome> {
        self.clip.outcome()
    }
}

/// What a call did to the clipboard, for the log and the diag record (no text).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipEvent {
    Begin(Begin),
    Finish(ClipOutcome),
    Released,
}

impl ClipEvent {
    pub fn what(self) -> &'static str {
        match self {
            ClipEvent::Begin(_) => "begin",
            ClipEvent::Finish(_) => "finish",
            ClipEvent::Released => "release",
        }
    }

    pub fn outcome(self) -> &'static str {
        match self {
            ClipEvent::Begin(b) => b.name(),
            ClipEvent::Finish(o) => o.name(),
            ClipEvent::Released => "emptied",
        }
    }
}

#[derive(Default)]
pub struct Replies {
    books: HashMap<JobKey, ReplyBook>,
    order: VecDeque<JobKey>,
}

impl Replies {
    pub fn new() -> Replies {
        Replies::default()
    }

    pub fn get(&self, key: &JobKey) -> Option<&ReplyBook> {
        self.books.get(key)
    }

    pub fn books(&self) -> &HashMap<JobKey, ReplyBook> {
        &self.books
    }

    pub fn len(&self) -> usize {
        self.books.len()
    }

    pub fn is_empty(&self) -> bool {
        self.books.is_empty()
    }

    /// A reply request arrived (`copy`: its copy bit; `switch_on`: the tray switch and not
    /// `--no-clipboard`). A repeated start of the same key is ignored.
    pub fn start(&mut self, os: &mut dyn ClipOs, key: JobKey, copy: bool, switch_on: bool, original: ChatText) -> Option<ClipEvent> {
        if self.books.contains_key(&key) {
            return None;
        }
        let mut clip = ReplyClip::new(Permit::new(copy, switch_on));
        let b = clip.begin(os);
        while self.order.len() >= MAX_REPLIES {
            // the oldest (already ended in practice): released if it still owns the clipboard
            if let Some(k) = self.order.pop_front() {
                if let Some(mut old) = self.books.remove(&k) {
                    old.clip.release(os);
                }
            }
        }
        self.books.insert(key, ReplyBook { clip, original, started: Instant::now() });
        self.order.push_back(key);
        Some(ClipEvent::Begin(b))
    }

    /// An engine update for `key`: a final result of a reply writes the clipboard (when allowed).
    pub fn on_status(&mut self, os: &mut dyn ClipOs, key: &JobKey, status: &Status) -> Option<ClipEvent> {
        let book = self.books.get_mut(key)?;
        if book.clip.outcome().is_some() {
            return None;
        }
        let text: &str = match status {
            Status::Done(t) => t.as_str(),
            // already in the target language, or nothing to translate: the player's own text
            Status::Same | Status::Nothing => book.original.as_str(),
            _ => return None,
        };
        let text = text.to_string();
        Some(ClipEvent::Finish(book.clip.finish(os, &text)))
    }

    /// The reply ended: forgets it and empties the clipboard when it still holds its write.
    pub fn end(&mut self, os: &mut dyn ClipOs, key: &JobKey) -> Option<ClipEvent> {
        self.order.retain(|k| k != key);
        let mut book = self.books.remove(key)?;
        book.clip.release(os).then_some(ClipEvent::Released)
    }

    /// Graceful exit: every reply ends.
    pub fn end_all(&mut self, os: &mut dyn ClipOs) -> usize {
        let keys: Vec<JobKey> = self.order.iter().copied().collect();
        keys.iter().filter(|k| self.end(os, k).is_some()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::FakeClip;

    fn key(req_id: u16) -> JobKey {
        JobKey { session: 7, req_id }
    }

    fn text(s: &str) -> ChatText {
        ChatText::new(s.to_string())
    }

    #[test]
    fn start_finish_end() {
        let mut os = FakeClip::new();
        os.user_copy("old clipboard");
        let mut r = Replies::new();
        let ev = r.start(&mut os, key(1), true, true, text("on part dans 5 min ?"));
        assert!(matches!(ev, Some(ClipEvent::Begin(Begin::Emptied(_)))));
        assert_eq!(os.text, None, "emptied on receipt (D2)");
        // partial results are never copied
        assert_eq!(r.on_status(&mut os, &key(1), &Status::Partial(text("Brechen"))), None);
        assert_eq!(r.on_status(&mut os, &key(1), &Status::Translating), None);
        assert_eq!(os.writes, 0);
        let ev = r.on_status(&mut os, &key(1), &Status::Done(text("Brechen wir in 5 Minuten auf?")));
        assert_eq!(ev, Some(ClipEvent::Finish(ClipOutcome::Copied { cut: false })));
        assert_eq!(os.text.as_deref(), Some("Brechen wir in 5 Minuten auf?"));
        assert_eq!(r.get(&key(1)).and_then(|b| b.outcome()), Some(ClipOutcome::Copied { cut: false }));
        // a later update changes nothing
        assert_eq!(r.on_status(&mut os, &key(1), &Status::Done(text("other"))), None);
        // CANCEL after the send: emptied, since it is still ours
        assert_eq!(r.end(&mut os, &key(1)), Some(ClipEvent::Released));
        assert_eq!(os.text, None);
        assert!(r.is_empty());
    }

    #[test]
    fn same_and_nothing_copy_the_players_own_text() {
        for st in [Status::Same, Status::Nothing] {
            let mut os = FakeClip::new();
            let mut r = Replies::new();
            r.start(&mut os, key(2), true, true, text("\u{2}\u{4}FFFFFF[Linen Cloth]\u{3} ok"));
            assert_eq!(r.on_status(&mut os, &key(2), &st), Some(ClipEvent::Finish(ClipOutcome::Copied { cut: false })));
            assert_eq!(os.text.as_deref(), Some("[Linen Cloth] ok"), "link marks and colour removed");
        }
    }

    #[test]
    fn a_coloured_translation_is_copied_as_plain_text() {
        // L3: the engine writes a coloured link back as a mark; the clipboard gets the name only
        let mut os = FakeClip::new();
        let mut r = Replies::new();
        r.start(&mut os, key(3), true, true, text("wts \u{2}\u{4}0070DD[Linen Cloth]\u{3}"));
        let ev = r.on_status(&mut os, &key(3), &Status::Done(text("Vends \u{2}\u{4}0070DD[Linen Cloth]\u{3}")));
        assert_eq!(ev, Some(ClipEvent::Finish(ClipOutcome::Copied { cut: false })));
        assert_eq!(os.text.as_deref(), Some("Vends [Linen Cloth]"));
    }

    #[test]
    fn failures_write_nothing_and_release_nothing() {
        let mut os = FakeClip::new();
        let mut r = Replies::new();
        r.start(&mut os, key(3), true, true, text("x"));
        assert_eq!(r.on_status(&mut os, &key(3), &Status::Failed(tp_engine::FailKind::Timeout)), None);
        assert_eq!(os.writes, 0);
        assert_eq!(r.end(&mut os, &key(3)), None);
    }

    #[test]
    fn switch_off_and_no_consent() {
        let mut os = FakeClip::new();
        os.user_copy("kept");
        let mut r = Replies::new();
        assert_eq!(r.start(&mut os, key(4), true, false, text("x")), Some(ClipEvent::Begin(Begin::NotAllowed)));
        assert_eq!(r.on_status(&mut os, &key(4), &Status::Done(text("y"))), Some(ClipEvent::Finish(ClipOutcome::Off)));
        assert_eq!(r.start(&mut os, key(5), false, true, text("x")), Some(ClipEvent::Begin(Begin::NotAllowed)));
        assert_eq!(r.on_status(&mut os, &key(5), &Status::Done(text("y"))), Some(ClipEvent::Finish(ClipOutcome::NotAllowed)));
        assert_eq!((os.opens, os.text.as_deref()), (0, Some("kept")));
    }

    #[test]
    fn a_repeated_start_keeps_the_state_and_exit_releases() {
        let mut os = FakeClip::new();
        let mut r = Replies::new();
        r.start(&mut os, key(6), true, true, text("x"));
        r.on_status(&mut os, &key(6), &Status::Done(text("y")));
        let empties = os.empties;
        assert_eq!(r.start(&mut os, key(6), true, true, text("x")), None, "no second empty");
        assert_eq!(os.empties, empties);
        assert_eq!(r.end_all(&mut os), 1);
        assert_eq!(os.text, None);
        // unknown keys (chat requests) are not replies
        assert_eq!(r.on_status(&mut os, &key(99), &Status::Done(text("z"))), None);
        // bounded
        for i in 0..40 {
            r.start(&mut os, key(100 + i), false, true, text("x"));
        }
        assert_eq!(r.len(), MAX_REPLIES);
    }
}
