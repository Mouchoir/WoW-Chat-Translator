//! Engine configuration, built by the app from settings.json and the command line.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::EngineEvent;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BackendPref {
    Auto,
    Cpu,
    Vulkan,
    Cuda,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Translator {
    /// llama-server with the Qwen3 model.
    Llama,
    /// Test translator: Loading for 300 ms on the first job, then Done(uppercase of the
    /// unmarked text) after 200 ms. Never starts a process.
    Echo,
}

pub struct EngineConfig {
    /// `--runtime`, else `settings.runtimeDir`; None lets discovery search the default places.
    pub runtime_dir: Option<PathBuf>,
    /// `--model`, else `settings.model`; None scans the Ollama manifests.
    pub model_path: Option<PathBuf>,
    pub backend: BackendPref,
    /// Free VRAM required on top of the model (default 1500).
    pub gpu_margin_mb: u32,
    /// 0 = clamp(physical cores / 2, 2, 4).
    pub cpu_threads: u32,
    pub idle_unload: Duration,
    pub keep_loaded: bool,
    pub translator: Translator,
    /// `%LOCALAPPDATA%\TranslateProbe` (holds `run\`).
    pub data_dir: PathBuf,
    /// Log sink. Events never carry text.
    pub log: Arc<dyn Fn(EngineEvent) + Send + Sync>,
}
