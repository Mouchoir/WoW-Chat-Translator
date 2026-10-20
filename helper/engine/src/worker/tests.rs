//! Lifecycle state machine against a fake server: an in-process HTTP stub (testutil::FakeHttp)
//! plus fake spawner, devices, discovery and flags. Timings are shrunk (see `fast`).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::*;
use crate::device::{AdapterInfo, Snapshot};
use crate::discover::{Discovery, Runtime};
use crate::model::ModelFile;
use crate::server::LaunchSpec;
use crate::remote::fake::{FakeReply, FakeTransport};
use crate::testutil::{temp_dir, FakeHttp, Reply};
use crate::{BackendPref, EngineConfig, EngineState, JobKey, Status, Translator};

const HELP_070: &str = include_str!("../../tests/fixtures/help_0_7_0.txt");

fn fast() -> Timing {
    Timing {
        health_every: Duration::from_millis(20),
        health_timeout: Duration::from_millis(200),
        ready_deadline: Duration::from_millis(600),
        ready_deadline_first_vulkan: Duration::from_millis(800),
        cpu_base: Duration::from_millis(500),
        cpu_tokens_per_s: 1e9,
        gpu_base: Duration::from_millis(500),
        gpu_tokens_per_s: 1e9,
        wow_gone_unload: Duration::from_millis(300),
        error_retry: Duration::from_millis(500),
        crash_window: Duration::from_secs(10),
        crash_hold: Duration::from_millis(800),
        crash_limit: 3,
        gpu_failure_limit: 2,
        vram_every: Duration::from_millis(100),
        vram_min_mb: 700,
        max_age: Duration::from_secs(30),
        echo_load: crate::echo::LOAD,
        echo_work: crate::echo::WORK,
        rediscover_every: Duration::from_millis(100),
        remote: remote::Policy {
            connect: Duration::from_millis(300),
            total: Duration::from_millis(800),
            retry_wait: Duration::from_millis(10),
            retry_floor: Duration::from_millis(100),
        },
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    Ok,
    /// /health answers 503 for this server.
    Hang,
    /// spawn() fails.
    Fail,
}

#[derive(Default)]
struct ProcState {
    exited: Option<i32>,
    killed: bool,
}

struct FakeProc {
    port: u16,
    st: Arc<Mutex<ProcState>>,
}

impl ServerProc for FakeProc {
    fn port(&self) -> u16 {
        self.port
    }
    fn exit_code(&mut self) -> Option<i32> {
        self.st.lock().unwrap().exited
    }
    fn kill(&mut self) {
        let mut s = self.st.lock().unwrap();
        s.killed = true;
        s.exited.get_or_insert(-1);
    }
    fn killer(&self) -> Arc<dyn Fn() + Send + Sync> {
        let st = self.st.clone();
        Arc::new(move || {
            let mut s = st.lock().unwrap();
            s.killed = true;
            s.exited.get_or_insert(-1);
        })
    }
    fn ring(&self) -> Vec<crate::ServerLine> {
        crate::ServerLine::filter("error: fake server line").into_iter().collect()
    }
}

#[derive(Clone)]
struct Spawns {
    specs: Arc<Mutex<Vec<LaunchSpec>>>,
    procs: Arc<Mutex<Vec<Arc<Mutex<ProcState>>>>>,
    modes: Arc<Mutex<VecDeque<Mode>>>,
    attempts: Arc<AtomicU32>,
}

struct FakeSpawner {
    http: Arc<FakeHttp>,
    s: Spawns,
}

impl Spawner for FakeSpawner {
    fn spawn(&mut self, spec: &LaunchSpec) -> Result<Box<dyn ServerProc>, &'static str> {
        self.s.attempts.fetch_add(1, Ordering::SeqCst);
        let mode = self.s.modes.lock().unwrap().pop_front().unwrap_or(Mode::Ok);
        if mode == Mode::Fail {
            return Err("fake spawn failure");
        }
        self.http.set_health(if mode == Mode::Hang { 503 } else { 200 });
        self.s.specs.lock().unwrap().push(spec.clone());
        let st = Arc::new(Mutex::new(ProcState::default()));
        self.s.procs.lock().unwrap().push(st.clone());
        Ok(Box::new(FakeProc { port: self.http.port(), st }))
    }
}

#[derive(Clone)]
struct Dev {
    snap: Arc<Mutex<Snapshot>>,
    free: Arc<Mutex<Option<u32>>>,
}

struct FakeDevices(Dev);

impl Devices for FakeDevices {
    fn snapshot(&mut self) -> Snapshot {
        self.0.snap.lock().unwrap().clone()
    }
    fn free_vram_mb(&mut self) -> Option<u32> {
        *self.0.free.lock().unwrap()
    }
    fn vulkan_device(&mut self, _rt: &Runtime, _name: &str) -> Option<String> {
        Some("Vulkan0".into())
    }
}

fn rtx() -> AdapterInfo {
    AdapterInfo { name: "NVIDIA GeForce RTX 3080".into(), luid_high: 0, luid_low: 1, vendor: 0x10DE, device: 0x2206, subsys: 0, dedicated_mb: 10240, software: false }
}

fn cpu_only() -> Snapshot {
    Snapshot { adapter: None, free_vram_mb: None, vram_source: "none", avail_ram_mb: 8000, physical_cores: 8 }
}

fn gpu_ok() -> Snapshot {
    Snapshot { adapter: Some(rtx()), free_vram_mb: Some(6000), vram_source: "nvml", avail_ram_mb: 8000, physical_cores: 8 }
}

struct Opts {
    translator: Translator,
    snap: Snapshot,
    installed: bool,
    keep_loaded: bool,
    idle_unload: Duration,
    modes: Vec<Mode>,
    timing: Timing,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            translator: Translator::Llama,
            snap: cpu_only(),
            installed: true,
            keep_loaded: false,
            idle_unload: Duration::from_secs(60),
            modes: vec![],
            timing: fast(),
        }
    }
}

type Log = Arc<Mutex<Vec<(JobKey, String)>>>;

struct H {
    engine: Option<Engine>,
    updates: Log,
    states: Arc<Mutex<Vec<EngineState>>>,
    events: Arc<Mutex<Vec<String>>>,
    /// Every fallback reason an update carried (the badge tooltip).
    fbs: Arc<Mutex<Vec<(JobKey, remote::Fallback)>>>,
    http: Arc<FakeHttp>,
    s: Spawns,
    dev: Dev,
    dir: PathBuf,
    remote: Arc<FakeTransport>,
}

