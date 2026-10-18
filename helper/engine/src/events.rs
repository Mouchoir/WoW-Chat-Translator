//! Engine state and log events. Events carry numbers, enums and `&'static str` only: never chat
//! text, translations, glossary terms, prompts or paths. The one exception is [`ServerLine`], a
//! llama-server output line that passed the lifecycle allowlist; it is sent only after a crash or
//! a failed start (design D.4).

use crate::{BackendPref, FailKind};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EngineState {
    /// Runtime or model not found.
    NotInstalled,
    /// Found, no server running.
    Unloaded,
    /// Server starting or warming up.
    Loading,
    ReadyGpu,
    ReadyCpu,
    /// A job is in flight.
    Busy,
    /// Start failed or too many crashes; retried later.
    Error,
}

/// The backend a server was started with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Cpu,
    Vulkan,
    Cuda,
}

/// One llama-server output line that passed the allowlist (`load_backend`, `offloaded`,
/// `model loaded`, `listening`, `error`, `failed`), cut to 160 characters. Lines that could echo
/// a prompt or a completion are refused. Only the allowlist filter can build one.
#[derive(Clone)]
pub struct ServerLine(String);

impl ServerLine {
    const ALLOW: [&'static str; 6] = ["load_backend", "offloaded", "model loaded", "listening", "error", "failed"];
    const DENY: [&'static str; 5] = ["<|", "\"prompt\"", "\"content\"", "prompt:", "content:"];

    pub(crate) fn filter(raw: &str) -> Option<ServerLine> {
        let line = raw.trim();
        let lower = line.to_lowercase();
        if !Self::ALLOW.iter().any(|a| lower.contains(a)) || Self::DENY.iter().any(|d| lower.contains(d)) {
            return None;
        }
        Some(ServerLine(line.chars().take(160).collect()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for ServerLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

/// Lifecycle and request events for the helper log. More variants may be added, so callers
/// match with a wildcard arm (or just format with Debug).
#[derive(Debug)]
#[non_exhaustive]
pub enum EngineEvent {
    /// Runtime and model discovery.
    Discovery { runtime_found: bool, model_found: bool, known_digest: bool },
    /// A configured runtime dir or model file failed its checks (the search went on).
    DiscoveryRejected { what: &'static str },
    /// The model was given by path and its digest is not a known one.
    UnknownModel,
    /// `llama-server --help` probe (feature names only).
    FlagsProbe { cached: bool, lm_none: bool, fit: bool, api_key_file: bool, no_ui: bool, offline: bool, dev: bool },
    FlagsProbeFailed { reason: &'static str },
    /// Backend decision before a server start.
    DeviceChoice { backend: BackendPref, free_vram_mb: u32, avail_ram_mb: u32, threads: u32, reason: &'static str },
    ServerStarting { backend: BackendPref, port: u16, threads: u32, gpu_layers: u32 },
    ServerReady { ms: u64 },
    ServerStartFailed { backend: BackendPref, ms: u64, reason: &'static str },
    ServerExited { code: i32, expected: bool },
    /// An allowlisted llama-server line, logged only after a crash or a failed start.
    ServerOutput { line: ServerLine },
    WarmUp { ms: u64 },
    JobQueued { req_id: u16, input_bytes: u32, pairs: u8, ahead: u8 },
    /// `outcome`: "done", "same", "nothing", "cached".
    JobDone { req_id: u16, ms: u64, output_bytes: u32, outcome: &'static str },
    JobFailed { req_id: u16, kind: FailKind, ms: u64 },
    JobCancelled { req_id: u16 },
    PlaceholderRetry { req_id: u16 },
    /// The output was the input again and the line is not shown to be in the target language:
    /// one retry with a stronger instruction (E1b, V2).
    InsistRetry { req_id: u16 },
    Watchdog { action: &'static str },
    VramGuard { free_mb: u32, switched_to_cpu: bool },
    /// Two GPU failures or the VRAM guard: CPU for the rest of the session.
    GpuBlocked { reason: &'static str },
    Unload { reason: &'static str },
    State(EngineState),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_line_allowlist() {
        assert!(ServerLine::filter("load_backend: loaded CPU backend from ggml-cpu-haswell.dll").is_some());
        assert!(ServerLine::filter("main: server is listening on http://127.0.0.1:5000").is_some());
        assert!(ServerLine::filter("srv  log_server_r: request: POST /completion 127.0.0.1 200").is_none());
        assert!(ServerLine::filter("slot update_slots: id  0 | task 3 | prompt processing progress").is_none());
        assert!(ServerLine::filter("error: \"prompt\" must be a string").is_none());
        assert!(ServerLine::filter("failed: <|im_start|>user").is_none());
        let long = format!("error: {}", "x".repeat(400));
        assert_eq!(ServerLine::filter(&long).unwrap().as_str().chars().count(), 160);
        assert_eq!(format!("{:?}", ServerLine::filter(" error: a ").unwrap()), "\"error: a\"");
    }
}
