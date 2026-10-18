//! Echo translator for tests: Loading for 300 ms on the first job, then Done(uppercase of the unmarked text) after 200 ms.
//!
//! It runs through the same queue, cache and shortcuts as the real translator (see `worker.rs`),
//! only the model call is replaced; it never starts a process.

use std::time::Duration;

pub const LOAD: Duration = Duration::from_millis(300);
pub const WORK: Duration = Duration::from_millis(200);

/// The echo "translation": marks removed, span contents kept, uppercased.
pub fn output(marked: &str) -> String {
    crate::protect::unmarked(marked).to_uppercase()
}

#[cfg(test)]
mod tests {
    #[test]
    fn uppercase_of_unmarked_text() {
        assert_eq!(super::output("wts \u{2}[Linen Cloth]\u{3} cheap\n"), "WTS [LINEN CLOTH] CHEAP");
        assert_eq!(super::output("été"), "ÉTÉ");
    }
}