/// The status, then `|fb:<kind>:<status>` when the update carries a fallback reason.
fn st_fb(u: &crate::Update) -> String {
    let mut s = st(&u.status);
    if let Some(f) = &u.fallback {
        s.push_str(&format!("|fb:{}:{}", f.kind.code(), f.status.unwrap_or(0)));
    }
    s
}

fn st(s: &Status) -> String {
    match s {
        Status::Queued { ahead } => format!("queued:{ahead}"),
        Status::Loading { first_gpu_use: false } => "loading".into(),
        Status::Loading { first_gpu_use: true } => "loading_gpu_first".into(),
        Status::Translating => "translating".into(),
        Status::Partial(t) => format!("partial:{}", t.as_str()),
        Status::Done(t) => format!("done:{}", t.as_str()),
        Status::Same => "same".into(),
        Status::Nothing => "nothing".into(),
        Status::Failed(k) => format!("failed:{k:?}"),
        Status::LowMemory => "lowmem".into(),
        Status::NotInstalled => "notinstalled".into(),
    }
}

fn key(session: u16, req_id: u16) -> JobKey {
    JobKey { session, req_id }
}

impl H {
    fn new(o: Opts) -> H {
        let dir = temp_dir("worker");
        let http = Arc::new(FakeHttp::start());
        // Default model behaviour: warm-up gets "", a message gets its placeholders back.
        http.respond_with(|b| {
            let prompt = b["prompt"].as_str().unwrap_or("");
            if !prompt.contains("Message:\n") {
                return Reply::Json(String::new());
            }
            if b["stream"] == true {
                Reply::Stream(vec!["Vends ".into(), "{1}".into(), " pas cher".into()], Duration::from_millis(30))
            } else {
                Reply::Json("Vends {1} pas cher".into())
            }
        });
        let s = Spawns {
            specs: Arc::default(),
            procs: Arc::default(),
            modes: Arc::new(Mutex::new(o.modes.into_iter().collect())),
            attempts: Arc::default(),
        };
        let dev = Dev { snap: Arc::new(Mutex::new(o.snap)), free: Arc::new(Mutex::new(Some(6000))) };
        let rt_dir = dir.join("runtime");
        std::fs::create_dir_all(rt_dir.join("vulkan")).unwrap();
        let rt = Runtime { dir: rt_dir.clone(), exe: rt_dir.join("llama-server.exe"), vulkan: true, cuda: Some(rt_dir.join("cuda_v12")) };
        let model = ModelFile {
            path: dir.join("model.gguf"),
            size: 2_497_280_256,
            digest: Some("sha256:7485fe6f11af29433bc51cab58009521f205840f5b4ae3a32fa7f92e8534fdf5".into()),
            known: true,
            mtime_secs: 0,
        };
        let installed = o.installed;
        let remote = Arc::new(FakeTransport::default());
        let deps = Deps {
            spawner: Box::new(FakeSpawner { http: http.clone(), s: s.clone() }),
            devices: Box::new(FakeDevices(dev.clone())),
            discover: Box::new(move || Discovery { runtime: Some(rt.clone()), model: installed.then(|| model.clone()), explicit_rejected: false }),
            flags: Box::new(|_| Ok((flags::features(&flags::parse_help(HELP_070)), true))),
            transport: remote.clone(),
        };
        let updates: Log = Arc::default();
        let states: Arc<Mutex<Vec<EngineState>>> = Arc::default();
        let events: Arc<Mutex<Vec<String>>> = Arc::default();
        let fbs: Arc<Mutex<Vec<(JobKey, remote::Fallback)>>> = Arc::default();
        let (u2, s2, e2, f2) = (updates.clone(), states.clone(), events.clone(), fbs.clone());
        let cfg = EngineConfig {
            runtime_dir: None,
            model_path: None,
            backend: BackendPref::Auto,
            gpu_margin_mb: 1500,
            cpu_threads: 0,
            idle_unload: o.idle_unload,
            keep_loaded: o.keep_loaded,
            translator: o.translator,
            data_dir: dir.clone(),
            log: Arc::new(move |e| e2.lock().unwrap().push(format!("{e:?}"))),
        };
        let engine = Engine::start_with(
            cfg,
            deps,
            o.timing,
            Box::new(move |u| {
                if let Some(fb) = &u.fallback {
                    f2.lock().unwrap().push((u.key, fb.clone()));
                }
                u2.lock().unwrap().push((u.key, st_fb(&u)));
            }),
            Box::new(move |s| s2.lock().unwrap().push(s)),
        );
        H { engine: Some(engine), updates, states, events, fbs, http, s, dev, dir, remote }
    }

    fn h(&self) -> EngineHandle {
        self.engine.as_ref().unwrap().handle()
    }

    fn submit(&self, k: JobKey, text: &str) {
        self.h().submit(job(k.session, k.req_id, "fr", text, &[("wts", "vend")], false));
    }

    fn of(&self, k: JobKey) -> Vec<String> {
        self.updates.lock().unwrap().iter().filter(|(x, _)| *x == k).map(|(_, s)| s.clone()).collect()
    }

    fn last(&self, k: JobKey) -> Option<String> {
        self.of(k).last().cloned()
    }

