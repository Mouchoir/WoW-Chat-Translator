//! Echo translator for tests: Loading for 300 ms on the first job, then Done(uppercase of the unmarked text) after 200 ms.
//!
//! It runs through the same queue, cache and shortcuts as the real translator (see `worker.rs`),
//! only the model call is replaced; it never starts a process.

use std::time::Duration;

use tp_bridge::marks::{self, Piece};

pub const LOAD: Duration = Duration::from_millis(300);
pub const WORK: Duration = Duration::from_millis(200);

/// The echo "translation": marks removed, span contents kept, uppercased. A span with a link
/// colour keeps it (written back as a coloured mark, like a real translation), so the overlay's
/// coloured runs can be checked end to end.
pub fn output(marked: &str) -> String {
    let mut s = String::new();
    for p in marks::split(&crate::protect::clean_input(marked)) {
        match p {
            Piece::Plain(t) => s.push_str(&t.to_uppercase()),
            Piece::Span { text, rgb } => s.push_str(&marks::span(&text.to_uppercase(), rgb)),
        }
    }
    s.trim().to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn uppercase_of_unmarked_text() {
        assert_eq!(super::output("wts \u{2}[Linen Cloth]\u{3} cheap\n"), "WTS [LINEN CLOTH] CHEAP");
        assert_eq!(super::output("été"), "ÉTÉ");
    }

    #[test]
    fn link_colours_are_kept() {
        assert_eq!(super::output("wts \u{2}\u{4}0070dd[Linen Cloth]\u{3} cheap"), "WTS \u{2}\u{4}0070DD[LINEN CLOTH]\u{3} CHEAP");
    }
}
