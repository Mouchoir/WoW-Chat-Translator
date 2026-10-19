//! tp_engine: the local translation engine of Translate Companion (the Helper).
//!
//! It finds the llama.cpp runtime and the Qwen3 model on disk, starts `llama-server` on first use
//! (GPU when enough VRAM is free, else CPU), builds the prompt with placeholders and glossary,
//! post-processes the output, caches results and reports per-request [`Status`] updates.
//! Design: docs/ENGINE.md (design sections D.4 and D.5).
//!
//! Chat text only travels inside [`ChatText`]; log events ([`EngineEvent`]) carry numbers, enums
//! and `&'static str` only.

pub use tp_bridge::msg::ChatText;

mod cache;
mod config;
mod echo;
mod events;
mod queue;
mod worker;

// Building blocks, public only for the E1 evaluation binary (src/bin/evalprompt.rs). Not part
// of the D.2 interface: the app uses Engine, EngineHandle, selftest and prompt only.
#[doc(hidden)]
pub mod client;
#[doc(hidden)]
pub mod device;
#[doc(hidden)]
pub mod discover;
#[doc(hidden)]
pub mod flags;
#[doc(hidden)]
pub mod grammar;
#[doc(hidden)]
pub mod model;
#[doc(hidden)]
pub mod pipeline;
#[doc(hidden)]
pub mod policy;
#[doc(hidden)]
pub mod post;
pub mod prompt;
#[doc(hidden)]
pub mod protect;
pub mod selftest;
#[doc(hidden)]
pub mod server;
#[doc(hidden)]
pub mod verbatim;

#[cfg(test)]
mod testutil;

pub use config::{BackendPref, EngineConfig, Translator};
pub use events::{Backend, EngineEvent, EngineState, ServerLine};
pub use worker::{Engine, EngineHandle};

/// Identifies one request: the addon session and its reqId.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct JobKey {
    pub session: u16,
    pub req_id: u16,
}

/// One translation request.
pub struct Job {
    pub key: JobKey,
    pub target: String,
    /// Marked text: item links and `{...}` groups wrapped in 0x02 ... 0x03.
    pub text: ChatText,
    /// Glossary pairs (term, meaning).
    pub pairs: Vec<(ChatText, ChatText)>,
    pub no_cache: bool,
    /// A reply the user is waiting for (the conversation window's reply card): it goes ahead of
    /// every waiting job that is not urgent (it never interrupts the job in flight).
    pub urgent: bool,
}

#[derive(Clone, Copy, Debug)]
pub enum FailKind {
    Timeout,
    ServerDied,
    BadOutput,
    Http,
    Overloaded,
}

/// Progress of one job, as shown in its card.
pub enum Status {
    Queued { ahead: u8 },
    Loading { first_gpu_use: bool },
    Translating,
    /// Streaming output so far (CPU only, at most 5 updates per second).
    Partial(ChatText),
    Done(ChatText),
    /// The text is already in the target language.
    Same,
    /// No letters outside placeholders: nothing to translate.
    Nothing,
    Failed(FailKind),
    LowMemory,
    NotInstalled,
}

pub struct Update {
    pub key: JobKey,
    pub status: Status,
}
