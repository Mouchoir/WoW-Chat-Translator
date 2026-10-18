//! Smoke test of the public Engine API against the real llama-server, on CPU.
//!
//! Ignored by default: it needs the llama.cpp runtime (env TPROBE_LLAMA_RUNTIME) and the Qwen3
//! model (Ollama manifests or env TPROBE_MODEL). Run from helper\engine:
//!   cargo test --offline --test real_server -- --ignored --nocapture
//! A 60 s watchdog exits the process (the job object then kills the server). The input is a
//! synthetic test line; translations are only compared, never printed.

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tp_engine::{BackendPref, ChatText, Engine, EngineConfig, EngineState, Job, JobKey, Status, Translator};

fn llama_server_running() -> bool {
    std::process::Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq llama-server.exe", "/NH"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_lowercase().contains("llama-server.exe"))
        .unwrap_or(false)
}

#[test]
#[ignore = "needs the llama.cpp runtime and the model; starts llama-server on CPU"]
fn real_server_cpu_smoke() {
    let Some(runtime) = std::env::var_os("TPROBE_LLAMA_RUNTIME").map(PathBuf::from) else {
        eprintln!("TPROBE_LLAMA_RUNTIME not set: skipped");
        return;
    };
    assert!(!llama_server_running(), "another llama-server is already running");
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(60));
        eprintln!("WATCHDOG: 60 s elapsed, exiting (the job object kills llama-server)");
        std::process::exit(3);
    });
    let data = std::env::temp_dir().join(format!("tp_engine_real_{}", std::process::id()));
    let events: Arc<Mutex<Vec<String>>> = Arc::default();
    let e2 = events.clone();
    let cfg = EngineConfig {
        runtime_dir: Some(runtime),
        model_path: std::env::var_os("TPROBE_MODEL").map(PathBuf::from),
        backend: BackendPref::Cpu,
        gpu_margin_mb: 1500,
        cpu_threads: 4,
        idle_unload: Duration::from_secs(600),
        keep_loaded: false,
        translator: Translator::Llama,
        data_dir: data.clone(),
        log: Arc::new(move |e| e2.lock().unwrap().push(format!("{e:?}"))),
    };
    let (tx, rx) = mpsc::channel::<(JobKey, String)>();
    let states: Arc<Mutex<Vec<EngineState>>> = Arc::default();
    let s2 = states.clone();
    let engine = Engine::start(
        cfg,
        Box::new(move |u| {
            let s = match &u.status {
                Status::Done(t) => format!("done:{}", t.as_str()),
                Status::Same => "same".into(),
                Status::Failed(k) => format!("failed:{k:?}"),
                Status::NotInstalled => "notinstalled".into(),
                Status::LowMemory => "lowmem".into(),
                Status::Nothing => "nothing".into(),
                Status::Partial(_) => "partial".into(),
                Status::Loading { .. } => "loading".into(),
                Status::Queued { .. } => "queued".into(),
                Status::Translating => "translating".into(),
            };
            let _ = tx.send((u.key, s));
        }),
        Box::new(move |s| s2.lock().unwrap().push(s)),
    );
    let h = engine.handle();
    let job = |req_id: u16| Job {
        key: JobKey { session: 7, req_id },
        target: "fr".into(),
        text: ChatText::new("wts \u{2}[Linen Cloth]\u{3} x20, anyone know where the bank is?".into()),
        pairs: vec![(ChatText::new("wts".into()), ChatText::new("vend".into()))],
        no_cache: false,
    };
    let t0 = Instant::now();
    h.submit(job(1));
    let mut seen = Vec::new();
    let mut first = None;
    while first.is_none() && t0.elapsed() < Duration::from_secs(50) {
        if let Ok((k, s)) = rx.recv_timeout(Duration::from_millis(200)) {
            if k.req_id == 1 {
                if s.starts_with("done:") || s == "same" || s.starts_with("failed") || s == "notinstalled" || s == "lowmem" {
                    first = Some(s.clone());
                }
                seen.push(if s.starts_with("done:") { "done".to_string() } else { s });
            }
        }
    }
    let first_ms = t0.elapsed().as_millis();
    let first = first.expect("no final status within 50 s");
    eprintln!("statuses: {seen:?}; first result after {first_ms} ms");
    assert!(first.starts_with("done:"), "got {}", first.split(':').next().unwrap_or(""));
    assert!(first.contains("[Linen Cloth]"), "item name restored");
    assert!(seen.contains(&"loading".to_string()) && seen.contains(&"translating".to_string()));
    // Same text, new reqId: served from the cache.
    let t1 = Instant::now();
    h.submit(job(2));
    let (k, s) = rx.recv_timeout(Duration::from_secs(5)).expect("cached result");
    assert_eq!(k.req_id, 2);
    assert_eq!(s, first, "cache returns the same translation");
    eprintln!("cached result after {} ms", t1.elapsed().as_millis());
    assert!(states.lock().unwrap().contains(&EngineState::ReadyCpu));
    let t2 = Instant::now();
    drop(engine);
    eprintln!("shutdown in {} ms", t2.elapsed().as_millis());
    assert!(t2.elapsed() < Duration::from_secs(2));
    std::thread::sleep(Duration::from_millis(300));
    assert!(!llama_server_running(), "llama-server still running after shutdown");
    let ev = events.lock().unwrap();
    assert!(ev.iter().any(|e| e.contains("ServerReady")), "{ev:?}");
    for e in ev.iter().filter(|e| !e.starts_with("JobQueued") && !e.starts_with("JobDone")) {
        eprintln!("event: {e}");
    }
    let leftover: Vec<_> = std::fs::read_dir(data.join("run")).map(|d| d.flatten().map(|e| e.file_name()).collect()).unwrap_or_default();
    assert!(leftover.iter().all(|n| !n.to_string_lossy().starts_with("key-")), "key file left: {leftover:?}");
    let _ = std::fs::remove_dir_all(&data);
}
