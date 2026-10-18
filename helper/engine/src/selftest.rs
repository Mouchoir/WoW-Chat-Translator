//! `--selftest`: discovery, flags, VRAM and RAM as JSON. Never loads the model.
//!
//! The flags probe runs `llama-server --help` (no model, no network) unless `flags.json` already
//! has the answer. The output is printed for the user; it contains local paths but never chat
//! text.

use serde_json::{json, Value};

use crate::device::{self, Devices, SystemDevices};
use crate::discover;
use crate::policy::{self, Choice};
use crate::{BackendPref, EngineConfig};

fn pref_name(p: BackendPref) -> &'static str {
    match p {
        BackendPref::Auto => "auto",
        BackendPref::Cpu => "cpu",
        BackendPref::Vulkan => "vulkan",
        BackendPref::Cuda => "cuda",
    }
}

pub fn run(cfg: &EngineConfig) -> Value {
    run_with(cfg, discover::discover(cfg.runtime_dir.as_deref(), cfg.model_path.as_deref()))
}

/// The selftest on a given discovery result (tests pass an empty one, so nothing is executed).
pub fn run_with(cfg: &EngineConfig, d: discover::Discovery) -> Value {
    let runtime = match &d.runtime {
        Some(r) => json!({
            "found": true,
            "dir": r.dir.to_string_lossy(),
            "vulkan": r.vulkan,
            "cuda": r.cuda.as_ref().and_then(|c| c.file_name()).map(|n| n.to_string_lossy().into_owned()),
        }),
        None => json!({"found": false}),
    };
    let model = match &d.model {
        Some(m) => json!({
            "found": true,
            "path": m.path.to_string_lossy(),
            "sizeMb": m.size / (1024 * 1024),
            "digestKnown": m.known,
        }),
        None => json!({"found": false}),
    };
    let flags = match &d.runtime {
        Some(r) => match crate::flags::probe(&r.exe, &r.dir, &cfg.data_dir) {
            Ok((f, cached)) => json!({
                "ok": f.usable().is_ok(),
                "cached": cached,
                "features": f.names.iter().collect::<Vec<_>>(),
            }),
            Err(reason) => json!({"ok": false, "error": reason}),
        },
        None => Value::Null,
    };
    let mut devs = SystemDevices::default();
    let snap = devs.snapshot();
    let adapters: Vec<Value> = device::adapters()
        .iter()
        .map(|a| json!({"name": a.name, "vendor": format!("{:04x}", a.vendor), "dedicatedMb": a.dedicated_mb, "software": a.software}))
        .collect();
    let decision = policy::choose(&policy::Input {
        pref: cfg.backend,
        gpu_blocked: false,
        adapter: snap.adapter.as_ref().map(device::AdapterInfo::summary),
        free_vram_mb: snap.free_vram_mb,
        avail_ram_mb: snap.avail_ram_mb,
        physical_cores: snap.physical_cores,
        cpu_threads: cfg.cpu_threads,
        gpu_margin_mb: cfg.gpu_margin_mb,
        has_vulkan: d.runtime.as_ref().is_some_and(|r| r.vulkan),
        has_cuda: d.runtime.as_ref().is_some_and(|r| r.cuda.is_some()),
    });
    let (backend, threads) = match decision.choice {
        Choice::Vulkan { threads } => ("vulkan", threads),
        Choice::Cuda { threads } => ("cuda", threads),
        Choice::Cpu { threads } => ("cpu", threads),
        Choice::LowMemory => ("lowmem", 0),
    };
    json!({
        "engine": {"promptVersion": crate::prompt::PROMPT_VERSION, "variant": format!("{:?}", crate::prompt::DEFAULT), "tuning": crate::pipeline::Tuning::DEFAULT.label()},
        "runtime": runtime,
        "model": model,
        "flags": flags,
        "gpu": {
            "adapters": adapters,
            "picked": snap.adapter.as_ref().map(|a| a.name.clone()),
            "freeVramMb": snap.free_vram_mb,
            "vramSource": snap.vram_source,
        },
        "ram": {"availMb": snap.avail_ram_mb},
        "cpu": {"physicalCores": snap.physical_cores},
        "policy": {"pref": pref_name(cfg.backend), "backend": backend, "threads": threads, "reason": decision.reason,
                   "gpuMarginMb": cfg.gpu_margin_mb},
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    fn config(dir: &std::path::Path) -> EngineConfig {
        EngineConfig {
            runtime_dir: Some(dir.join("none")),
            model_path: Some(dir.join("none.gguf")),
            backend: BackendPref::Auto,
            gpu_margin_mb: 1500,
            cpu_threads: 0,
            idle_unload: Duration::from_secs(600),
            keep_loaded: false,
            translator: crate::Translator::Llama,
            data_dir: dir.to_path_buf(),
            log: Arc::new(|_| {}),
        }
    }

    /// An empty discovery (no runtime, no model): nothing is executed, so this test never starts
    /// llama-server, whatever is installed on the machine. Every section must be present.
    #[test]
    fn selftest_shape_without_runtime() {
        let dir = crate::testutil::temp_dir("selftest");
        let v = run_with(&config(&dir), crate::discover::Discovery::default());
        for k in ["engine", "runtime", "model", "flags", "gpu", "ram", "cpu", "policy"] {
            assert!(v.get(k).is_some(), "{k}");
        }
        assert_eq!(v["runtime"]["found"], false);
        assert_eq!(v["model"]["found"], false);
        assert!(v["flags"].is_null(), "no runtime, no flags probe");
        assert!(v["ram"]["availMb"].as_u64().unwrap() > 0);
        assert!(v["cpu"]["physicalCores"].as_u64().unwrap() >= 1);
        assert_eq!(v["engine"]["promptVersion"], crate::prompt::PROMPT_VERSION);
        assert_eq!(v["engine"]["tuning"], crate::pipeline::Tuning::DEFAULT.label());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The real discovery on this machine. The fallback search may find a runtime, and then
    /// llama-server runs once with --help (no model, no network): only the engine and integration
    /// steps may start it (amendment A5).
    /// Run: cargo test --offline --lib selftest_on_this_machine -- --ignored
    #[test]
    #[ignore]
    fn selftest_on_this_machine() {
        let dir = crate::testutil::temp_dir("selftest-real");
        let v = run(&config(&dir));
        for k in ["engine", "runtime", "model", "flags", "gpu", "ram", "cpu", "policy"] {
            assert!(v.get(k).is_some(), "{k}");
        }
        if v["runtime"]["found"] == true {
            assert_eq!(v["flags"]["ok"], true, "{}", v["flags"]);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