    fn wait(&self, what: &str, timeout: Duration, f: impl Fn(&H) -> bool) {
        let t = Instant::now();
        while t.elapsed() < timeout {
            if f(self) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timeout waiting for {what}; updates={:?} events={:?}", self.updates.lock().unwrap(), self.events.lock().unwrap());
    }

    fn wait_final(&self, k: JobKey, timeout: Duration) -> String {
        self.wait("final status", timeout, |h| h.last(k).is_some_and(|s| is_final(&s)));
        self.last(k).unwrap()
    }

    fn has_event(&self, needle: &str) -> bool {
        self.events.lock().unwrap().iter().any(|e| e.contains(needle))
    }

    fn state(&self) -> Option<EngineState> {
        self.states.lock().unwrap().last().copied()
    }

    fn spec(&self, i: usize) -> LaunchSpec {
        self.s.specs.lock().unwrap()[i].clone()
    }

    fn proc_killed(&self, i: usize) -> bool {
        self.s.procs.lock().unwrap()[i].lock().unwrap().killed
    }
}

impl Drop for H {
    fn drop(&mut self) {
        drop(self.engine.take());
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn is_final(s: &str) -> bool {
    let s = s.split("|fb:").next().unwrap_or(s);
    !(s.starts_with("queued") || s.starts_with("loading") || s == "translating" || s.starts_with("partial"))
}

fn args(spec: &LaunchSpec) -> Vec<String> {
    spec.args.iter().map(|a| a.to_string_lossy().into_owned()).collect()
}

fn has_pair(a: &[String], k: &str, v: &str) -> bool {
    a.windows(2).any(|w| w[0] == k && w[1] == v)
}

const MSG: &str = "wts \u{2}[Linen Cloth]\u{3} cheap";
const SECS: Duration = Duration::from_secs(5);

#[test]
fn echo_translator() {
    let h = H::new(Opts { translator: Translator::Echo, ..Default::default() });
    let t = Instant::now();
    h.submit(key(1, 1), MSG);
    assert_eq!(h.wait_final(key(1, 1), SECS), "done:WTS [LINEN CLOTH] CHEAP");
    assert!(t.elapsed() >= Duration::from_millis(480), "300 ms loading + 200 ms work");
    assert_eq!(h.of(key(1, 1)), vec!["queued:0", "loading", "translating", "done:WTS [LINEN CLOTH] CHEAP"]);
    h.submit(key(1, 2), "second line");
    assert_eq!(h.wait_final(key(1, 2), SECS), "done:SECOND LINE");
    assert!(!h.of(key(1, 2)).contains(&"loading".to_string()), "loaded once");
    // Same text again: served from the cache at once.
    h.submit(key(1, 3), MSG);
    h.wait("cached", Duration::from_millis(300), |h| h.last(key(1, 3)).as_deref() == Some("done:WTS [LINEN CLOTH] CHEAP"));
    assert_eq!(h.of(key(1, 3)).len(), 1);
    assert!(h.has_event("outcome: \"cached\""));
    let states = h.states.lock().unwrap().clone();
    assert_eq!(&states[..4], &[EngineState::Unloaded, EngineState::Loading, EngineState::Busy, EngineState::ReadyCpu]);
    assert_eq!(h.s.attempts.load(Ordering::SeqCst), 0, "echo never starts a process");
}

#[test]
fn llama_cpu_happy_path() {
    let h = H::new(Opts::default());
    h.http.push(Reply::Json(String::new()));
    h.http.push(Reply::Stream(vec!["Vends ".into(), "{1}".into(), " pas cher".into()], Duration::from_millis(120)));
    h.submit(key(1, 1), MSG);
    assert_eq!(h.wait_final(key(1, 1), SECS), "done:Vends [Linen Cloth] pas cher");
    let u = h.of(key(1, 1));
    assert_eq!(&u[..3], &["queued:0", "loading", "translating"]);
    assert!(u.iter().any(|s| s.starts_with("partial:")), "streamed on CPU: {u:?}");
    let spec = h.spec(0);
    let a = args(&spec);
    assert!(has_pair(&a, "-ngl", "0") && has_pair(&a, "-t", "4") && has_pair(&a, "-tb", "4"));
    assert!(has_pair(&a, "-c", "1024") && has_pair(&a, "-np", "1") && has_pair(&a, "--host", "127.0.0.1"));
    assert!(has_pair(&a, "--fit", "off") && has_pair(&a, "-lm", "none"));
    assert!(a.contains(&"--no-ui".to_string()) && a.contains(&"--offline".to_string()));
    assert!(!a.contains(&"-dev".to_string()));
    assert_eq!(spec.env, vec![("CUDA_VISIBLE_DEVICES".to_string(), "-1".to_string())]);
    assert_eq!(spec.cwd, h.dir.join("runtime"));
    // API key in a file, never on the command line.
    let kf = spec.key_file.clone().unwrap();
    assert!(has_pair(&a, "--api-key-file", &kf.to_string_lossy()));
    assert!(!a.contains(&"--api-key".to_string()));
    let key_text = std::fs::read_to_string(&kf).unwrap();
    assert_eq!(h.http.last_auth().unwrap(), format!("Bearer {}", key_text.trim()));
    assert_eq!(key_text.trim().len(), 32);
    // Warm-up first (n_predict 1, system prefix only), then the request.
    let bodies = h.http.bodies();
    assert_eq!(bodies[0]["n_predict"], 1);
    assert!(!bodies[0]["prompt"].as_str().unwrap().contains("Message:"));
    let b = &bodies[1];
    assert_eq!(b["grammar"], crate::grammar::LATIN);
    assert_eq!(b["stream"], true);
    assert_eq!(b["seed"], 42);
    assert!(b["prompt"].as_str().unwrap().contains("Glossary:\n- \"wts\" = \"vend\"\n\nMessage:\nwts {1} cheap<|im_end|>"));
    h.wait("ready cpu", SECS, |h| h.state() == Some(EngineState::ReadyCpu));
    // Cached: same text, other reqId.
    h.submit(key(1, 2), MSG);
    h.wait("cached", Duration::from_millis(500), |h| h.last(key(1, 2)).as_deref() == Some("done:Vends [Linen Cloth] pas cher"));
    // noCache: asks the model again.
    let n = h.http.completions();
    h.h().submit(job(1, 3, "fr", MSG, &[("wts", "vend")], true));
    h.wait_final(key(1, 3), SECS);
    assert_eq!(h.http.completions(), n + 1);
    // Shutdown kills the server and deletes the key file.
    let procs = h.s.procs.clone();
    let mut h = h;
    drop(h.engine.take());
    assert!(procs.lock().unwrap()[0].lock().unwrap().killed);
    assert!(!kf.exists());
}

#[test]
fn placeholder_retry_uses_brackets() {
    let h = H::new(Opts::default());
    h.http.respond_with(|b| {
        let p = b["prompt"].as_str().unwrap_or("");
        if !p.contains("Message:\n") {
            Reply::Json(String::new())
        } else if p.contains("every token like {1}") {
            Reply::Json("Vends [Linen Cloth] pas cher".into()) // placeholder lost
        } else {
            Reply::Json("Vends [Linen Cloth] pas cher !".into())
        }
    });
    h.submit(key(1, 1), MSG);
    assert_eq!(h.wait_final(key(1, 1), SECS), "done:Vends [Linen Cloth] pas cher !");
    assert!(h.has_event("PlaceholderRetry { req_id: 1 }"));
    let bodies = h.http.bodies();
    assert!(bodies.last().unwrap()["prompt"].as_str().unwrap().contains("Message:\nwts [Linen Cloth] cheap<|im_end|>"));
}

#[test]
fn default_tuning_expands_and_never_says_same_without_evidence() {
    let h = H::new(Opts::default());
    h.http.respond_with(|b| {
        let p = b["prompt"].as_str().unwrap_or("");
        if !p.contains("Message:\n") {
            Reply::Json(String::new())
        } else {
            // The model sends the (expanded) message back unchanged.
            let msg = p.split("Message:\n").nth(1).unwrap_or("").split("<|im_end|>").next().unwrap_or("");
            Reply::Json(msg.to_string())
        }
    });
    // An English line for a French reader: brb is must-use, so it is expanded before the call.
    h.h().submit(job(1, 1, "fr", "brb 5 min, need to feed the cat", &[("brb", "je reviens")], false));
    assert_eq!(h.wait_final(key(1, 1), SECS), "done:je reviens 5 min, need to feed the cat");
    let bodies = h.http.bodies();
    let p = bodies.last().unwrap()["prompt"].as_str().unwrap().to_string();
    assert!(p.contains("the way a French player would write in chat"), "casual style");
    assert!(p.contains("Message:\nje reviens 5 min, need to feed the cat<|im_end|>"));
    assert!(!p.contains("Glossary:"), "the expanded pair is not listed again");
    assert_eq!(h.http.completions(), 2, "warm-up and one request: no insist retry by default");
    // A French line echoed for a French reader: Same.
    h.h().submit(job(1, 2, "fr", "salut tout le monde, on fait le donjon ?", &[], false));
    assert_eq!(h.wait_final(key(1, 2), SECS), "same");
}

#[test]
fn bad_output_and_http_error() {
    let h = H::new(Opts::default());
    h.http.push(Reply::Json(String::new())); // warm-up
    h.http.push(Reply::Json("<|im_start|>assistant".into()));
    h.submit(key(1, 1), MSG);
    assert_eq!(h.wait_final(key(1, 1), SECS), "failed:BadOutput");
    h.http.push(Reply::Status(500));
    h.submit(key(1, 2), "another line");
    assert_eq!(h.wait_final(key(1, 2), SECS), "failed:Http");
}

#[test]
fn gpu_start_hangs_then_cpu_fallback() {
    let h = H::new(Opts { snap: gpu_ok(), modes: vec![Mode::Hang, Mode::Ok], ..Default::default() });
    h.submit(key(1, 1), MSG);
    assert_eq!(h.wait_final(key(1, 1), SECS), "done:Vends [Linen Cloth] pas cher");
    let u = h.of(key(1, 1));
    assert!(u.contains(&"loading_gpu_first".to_string()) && u.contains(&"loading".to_string()), "{u:?}");
    let v = h.spec(0);
    let a = args(&v);
    assert_eq!(v.cwd, h.dir.join("runtime").join("vulkan"));
    assert!(has_pair(&a, "-ngl", "999") && has_pair(&a, "-dev", "Vulkan0"));
    assert!(v.env.is_empty());
    let c = h.spec(1);
    assert!(has_pair(&args(&c), "-ngl", "0"));
    assert!(h.proc_killed(0));
    assert!(h.has_event("reason: \"health_timeout\""));
    assert!(h.has_event("ServerOutput"), "ring logged after a failed start");
    // The warm-up on CPU does not create the Vulkan marker.
    assert!(!crate::device::vulkan_marker(&h.dir).exists());
}

#[test]
fn vulkan_warmup_marks_first_use_done() {
    let h = H::new(Opts { snap: gpu_ok(), ..Default::default() });
    h.submit(key(1, 1), MSG);
    assert_eq!(h.wait_final(key(1, 1), SECS), "done:Vends [Linen Cloth] pas cher");
    assert!(h.of(key(1, 1)).contains(&"loading_gpu_first".to_string()));
    assert!(crate::device::vulkan_marker(&h.dir).exists());
    h.wait("ready gpu", SECS, |h| h.state() == Some(EngineState::ReadyGpu));
    // GPU requests are not streamed.
    assert_eq!(h.http.bodies().last().unwrap()["stream"], false);
    h.h().unload_now();
    h.wait("unload", SECS, |h| h.has_event("Unload { reason: \"tray\" }"));
    h.submit(key(1, 2), "next line");
    h.wait_final(key(1, 2), SECS);
    assert!(h.of(key(1, 2)).contains(&"loading".to_string()), "marker present: not first use");
}

#[test]
fn cpu_start_failure_enters_error_then_retries() {
    let h = H::new(Opts { modes: vec![Mode::Fail], ..Default::default() });
    h.submit(key(1, 1), MSG);
    assert_eq!(h.wait_final(key(1, 1), SECS), "failed:ServerDied");
    h.wait("error state", SECS, |h| h.state() == Some(EngineState::Error));
    h.submit(key(1, 2), "second");
    assert_eq!(h.wait_final(key(1, 2), SECS), "failed:ServerDied");
    assert_eq!(h.s.attempts.load(Ordering::SeqCst), 1, "no retry inside the 60 s window");
    std::thread::sleep(Duration::from_millis(600));
    h.submit(key(1, 3), MSG);
    assert_eq!(h.wait_final(key(1, 3), SECS), "done:Vends [Linen Cloth] pas cher");
    assert_eq!(h.s.attempts.load(Ordering::SeqCst), 2);
}

#[test]
fn crashes_during_requests_and_crash_limit() {
    let h = H::new(Opts::default());
    for i in 0..3u16 {
        h.http.push(Reply::Json(String::new())); // warm-up of each new server
        h.http.push(Reply::Hang);
        let n = h.http.completions();
        h.submit(key(1, 10 + i), &format!("crash line {i}"));
        h.wait("request sent", SECS, |h| h.http.completions() >= n + 2);
        let procs = h.s.procs.lock().unwrap().clone();
        procs.last().unwrap().lock().unwrap().exited = Some(3);
        assert_eq!(h.wait_final(key(1, 10 + i), SECS), "failed:ServerDied");
    }
    h.wait("error state", SECS, |h| h.state() == Some(EngineState::Error));
    assert!(h.has_event("ServerExited { code: 3, expected: false }"));
    h.submit(key(1, 20), "during the hold");
    assert_eq!(h.wait_final(key(1, 20), SECS), "failed:ServerDied");
    assert_eq!(h.s.attempts.load(Ordering::SeqCst), 3);
    std::thread::sleep(Duration::from_millis(900));
    h.submit(key(1, 21), MSG);
    assert_eq!(h.wait_final(key(1, 21), SECS), "done:Vends [Linen Cloth] pas cher");
}

#[test]
fn request_timeout_kills_and_restarts_lazily() {
    let h = H::new(Opts::default());
    h.http.push(Reply::Json(String::new()));
    h.http.push(Reply::Delay(Duration::from_millis(1500), Box::new(Reply::Json("late".into()))));
    let t = Instant::now();
    h.submit(key(1, 1), "slow line");
    assert_eq!(h.wait_final(key(1, 1), SECS), "failed:Timeout");
    assert!(t.elapsed() < Duration::from_millis(1400), "deadline, not the reply");
    assert!(h.proc_killed(0));
    assert!(h.has_event("request_timeout"));
    h.wait("unloaded", SECS, |h| h.state() == Some(EngineState::Unloaded));
    h.submit(key(1, 2), MSG);
    assert_eq!(h.wait_final(key(1, 2), SECS), "done:Vends [Linen Cloth] pas cher");
    assert_eq!(h.s.specs.lock().unwrap().len(), 2);
}

#[test]
fn two_gpu_failures_block_the_gpu() {
    let h = H::new(Opts { snap: gpu_ok(), ..Default::default() });
    for i in 0..2u16 {
        h.http.push(Reply::Json(String::new()));
        h.http.push(Reply::Delay(Duration::from_millis(1500), Box::new(Reply::Json("late".into()))));
        h.submit(key(1, i + 1), &format!("slow {i}"));
        assert_eq!(h.wait_final(key(1, i + 1), SECS), "failed:Timeout");
    }
    assert!(h.has_event("GpuBlocked { reason: \"failures\" }"));
    h.submit(key(1, 9), MSG);
    h.wait_final(key(1, 9), SECS);
    let specs = h.s.specs.lock().unwrap().clone();
    assert!(has_pair(&args(&specs[0]), "-ngl", "999") && has_pair(&args(&specs[1]), "-ngl", "999"));
    assert!(has_pair(&args(&specs[2]), "-ngl", "0"));
}

#[test]
fn idle_unload_and_keep_loaded() {
    let h = H::new(Opts { idle_unload: Duration::from_millis(300), ..Default::default() });
    h.submit(key(1, 1), MSG);
    h.wait_final(key(1, 1), SECS);
    h.wait("idle unload", SECS, |h| h.has_event("Unload { reason: \"idle\" }"));
    assert!(h.proc_killed(0));
    h.wait("unloaded", SECS, |h| h.state() == Some(EngineState::Unloaded));

    let k = H::new(Opts { idle_unload: Duration::from_millis(300), keep_loaded: true, ..Default::default() });
    k.submit(key(1, 1), MSG);
    k.wait_final(key(1, 1), SECS);
    std::thread::sleep(Duration::from_millis(700));
    assert!(!k.has_event("Unload {"), "events={:?}", k.events.lock().unwrap());
    assert!(!k.proc_killed(0));
}

#[test]
fn wow_gone_unloads_after_delay() {
    let h = H::new(Opts { keep_loaded: true, ..Default::default() });
    h.submit(key(1, 1), MSG);
    h.wait_final(key(1, 1), SECS);
    h.h().set_wow_present(false);
    std::thread::sleep(Duration::from_millis(150));
    assert!(!h.has_event("wow_gone"));
    h.wait("wow_gone unload", SECS, |h| h.has_event("Unload { reason: \"wow_gone\" }"));
    // Back before the delay: nothing happens.
    let g = H::new(Opts { keep_loaded: true, ..Default::default() });
    g.submit(key(1, 1), MSG);
    g.wait_final(key(1, 1), SECS);
    g.h().set_wow_present(false);
    std::thread::sleep(Duration::from_millis(100));
    g.h().set_wow_present(true);
    std::thread::sleep(Duration::from_millis(500));
    assert!(!g.has_event("wow_gone"));
}

#[test]
fn vram_pressure_switches_session_to_cpu() {
    let h = H::new(Opts { snap: gpu_ok(), keep_loaded: true, ..Default::default() });
    h.submit(key(1, 1), MSG);
    h.wait_final(key(1, 1), SECS);
    *h.dev.free.lock().unwrap() = Some(500);
    h.wait("vram guard", SECS, |h| h.has_event("Unload { reason: \"vram\" }"));
    assert!(h.has_event("VramGuard { free_mb: 500, switched_to_cpu: true }"));
    assert!(h.has_event("GpuBlocked { reason: \"vram\" }"));
    h.submit(key(1, 2), "after the guard");
    h.wait_final(key(1, 2), SECS);
    assert!(has_pair(&args(&h.spec(1)), "-ngl", "0"));
    // set_backend resets the block: the next start may use the GPU again.
    h.h().set_backend(BackendPref::Vulkan);
    h.wait("backend unload", SECS, |h| h.has_event("Unload { reason: \"backend\" }"));
    *h.dev.free.lock().unwrap() = Some(6000);
    h.submit(key(1, 3), "third");
    h.wait_final(key(1, 3), SECS);
    assert!(has_pair(&args(&h.spec(2)), "-ngl", "999"));
}

#[test]
fn queue_rules_overload_cancel_clear_merge() {
    let h = H::new(Opts { modes: vec![Mode::Hang], timing: Timing { ready_deadline: Duration::from_secs(3), ..fast() }, ..Default::default() });
    h.submit(key(1, 1), "first");
    h.wait("loading", SECS, |h| h.last(key(1, 1)).as_deref() == Some("loading"));
    for i in 2..=10u16 {
        h.submit(key(1, i), &format!("line {i}"));
    }
    // 1 in flight, 8 waiting: the 10th pushed out the oldest waiting job (2).
    h.wait("overload", SECS, |h| h.last(key(1, 2)).as_deref() == Some("failed:Overloaded"));
    assert_eq!(h.last(key(1, 10)).as_deref(), Some("queued:8"));
    // Duplicate text from another session merges with job 4.
    h.submit(key(2, 1), "line 4");
    h.wait("merged", SECS, |h| h.last(key(2, 1)).as_deref() == Some("queued:2"));
    // Cancel job 3; clear session 2 (only the merged key belongs to it).
    h.h().cancel(key(1, 3));
    h.h().clear_session(2);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(h.last(key(1, 4)).as_deref(), Some("queued:1"), "job 3 left the queue");
    h.http.set_health(200);
    for i in [1u16, 4, 5, 6, 7, 8, 9, 10] {
        assert!(h.wait_final(key(1, i), SECS).starts_with("done:"), "job {i}");
    }
    std::thread::sleep(Duration::from_millis(100));
    assert!(h.of(key(1, 3)).iter().all(|s| !s.starts_with("done")), "cancelled job got a result");
    assert!(h.of(key(2, 1)).iter().all(|s| !s.starts_with("done")), "cleared job got a result");
    // Re-submitting a key in progress is ignored; after it finished, it is served from the cache.
    h.submit(key(1, 1), "first");
    h.wait("cached again", SECS, |h| h.of(key(1, 1)).iter().filter(|s| s.starts_with("done:")).count() == 2);
}

#[test]
fn merged_in_flight_duplicate_gets_the_result() {
    let h = H::new(Opts::default());
    h.http.push(Reply::Json(String::new()));
    h.http.push(Reply::Delay(Duration::from_millis(200), Box::new(Reply::Json("Vends {1} pas cher".into()))));
    h.submit(key(1, 1), MSG);
    h.wait("translating", SECS, |h| h.last(key(1, 1)).as_deref() == Some("translating"));
    h.submit(key(1, 2), MSG);
    assert_eq!(h.wait_final(key(1, 2), SECS), "done:Vends [Linen Cloth] pas cher");
    assert_eq!(h.last(key(1, 1)).as_deref(), Some("done:Vends [Linen Cloth] pas cher"));
    // Cancel while in flight: the result is discarded for that key only.
    h.http.push(Reply::Delay(Duration::from_millis(200), Box::new(Reply::Json("Autre".into()))));
    h.submit(key(1, 3), "other");
    h.submit(key(1, 4), "other");
    h.wait("translating", SECS, |h| h.last(key(1, 3)).as_deref() == Some("translating"));
    h.h().cancel(key(1, 3));
    assert_eq!(h.wait_final(key(1, 4), SECS), "done:Autre");
    assert_eq!(h.last(key(1, 3)).as_deref(), Some("translating"));
}

#[test]
fn not_installed_low_memory_and_shortcuts() {
    let h = H::new(Opts { installed: false, ..Default::default() });
    h.submit(key(1, 1), MSG);
    assert_eq!(h.wait_final(key(1, 1), SECS), "notinstalled");
    assert_eq!(h.state(), Some(EngineState::NotInstalled));

    let l = H::new(Opts { snap: Snapshot { avail_ram_mb: 3000, ..cpu_only() }, ..Default::default() });
    l.submit(key(1, 1), MSG);
    assert_eq!(l.wait_final(key(1, 1), SECS), "lowmem");
    assert_eq!(l.s.attempts.load(Ordering::SeqCst), 0);

    let s = H::new(Opts::default());
    s.submit(key(1, 1), "\u{2}{skull}\u{3} 123");
    assert_eq!(s.wait_final(key(1, 1), SECS), "nothing");
    s.h().submit(job(1, 2, "ko", "안녕하세요 같이 던전 가실 분?", &[], false));
    assert_eq!(s.wait_final(key(1, 2), SECS), "same");
    assert_eq!(s.s.attempts.load(Ordering::SeqCst), 0, "no model call for shortcuts");
    // The model says the text is already French.
    s.http.push(Reply::Json(String::new()));
    s.http.push(Reply::Json("Salut tout le monde !".into()));
    s.submit(key(1, 3), "salut tout le monde");
    assert_eq!(s.wait_final(key(1, 3), SECS), "same");
}

#[test]
fn shutdown_is_fast_with_a_hanging_request() {
    let mut h = H::new(Opts::default());
    h.http.push(Reply::Json(String::new()));
    h.http.push(Reply::Hang);
    h.submit(key(1, 1), MSG);
    h.wait("translating", SECS, |h| h.last(key(1, 1)).as_deref() == Some("translating"));
    let t = Instant::now();
    drop(h.engine.take());
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    assert!(h.proc_killed(0));
}

// ---------------------------------------------------------------- remote services (docs/ENGINE.md section 13)

const RKEY: &str = "TPKEYSENTINEL-9f8e7d6c5b4a39281706";
/// A word of the chat lines below: never in a log event.
const CHAT_WORD: &str = "Zorbulax";

fn claude(cap: u32) -> RemoteConfig {
    RemoteConfig::new(remote::Provider::Claude, "", "", remote::Secret::new(RKEY), cap).unwrap()
}

fn claude_says(text: &str) -> FakeReply {
    FakeReply::ok(serde_json::json!({"content": [{"type": "text", "text": text}], "stop_reason": "end_turn"}))
}

fn claude_error(status: u16, message: &str) -> FakeReply {
    FakeReply::status(status, serde_json::json!({"type": "error", "error": {"type": "authentication_error", "message": message}}))
}

/// No key, no piece of chat text and no translation in any log event.
fn log_is_clean(h: &H) {
    for e in h.events.lock().unwrap().iter() {
        for bad in [RKEY, "TPKEYS", CHAT_WORD, "Linen", "Vends", "Bonjour"] {
            assert!(!e.contains(bad), "{bad} in log event {e}");
        }
    }
}

const RMSG: &str = "wts \u{2}[Linen Cloth]\u{3} cheap Zorbulax";

#[test]
fn remote_happy_path_never_starts_the_local_server() {
    let h = H::new(Opts::default());
    h.h().set_remote(Some(claude(0)));
    h.remote.push(claude_says("Vends {1} pas cher Zorbulax"));
    h.submit(key(1, 1), RMSG);
    assert_eq!(h.wait_final(key(1, 1), SECS), "done:Vends [Linen Cloth] pas cher Zorbulax");
    assert_eq!(h.of(key(1, 1)), vec!["queued:0", "translating", "done:Vends [Linen Cloth] pas cher Zorbulax"]);
    assert_eq!(h.s.attempts.load(Ordering::SeqCst), 0, "the local server never started");
    h.remote.with_seen(|s| {
        assert_eq!(s.len(), 1);
        let user = s[0].body["messages"][0]["content"].as_str().unwrap();
        assert!(user.ends_with("Glossary:\n- \"wts\" = \"vend\"\n\nMessage:\nwts {1} cheap Zorbulax"), "{user}");
        assert!(!user.contains("<|") && !s[0].body["system"].as_str().unwrap().contains("<|"));
        assert!(!user.contains("Linen"), "item names stay placeholders");
    });
    // cached under the remote identity: no second request
    h.submit(key(1, 2), RMSG);
    h.wait("cached", Duration::from_millis(500), |h| h.last(key(1, 2)).as_deref() == Some("done:Vends [Linen Cloth] pas cher Zorbulax"));
    assert_eq!(h.remote.count(), 1);
    assert!(h.has_event("RemoteConfigured { provider: \"claude\", model: \"claude-opus-5-5\", daily_cap: 0 }"));
    assert!(h.has_event("RemoteRequest { req_id: 1, provider: \"claude\", model: \"claude-opus-5-5\", chars: 29, follow_up: false }"));
    assert!(h.has_event("RemoteDone { req_id: 1, provider: \"claude\", model: \"claude-opus-5-5\", status: 200"));
    assert!(h.fbs.lock().unwrap().is_empty());
    log_is_clean(&h);
}

#[test]
fn remote_failure_falls_back_to_the_local_model_with_the_reason() {
    let h = H::new(Opts::default());
    h.h().set_remote(Some(claude(0)));
    h.remote.push(claude_error(401, &format!("invalid x-api-key {RKEY}")));
    h.submit(key(1, 1), RMSG);
    assert_eq!(h.wait_final(key(1, 1), SECS), "done:Vends [Linen Cloth] pas cher|fb:http:401");
    let u = h.of(key(1, 1));
    assert_eq!(&u[..2], &["queued:0", "translating"]);
    assert!(u[2..].iter().all(|s| s.ends_with("|fb:http:401")), "every update after the fallback carries it: {u:?}");
    assert!(u.contains(&"loading|fb:http:401".to_string()));
    assert_eq!(h.s.attempts.load(Ordering::SeqCst), 1, "the local server served it");
    // the badge tooltip: provider, status and the service's message without the key
    let fbs = h.fbs.lock().unwrap().clone();
    let (_, last) = fbs.last().unwrap();
    assert_eq!((last.provider, last.kind, last.status), (remote::Provider::Claude, FallbackKind::Http, Some(401)));
    assert_eq!(last.message, "invalid x-api-key [redacted]");
    assert!(h.has_event("RemoteFallback { req_id: 1, provider: \"claude\", model: \"claude-opus-5-5\", kind: \"http\", status: 401"));
    // a local answer to a remote job is not cached: the next request tries the service again
    h.remote.push(claude_says("Vends {1} pas cher"));
    h.submit(key(1, 2), RMSG);
    assert_eq!(h.wait_final(key(1, 2), SECS), "done:Vends [Linen Cloth] pas cher");
    assert_eq!(h.remote.count(), 2);
    log_is_clean(&h);
}

#[test]
fn daily_cap_sends_nothing_once_reached() {
    let h = H::new(Opts::default());
    // "wts {1} cheap Zorbulax" (22) + "wts" + "vend" (7) = 29 characters sent per job, and the
    // answer "Vends {1} pas cher" (18) received
    h.h().set_remote(Some(claude(40)));
    h.remote.respond_with(|_| claude_says("Vends {1} pas cher"));
    h.submit(key(1, 1), RMSG);
    assert_eq!(h.wait_final(key(1, 1), SECS), "done:Vends [Linen Cloth] pas cher");
    let text = std::fs::read_to_string(h.dir.join("remote_usage.json")).unwrap();
    assert_eq!(text, format!("{{\"version\":1,\"day\":\"{}\",\"chars\":47}}", remote::usage::today()), "numbers only");
    h.submit(key(1, 2), "wts \u{2}[Silk Cloth]\u{3} cheap Zorbulax");
    assert_eq!(h.wait_final(key(1, 2), SECS), "done:Vends [Silk Cloth] pas cher|fb:daily_cap:0");
    assert_eq!(h.remote.count(), 1, "nothing sent past the cap");
    assert!(h.has_event("kind: \"daily_cap\""));
    // the reason carries the cap that applied (the badge shows it even if the setting changes)
    assert_eq!(h.fbs.lock().unwrap().last().map(|(_, f)| f.message.clone()).as_deref(), Some("40"));
    // the count is on disk for the next start (same data dir)
    assert_eq!(remote::usage::read_today(&h.dir), 47);
    log_is_clean(&h);
}

#[test]
fn failed_requests_do_not_count_toward_the_cap() {
    let h = H::new(Opts::default());
    h.h().set_remote(Some(claude(60)));
    // an invalid key: every line falls back with the real error, and nothing is counted
    for i in 1..=4u16 {
        h.remote.push(claude_error(401, "invalid x-api-key"));
        h.submit(key(1, i), &format!("wts \u{2}[Linen Cloth]\u{3} cheap Zorbulax {}", "x".repeat(i as usize)));
        assert!(h.wait_final(key(1, i), SECS).ends_with("|fb:http:401"), "line {i}: the real error, never the daily limit");
    }
    assert_eq!(remote::usage::read_today(&h.dir), 0);
    // a working answer counts what was sent and received
    h.remote.push(claude_says("Vends {1} pas cher"));
    h.submit(key(1, 9), RMSG);
    assert_eq!(h.wait_final(key(1, 9), SECS), "done:Vends [Linen Cloth] pas cher");
    assert_eq!(remote::usage::read_today(&h.dir), 47);
}

#[test]
fn ids_and_addresses_never_reach_the_service_in_clear() {
    let h = H::new(Opts::default());
    h.h().set_remote(Some(claude(0)));
    // the service lost a placeholder of a line holding an ID: no Brackets retry at the service
    // (that form writes the ID inline); the line is translated locally with the reason
    h.remote.push(claude_says("Vends {1} pas cher, ajoute"));
    h.submit(key(1, 1), "wts \u{2}[Linen Cloth]\u{3} cheap, add Kiwi48213");
    assert!(h.wait_final(key(1, 1), SECS).ends_with("|fb:kept_local:0"));
    assert_eq!(h.remote.count(), 1);
    // more IDs than placeholders: nothing is sent at all
    let ids: Vec<String> = (1..=10).map(|i| format!("Kiwi{}", 48200 + i)).collect();
    h.submit(key(1, 2), &format!("add {} please", ids.join(" ")));
    assert!(h.wait_final(key(1, 2), SECS).ends_with("|fb:kept_local:0"));
    assert_eq!(h.remote.count(), 1, "the second line was never sent");
    h.remote.with_seen(|s| {
        for r in s {
            let body = r.body.to_string();
            assert!(!body.contains("Kiwi48"), "an ID in clear: {body}");
        }
    });
    assert!(h.has_event("kind: \"kept_local\""));
    // a line with item links only still gets its Brackets retry at the service
    h.remote.push(claude_says("Vends pas cher"));
    h.remote.push(claude_says("Vends [Silk Cloth] pas cher"));
    h.submit(key(1, 3), "wts \u{2}[Silk Cloth]\u{3} cheap");
    assert_eq!(h.wait_final(key(1, 3), SECS), "done:Vends [Silk Cloth] pas cher");
    assert_eq!(h.remote.count(), 3);
    log_is_clean(&h);
}

#[test]
fn cache_key_holds_provider_and_model() {
    let h = H::new(Opts::default());
    let gemini = RemoteConfig::new(remote::Provider::Gemini, "", "", remote::Secret::new(RKEY), 0).unwrap();
    let haiku = RemoteConfig::new(remote::Provider::Claude, "claude-haiku-5-5", "", remote::Secret::new(RKEY), 0).unwrap();
    h.h().set_remote(Some(claude(0)));
    h.remote.push(claude_says("A {1}"));
    h.submit(key(1, 1), MSG);
    assert_eq!(h.wait_final(key(1, 1), SECS), "done:A [Linen Cloth]");
    h.h().set_remote(Some(gemini));
    h.remote.push(FakeReply::ok(serde_json::json!({"candidates": [{"content": {"parts": [{"text": "B {1}"}]}, "finishReason": "STOP"}]})));
    h.submit(key(1, 2), MSG);
    assert_eq!(h.wait_final(key(1, 2), SECS), "done:B [Linen Cloth]");
    h.h().set_remote(Some(haiku));
    h.remote.push(claude_says("C {1}"));
    h.submit(key(1, 3), MSG);
    assert_eq!(h.wait_final(key(1, 3), SECS), "done:C [Linen Cloth]");
    h.h().set_remote(Some(claude(0)));
    h.submit(key(1, 4), MSG);
    h.wait("cached", Duration::from_millis(500), |h| h.last(key(1, 4)).as_deref() == Some("done:A [Linen Cloth]"));
    assert_eq!(h.remote.count(), 3);
    // back to the local model: its own entry, not a remote one
    h.h().set_remote(None);
    h.submit(key(1, 5), MSG);
    assert_eq!(h.wait_final(key(1, 5), SECS), "done:Vends [Linen Cloth] pas cher");
    assert_eq!(h.s.attempts.load(Ordering::SeqCst), 1);
    assert!(h.has_event("RemoteConfigured { provider: \"local\""));
}

#[test]
fn remote_follow_up_and_timeout() {
    let h = H::new(Opts::default());
    h.h().set_remote(Some(claude(0)));
    // the placeholder was dropped: one Brackets retry at the same service
    h.remote.push(claude_says("Vends pas cher"));
    h.remote.push(claude_says("Vends [Linen Cloth] pas cher"));
    h.submit(key(1, 1), MSG);
    assert_eq!(h.wait_final(key(1, 1), SECS), "done:Vends [Linen Cloth] pas cher");
    assert!(h.has_event("PlaceholderRetry { req_id: 1 }"));
    assert!(h.has_event("follow_up: true"));
    h.remote.with_seen(|s| {
        assert_eq!(s.len(), 2);
        assert!(s[1].body["messages"][0]["content"].as_str().unwrap().ends_with("Message:\nwts [Linen Cloth] cheap"));
    });
    assert_eq!(h.s.attempts.load(Ordering::SeqCst), 0);
    // no answer within the budget (800 ms in these tests): local, with the reason
    h.remote.push(FakeReply::Delay(Duration::from_secs(10), Box::new(claude_says("late"))));
    let t = Instant::now();
    h.submit(key(1, 2), "wts \u{2}[Silk Cloth]\u{3} cheap Zorbulax");
    assert_eq!(h.wait_final(key(1, 2), SECS), "done:Vends [Silk Cloth] pas cher|fb:timeout:0");
    assert!(t.elapsed() < Duration::from_secs(4), "{:?}", t.elapsed());
    log_is_clean(&h);
}

#[test]
fn remote_without_a_local_model() {
    let h = H::new(Opts { installed: false, ..Default::default() });
    h.h().set_remote(Some(claude(0)));
    h.remote.push(claude_says("Vends {1} pas cher"));
    h.submit(key(1, 1), MSG);
    assert_eq!(h.wait_final(key(1, 1), SECS), "done:Vends [Linen Cloth] pas cher", "no NotInstalled with a working service");
    h.remote.push(claude_error(500, "boom"));
    h.remote.push(claude_error(500, "boom again"));
    h.submit(key(1, 2), "another line");
    assert_eq!(h.wait_final(key(1, 2), SECS), "notinstalled|fb:http:500");
    // a refusal is a fallback too
    h.remote.push(FakeReply::ok(serde_json::json!({"content": [], "stop_reason": "refusal", "stop_details": {"category": "cyber"}})));
    h.submit(key(1, 3), "third line");
    assert_eq!(h.wait_final(key(1, 3), SECS), "notinstalled|fb:refused:0");
}

#[test]
fn echo_translator_ignores_the_remote_service() {
    let h = H::new(Opts { translator: Translator::Echo, ..Default::default() });
    h.h().set_remote(Some(claude(0)));
    h.submit(key(1, 1), MSG);
    assert_eq!(h.wait_final(key(1, 1), SECS), "done:WTS [LINEN CLOTH] CHEAP");
    assert_eq!(h.remote.count(), 0);
}
