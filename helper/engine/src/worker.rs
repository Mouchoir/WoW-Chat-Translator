//! The engine thread: queue, server supervisor, HTTP requests and the VRAM guard.
//!
//! One worker thread owns all state. It wakes on commands from [`EngineHandle`], on events from
//! request threads (one short-lived thread per HTTP request), and on a timer (50 ms while
//! something is happening, 250 ms with an idle server, 1 s otherwise). It never blocks on HTTP
//! except for a 500 ms `/health` probe while a server starts.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::cache::{self, Cache, Cached};
use crate::client::{self, Client, ReqError};
use crate::device::{self, Devices, SystemDevices};
use crate::discover::{self, Discovery, Runtime};
use crate::events::Backend;
use crate::flags::{self, Features};
use crate::pipeline::{self, Attempt, Finish, Prep, Prepared, Step};
use crate::policy::{self, Choice};
use crate::prompt;
use crate::protect;
use crate::queue::{Emit, Entry, Pushed, Queue, St, Work};
use crate::remote::{self, Fallback, FallbackKind, RemoteConfig, Transport};
use crate::server::{self, RealSpawner, ServerProc, Spawner};
use crate::{BackendPref, EngineConfig, EngineEvent, EngineState, FailKind, Job, JobKey, Translator, Update};

/// Timings of the lifecycle (design D.4); tests shrink them.
#[derive(Clone, Debug)]
pub(crate) struct Timing {
    pub health_every: Duration,
    pub health_timeout: Duration,
    pub ready_deadline: Duration,
    pub ready_deadline_first_vulkan: Duration,
    pub cpu_base: Duration,
    pub cpu_tokens_per_s: f64,
    pub gpu_base: Duration,
    pub gpu_tokens_per_s: f64,
    pub wow_gone_unload: Duration,
    pub error_retry: Duration,
    pub crash_window: Duration,
    pub crash_hold: Duration,
    pub crash_limit: usize,
    pub gpu_failure_limit: u32,
    pub vram_every: Duration,
    pub vram_min_mb: u32,
    pub max_age: Duration,
    pub echo_load: Duration,
    pub echo_work: Duration,
    pub rediscover_every: Duration,
    /// Remote requests: connect 5 s, total 15 s, one quick retry (docs/ENGINE.md section 13).
    pub remote: remote::Policy,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            health_every: Duration::from_millis(200),
            health_timeout: Duration::from_millis(500),
            ready_deadline: Duration::from_secs(60),
            ready_deadline_first_vulkan: Duration::from_secs(90),
            cpu_base: Duration::from_secs(6),
            cpu_tokens_per_s: 8.0,
            gpu_base: Duration::from_secs(4),
            gpu_tokens_per_s: 40.0,
            wow_gone_unload: Duration::from_secs(60),
            error_retry: Duration::from_secs(60),
            crash_window: Duration::from_secs(600),
            crash_hold: Duration::from_secs(600),
            crash_limit: 3,
            gpu_failure_limit: 2,
            vram_every: Duration::from_secs(15),
            vram_min_mb: 700,
            max_age: crate::queue::MAX_AGE,
            echo_load: crate::echo::LOAD,
            echo_work: crate::echo::WORK,
            rediscover_every: Duration::from_secs(30),
            remote: remote::Policy::default(),
        }
    }
}

impl Timing {
    /// CPU `6 s + n_predict/8 s`; GPU `4 s + n_predict/40 s`.
    pub fn request_deadline(&self, gpu: bool, n_predict: u32) -> Duration {
        if gpu {
            self.gpu_base + Duration::from_secs_f64(n_predict as f64 / self.gpu_tokens_per_s)
        } else {
            self.cpu_base + Duration::from_secs_f64(n_predict as f64 / self.cpu_tokens_per_s)
        }
    }
}

type DiscoverFn = Box<dyn FnMut() -> Discovery + Send>;
type FlagsFn = Box<dyn FnMut(&Runtime) -> Result<(Features, bool), &'static str> + Send>;

/// Everything the worker reaches outside itself; replaced by fakes in tests.
pub(crate) struct Deps {
    pub spawner: Box<dyn Spawner>,
    pub devices: Box<dyn Devices>,
    pub discover: DiscoverFn,
    pub flags: FlagsFn,
    /// HTTPS to the remote services (WinHTTP; a fake in tests).
    pub transport: Arc<dyn Transport>,
}

impl Deps {
    fn real(cfg: &EngineConfig) -> Deps {
        let runtime = cfg.runtime_dir.clone();
        let model = cfg.model_path.clone();
        let data_dir = cfg.data_dir.clone();
        Deps {
            spawner: Box::new(RealSpawner),
            devices: Box::new(SystemDevices::default()),
            discover: Box::new(move || discover::discover(runtime.as_deref(), model.as_deref())),
            flags: Box::new(move |rt: &Runtime| flags::probe(&rt.exe, &rt.dir, &data_dir)),
            transport: remote::system_transport(),
        }
    }
}

enum Cmd {
    Submit(Job),
    Cancel(JobKey),
    ClearSession(u16),
    WowPresent(bool),
    SetBackend(BackendPref),
    UnloadNow,
    SetRemote(Option<RemoteConfig>),
    Shutdown,
}

enum Ev {
    Warm { gen: u64, ok: bool },
    Partial { gen: u64, text: String },
    Done { gen: u64, result: Result<String, ReqError> },
    RemoteDone { gen: u64, result: Result<remote::Answer, remote::CallError> },
}

enum Msg {
    Cmd(Cmd),
    Ev(Ev),
}

type Killer = Arc<dyn Fn() + Send + Sync>;

/// Owns the worker thread and the llama-server process.
pub struct Engine {
    handle: EngineHandle,
    join: Option<JoinHandle<()>>,
    done_rx: Receiver<()>,
    killer: Arc<Mutex<Option<Killer>>>,
}

/// Cheap to clone; usable from any thread.
#[derive(Clone)]
pub struct EngineHandle {
    tx: Sender<Msg>,
}

impl Engine {
    /// Starts the worker thread. Nothing is loaded until the first job.
    pub fn start(cfg: EngineConfig, on_update: Box<dyn Fn(Update) + Send>, on_state: Box<dyn Fn(EngineState) + Send>) -> Engine {
        let deps = Deps::real(&cfg);
        Engine::start_with(cfg, deps, Timing::default(), on_update, on_state)
    }

    pub(crate) fn start_with(
        cfg: EngineConfig,
        deps: Deps,
        timing: Timing,
        on_update: Box<dyn Fn(Update) + Send>,
        on_state: Box<dyn Fn(EngineState) + Send>,
    ) -> Engine {
        let (tx, rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let killer = Arc::new(Mutex::new(None));
        let w = Worker::new(cfg, deps, timing, on_update, on_state, tx.clone(), killer.clone());
        let join = std::thread::Builder::new()
            .name("tp-engine".into())
            .spawn(move || {
                w.run(rx);
                let _ = done_tx.send(());
            })
            .expect("engine thread");
        Engine { handle: EngineHandle { tx }, join: Some(join), done_rx, killer }
    }

    pub fn handle(&self) -> EngineHandle {
        self.handle.clone()
    }

    /// Kills llama-server and joins the engine threads within 2 s.
    pub fn shutdown(self) {
        // Drop does the work.
    }

    fn stop(&mut self) {
        let Some(join) = self.join.take() else { return };
        // Kill the server first: any request thread blocked on it returns at once.
        if let Some(k) = self.killer.lock().unwrap_or_else(|e| e.into_inner()).take() {
            k();
        }
        let _ = self.handle.tx.send(Msg::Cmd(Cmd::Shutdown));
        match self.done_rx.recv_timeout(Duration::from_secs(2)) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                let _ = join.join();
            }
            // Never block the app's exit; the job object still kills the server with the process.
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}

impl EngineHandle {
    fn send(&self, c: Cmd) {
        let _ = self.tx.send(Msg::Cmd(c));
    }

    pub fn submit(&self, job: Job) {
        self.send(Cmd::Submit(job));
    }

    /// Drops a waiting job, or discards the result of the job in flight.
    pub fn cancel(&self, key: JobKey) {
        self.send(Cmd::Cancel(key));
    }

    /// Cancels every job of one addon session.
    pub fn clear_session(&self, session: u16) {
        self.send(Cmd::ClearSession(session));
    }

    /// false starts the 60 s unload countdown.
    pub fn set_wow_present(&self, present: bool) {
        self.send(Cmd::WowPresent(present));
    }

    pub fn set_backend(&self, p: BackendPref) {
        self.send(Cmd::SetBackend(p));
    }

    pub fn unload_now(&self) {
        self.send(Cmd::UnloadNow);
    }

    /// The remote service for the next jobs (None: the local model only). A job already in
    /// flight keeps its route. Build it with `RemoteConfig::from_settings` (consent and key
    /// checked) when the settings are loaded or saved.
    pub fn set_remote(&self, cfg: Option<RemoteConfig>) {
        self.send(Cmd::SetRemote(cfg));
    }
}

fn pref_of(b: Backend) -> BackendPref {
    match b {
        Backend::Cpu => BackendPref::Cpu,
        Backend::Vulkan => BackendPref::Vulkan,
        Backend::Cuda => BackendPref::Cuda,
    }
}

enum Srv {
    None,
    Starting { proc: Box<dyn ServerProc>, client: Client, backend: Backend, started: Instant, deadline: Instant, next_health: Instant },
    Warming { proc: Box<dyn ServerProc>, client: Client, backend: Backend, started: Instant, deadline: Instant, gen: u64 },
    Ready { proc: Box<dyn ServerProc>, client: Client, backend: Backend },
}

impl Srv {
    fn proc_mut(&mut self) -> Option<&mut Box<dyn ServerProc>> {
        match self {
            Srv::None => None,
            Srv::Starting { proc, .. } | Srv::Warming { proc, .. } | Srv::Ready { proc, .. } => Some(proc),
        }
    }

    fn backend(&self) -> Option<Backend> {
        match self {
            Srv::None => None,
            Srv::Starting { backend, .. } | Srv::Warming { backend, .. } | Srv::Ready { backend, .. } => Some(*backend),
        }
    }
}

/// The HTTP request in flight (to llama-server, or to the remote service).
struct Req {
    gen: u64,
    deadline: Instant,
    prepared: Prepared,
    gpu: bool,
    remote: bool,
    /// Remote request: the characters it sent (counted against the daily cap once it answers).
    sent_chars: u32,
}

/// Where the job in flight is translated.
#[derive(Clone)]
enum Route {
    Local,
    Remote(Arc<RemoteConfig>),
    /// Meant for the remote service, translated locally: every update carries the reason.
    FellBack(Fallback),
}

#[derive(Clone, Copy, PartialEq)]
enum EchoPhase {
    Load,
    Work,
}

struct Worker {
    translator: Translator,
    gpu_margin_mb: u32,
    cpu_threads: u32,
    idle_unload: Duration,
    keep_loaded: bool,
    data_dir: PathBuf,
    log: Arc<dyn Fn(EngineEvent) + Send + Sync>,
    on_update: Box<dyn Fn(Update) + Send>,
    on_state: Box<dyn Fn(EngineState) + Send>,
    deps: Deps,
    t: Timing,
    tx: Sender<Msg>,
    killer: Arc<Mutex<Option<Killer>>>,

    queue: Queue,
    cache: Cache,
    discovery: Option<Discovery>,
    discovered_at: Option<Instant>,
    features: Option<Features>,
    srv: Srv,
    req: Option<Req>,
    gen: u64,
    /// Last non-final status sent to the job in flight (merged duplicates get it too).
    flight_status: St,
    state: Option<EngineState>,
    pref: BackendPref,
    gpu_blocked: bool,
    gpu_failures: u32,
    /// Set after a GPU start failure: the next start is CPU, once.
    cpu_once: bool,
    crashes: VecDeque<Instant>,
    error_until: Option<Instant>,
    wow_absent_since: Option<Instant>,
    pending_unload: Option<&'static str>,
    next_vram: Instant,
    last_used: Instant,
    echo_loaded: bool,
    echo_phase: Option<(Instant, EchoPhase)>,
    key_file: Option<PathBuf>,
    loading_first_gpu: bool,
    shutdown: bool,
    /// The remote service (`set_remote`), if any.
    remote: Option<Arc<RemoteConfig>>,
    /// The daily character cap's count.
    usage: remote::usage::Usage,
    /// The route of the job in flight (None until it is chosen).
    route: Option<Route>,
}

impl Worker {
    fn new(
        cfg: EngineConfig,
        deps: Deps,
        t: Timing,
        on_update: Box<dyn Fn(Update) + Send>,
        on_state: Box<dyn Fn(EngineState) + Send>,
        tx: Sender<Msg>,
        killer: Arc<Mutex<Option<Killer>>>,
    ) -> Worker {
        let now = Instant::now();
        let usage = remote::usage::Usage::load(Some(&cfg.data_dir));
        Worker {
            translator: cfg.translator,
            gpu_margin_mb: cfg.gpu_margin_mb,
            cpu_threads: cfg.cpu_threads,
            idle_unload: cfg.idle_unload,
            keep_loaded: cfg.keep_loaded,
            data_dir: cfg.data_dir,
            log: cfg.log,
            on_update,
            on_state,
            deps,
            queue: Queue::new(crate::queue::MAX_WAITING, t.max_age),
            t,
            tx,
            killer,
            cache: Cache::new(cache::CAPACITY),
            discovery: None,
            discovered_at: None,
            features: None,
            srv: Srv::None,
            req: None,
            gen: 0,
            flight_status: St::Translating,
            state: None,
            pref: cfg.backend,
            gpu_blocked: false,
            gpu_failures: 0,
            cpu_once: false,
            crashes: VecDeque::new(),
            error_until: None,
            wow_absent_since: None,
            pending_unload: None,
            next_vram: now,
            last_used: now,
            echo_loaded: false,
            echo_phase: None,
            key_file: None,
            loading_first_gpu: false,
            shutdown: false,
            remote: None,
            usage,
            route: None,
        }
    }

    fn run(mut self, rx: Receiver<Msg>) {
        if self.translator == Translator::Llama {
            self.discover(Instant::now());
        }
        self.emit_state(Instant::now());
        while !self.shutdown {
            match rx.recv_timeout(self.wait()) {
                Ok(Msg::Cmd(c)) => self.command(c, Instant::now()),
                Ok(Msg::Ev(e)) => self.event(e, Instant::now()),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if self.shutdown {
                break;
            }
            self.step(Instant::now());
        }
        self.kill_server("shutdown");
        self.delete_key_file();
    }

    fn wait(&self) -> Duration {
        let busy = self.queue.flight().is_some()
            || self.queue.waiting_len() > 0
            || self.echo_phase.is_some()
            || matches!(self.srv, Srv::Starting { .. } | Srv::Warming { .. });
        if busy {
            Duration::from_millis(50)
        } else if !matches!(self.srv, Srv::None) || self.pending_unload.is_some() {
            Duration::from_millis(250)
        } else {
            Duration::from_secs(1)
        }
    }

    fn log(&self, e: EngineEvent) {
        (self.log)(e);
    }

    fn emit_all(&self, out: Emit) {
        for (key, st) in out {
            (self.on_update)(Update { key, status: st.to_status(), fallback: None });
        }
    }

    fn emit(&self, key: JobKey, st: &St) {
        self.emit_with(key, st, None);
    }

    fn emit_with(&self, key: JobKey, st: &St, fallback: Option<&Fallback>) {
        (self.on_update)(Update { key, status: st.to_status(), fallback: fallback.cloned() });
    }

    /// The fallback reason of the job in flight (its updates carry it).
    fn flight_fallback(&self) -> Option<&Fallback> {
        match &self.route {
            Some(Route::FellBack(f)) => Some(f),
            _ => None,
        }
    }

    /// Non-final status for the job in flight.
    fn flight_update(&mut self, st: St) {
        for k in self.queue.flight_keys() {
            self.emit_with(k, &st, self.flight_fallback());
        }
        self.flight_status = st;
    }

    /// Final status for the job in flight; the queue moves on.
    fn finish_flight(&mut self, st: St, now: Instant) {
        let fallback = self.flight_fallback().cloned();
        self.route = None;
        let mut out = Emit::new();
        if let Some(e) = self.queue.finish_flight(&mut out) {
            let ms = now.duration_since(e.submitted).as_millis() as u64;
            for &k in &e.keys {
                self.emit_with(k, &st, fallback.as_ref());
                match &st {
                    St::Failed(kind) => self.log(EngineEvent::JobFailed { req_id: k.req_id, kind: *kind, ms }),
                    St::Done(s) => self.log(EngineEvent::JobDone { req_id: k.req_id, ms, output_bytes: s.len() as u32, outcome: "done" }),
                    St::Same => self.log(EngineEvent::JobDone { req_id: k.req_id, ms, output_bytes: 0, outcome: "same" }),
                    _ => {}
                }
            }
        }
        self.emit_all(out);
        self.flight_status = St::Translating;
        self.last_used = now;
    }

    fn compute_state(&self, now: Instant) -> EngineState {
        if self.translator == Translator::Echo {
            return match self.echo_phase {
                Some((_, EchoPhase::Load)) => EngineState::Loading,
                Some((_, EchoPhase::Work)) => EngineState::Busy,
                None if self.echo_loaded => EngineState::ReadyCpu,
                None => EngineState::Unloaded,
            };
        }
        if self.discovery.as_ref().is_some_and(|d| !d.installed()) {
            return EngineState::NotInstalled;
        }
        if self.error_until.is_some_and(|u| now < u) {
            return EngineState::Error;
        }
        match &self.srv {
            Srv::Starting { .. } | Srv::Warming { .. } => EngineState::Loading,
            Srv::Ready { .. } if self.req.is_some() => EngineState::Busy,
            Srv::Ready { backend: Backend::Cpu, .. } => EngineState::ReadyCpu,
            Srv::Ready { .. } => EngineState::ReadyGpu,
            Srv::None => EngineState::Unloaded,
        }
    }

    fn emit_state(&mut self, now: Instant) {
        let s = self.compute_state(now);
        if self.state != Some(s) {
            self.state = Some(s);
            self.log(EngineEvent::State(s));
            (self.on_state)(s);
        }
    }

    // ---------------------------------------------------------------- discovery

    fn discover(&mut self, now: Instant) {
        let d = (self.deps.discover)();
        self.log(EngineEvent::Discovery {
            runtime_found: d.runtime.is_some(),
            model_found: d.model.is_some(),
            known_digest: d.model.as_ref().is_some_and(|m| m.known),
        });
        if d.explicit_rejected {
            self.log(EngineEvent::DiscoveryRejected { what: "configured runtime or model" });
        }
        if d.model.as_ref().is_some_and(|m| !m.known) {
            self.log(EngineEvent::UnknownModel);
        }
        if self.discovery.as_ref().and_then(|o| o.runtime.as_ref().map(|r| r.exe.clone())) != d.runtime.as_ref().map(|r| r.exe.clone()) {
            self.features = None;
        }
        self.discovery = Some(d);
        self.discovered_at = Some(now);
    }

    /// True when runtime and model are present (rediscovers at most every 30 s while missing).
    fn installed(&mut self, now: Instant) -> bool {
        let stale = self.discovered_at.is_none_or(|t| now.duration_since(t) >= self.t.rediscover_every);
        if self.discovery.as_ref().is_none_or(|d| !d.installed()) && stale {
            self.discover(now);
        }
        self.discovery.as_ref().is_some_and(Discovery::installed)
    }

    fn model_identity(&self) -> String {
        match self.translator {
            Translator::Echo => "echo".into(),
            Translator::Llama => self.discovery.as_ref().and_then(|d| d.model.as_ref()).map(|m| m.identity()).unwrap_or_default(),
        }
    }

    // ---------------------------------------------------------------- commands

    fn command(&mut self, c: Cmd, now: Instant) {
        match c {
            Cmd::Submit(job) => self.submit(job, now),
            Cmd::Cancel(k) => {
                let mut out = Emit::new();
                if self.queue.cancel(k, &mut out) {
                    self.log(EngineEvent::JobCancelled { req_id: k.req_id });
                }
                self.emit_all(out);
            }
            Cmd::ClearSession(s) => {
                let mut out = Emit::new();
                self.queue.clear_session(s, &mut out);
                self.emit_all(out);
            }
            Cmd::WowPresent(true) => self.wow_absent_since = None,
            Cmd::WowPresent(false) => {
                self.wow_absent_since.get_or_insert(now);
            }
            Cmd::SetBackend(p) => {
                if p != self.pref {
                    self.pref = p;
                    self.gpu_blocked = false;
                    self.gpu_failures = 0;
                    if !matches!(self.srv, Srv::None) {
                        self.pending_unload = Some("backend");
                    }
                }
            }
            Cmd::UnloadNow => {
                if !matches!(self.srv, Srv::None) || self.echo_loaded {
                    self.pending_unload = Some("tray");
                }
            }
            Cmd::SetRemote(c) => {
                self.log(match &c {
                    Some(c) => EngineEvent::RemoteConfigured { provider: c.provider.id(), model: c.model_label(), daily_cap: c.daily_char_cap },
                    None => EngineEvent::RemoteConfigured { provider: "local", model: "default", daily_cap: 0 },
                });
                self.remote = c.map(Arc::new);
            }
            Cmd::Shutdown => self.shutdown = true,
        }
    }

    fn submit(&mut self, job: Job, now: Instant) {
        let key = job.key;
        if self.queue.contains(key) {
            // A re-click of a job still in progress: it keeps its place.
            return;
        }
        let protected = protect::protect(job.text.as_str(), prompt::DEFAULT);
        if let Some(sc) = pipeline::shortcut(&job.target, &protected) {
            let (st, outcome) = match sc {
                Prep::Nothing => (St::Nothing, "nothing"),
                _ => (St::Same, "same"),
            };
            self.emit(key, &st);
            self.log(EngineEvent::JobDone { req_id: key.req_id, ms: 0, output_bytes: 0, outcome });
            return;
        }
        // With a remote service, a missing local model only matters if the service fails.
        let remote = if self.translator == Translator::Llama { self.remote.clone() } else { None };
        if self.translator == Translator::Llama && remote.is_none() && !self.installed(now) {
            self.emit(key, &St::NotInstalled);
            self.log(EngineEvent::JobDone { req_id: key.req_id, ms: 0, output_bytes: 0, outcome: "not_installed" });
            return;
        }
        let raw_pairs: Vec<(&str, &str)> = job.pairs.iter().map(|(t, m)| (t.as_str(), m.as_str())).collect();
        let pairs = pipeline::clean_pairs(&raw_pairs);
        let identity = match &remote {
            Some(c) => c.identity(),
            None => self.model_identity(),
        };
        let ck = cache::key(&identity, &job.target, &protected.text, &protected.span_key(), &pairs);
        if !job.no_cache {
            if let Some(c) = self.cache.get(ck) {
                let (st, n) = match c {
                    Cached::Done(s) => {
                        let n = s.len() as u32;
                        (St::Done(s), n)
                    }
                    Cached::Same => (St::Same, 0),
                };
                self.emit(key, &st);
                self.log(EngineEvent::JobDone { req_id: key.req_id, ms: 0, output_bytes: n, outcome: "cached" });
                return;
            }
        }
        let input_bytes = job.text.len() as u32;
        let n_pairs = job.pairs.len().min(255) as u8;
        let work = Work { target: job.target, text: job.text, pairs: job.pairs };
        let mut out = Emit::new();
        match self.queue.push(Entry::new(key, work, ck, now).urgent(job.urgent), &mut out) {
            Pushed::Duplicate => {}
            Pushed::MergedInFlight => {
                let st = self.flight_status.clone();
                self.emit_with(key, &st, self.flight_fallback());
            }
            Pushed::Waiting => {}
        }
        let ahead = out.iter().rev().find_map(|(k, s)| match s {
            St::Queued { ahead } if *k == key => Some(*ahead),
            _ => None,
        });
        self.emit_all(out);
        self.log(EngineEvent::JobQueued { req_id: key.req_id, input_bytes, pairs: n_pairs, ahead: ahead.unwrap_or(0) });
    }

    // ---------------------------------------------------------------- request events

    fn event(&mut self, e: Ev, now: Instant) {
        match e {
            Ev::Warm { gen, ok } => {
                let Srv::Warming { gen: g, .. } = &self.srv else { return };
                if *g != gen {
                    return;
                }
                let Srv::Warming { proc, client, backend, started, .. } = std::mem::replace(&mut self.srv, Srv::None) else { unreachable!() };
                self.log(EngineEvent::WarmUp { ms: now.duration_since(started).as_millis() as u64 });
                if !ok {
                    self.log(EngineEvent::Watchdog { action: "warmup_failed" });
                } else if backend == Backend::Vulkan {
                    let m = device::vulkan_marker(&self.data_dir);
                    let _ = std::fs::create_dir_all(m.parent().unwrap_or(&self.data_dir));
                    let _ = std::fs::write(m, b"");
                }
                self.srv = Srv::Ready { proc, client, backend };
                self.last_used = now;
                self.next_vram = now + self.t.vram_every;
            }
            Ev::Partial { gen, text } => {
                let Some(r) = self.req.as_ref().filter(|r| r.gen == gen) else { return };
                if let Some(s) = pipeline::partial(&r.prepared, &text) {
                    self.flight_update(St::Partial(s));
                }
            }
            Ev::Done { gen, result } => {
                if self.req.as_ref().is_none_or(|r| r.gen != gen) {
                    return;
                }
                let r = self.req.take().unwrap();
                self.on_result(r, result, now);
            }
            Ev::RemoteDone { gen, result } => {
                if self.req.as_ref().is_none_or(|r| r.gen != gen || !r.remote) {
                    return;
                }
                let r = self.req.take().unwrap();
                self.on_remote_result(r, result, now);
            }
        }
    }

    /// The cache key of the job in flight for one model identity (the route decides which:
    /// the local model, or the remote provider and model).
    fn flight_cache_key(&self, identity: &str) -> Option<u64> {
        let e = self.queue.flight()?;
        let protected = protect::protect(e.work.text.as_str(), prompt::DEFAULT);
        let raw_pairs: Vec<(&str, &str)> = e.work.pairs.iter().map(|(t, m)| (t.as_str(), m.as_str())).collect();
        let pairs = pipeline::clean_pairs(&raw_pairs);
        Some(cache::key(identity, &e.work.target, &protected.text, &protected.span_key(), &pairs))
    }

    fn on_result(&mut self, r: Req, result: Result<String, ReqError>, now: Instant) {
        // A local answer to a job meant for the remote service is not cached: the next request
        // tries the service again.
        let ck = if self.flight_fallback().is_some() { None } else { self.flight_cache_key(&self.model_identity()) };
        match result {
            Ok(raw) => {
                let fin = pipeline::finish(&r.prepared, &raw);
                let marked = self.queue.flight().map(|e| e.work.text.as_str().to_string()).unwrap_or_default();
                // At most one follow-up per job (pipeline::next): Brackets or insist retry.
                match pipeline::next(&r.prepared, fin, &marked) {
                    Step::Again(p) => {
                        for k in self.queue.flight_keys() {
                            self.log(match p.attempt {
                                Attempt::Insist => EngineEvent::InsistRetry { req_id: k.req_id },
                                _ => EngineEvent::PlaceholderRetry { req_id: k.req_id },
                            });
                        }
                        self.send_request(p, true, now);
                    }
                    Step::Final(Finish::Done(s)) => {
                        if let Some(ck) = ck {
                            self.cache.put(ck, Cached::Done(s.clone()));
                        }
                        self.finish_flight(St::Done(s), now);
                    }
                    Step::Final(Finish::Same) => {
                        if let Some(ck) = ck {
                            self.cache.put(ck, Cached::Same);
                        }
                        self.finish_flight(St::Same, now);
                    }
                    Step::Final(_) => self.finish_flight(St::Failed(FailKind::BadOutput), now),
                }
            }
            Err(ReqError::Timeout) => self.request_timeout(now),
            Err(ReqError::Io) => {
                // A reset connection usually means the server is exiting: give it 300 ms to show.
                let t = Instant::now();
                let mut died = false;
                while let Some(p) = self.srv.proc_mut() {
                    died = p.exit_code().is_some();
                    if died || t.elapsed() >= Duration::from_millis(300) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                if died {
                    self.server_crashed(now, true);
                } else {
                    self.finish_flight(St::Failed(FailKind::Http), now);
                }
            }
            Err(ReqError::Http(_)) | Err(ReqError::Bad) => self.finish_flight(St::Failed(FailKind::Http), now),
        }
    }

    // ---------------------------------------------------------------- the tick

    fn step(&mut self, now: Instant) {
        let mut out = Emit::new();
        self.queue.expire(now, &mut out);
        self.emit_all(out);

        if self.translator == Translator::Echo {
            self.step_echo(now);
        } else {
            self.check_exit(now);
            self.poll_start(now);
            if self.req.as_ref().is_some_and(|r| now > r.deadline) {
                if self.req.as_ref().is_some_and(|r| r.remote) {
                    self.remote_timeout();
                } else {
                    self.request_timeout(now);
                }
            }
            self.drive_flight(now);
            self.housekeeping(now);
        }
        self.emit_state(now);
    }

    fn next_flight(&mut self, now: Instant) -> bool {
        if self.queue.flight().is_some() {
            return true;
        }
        let mut out = Emit::new();
        let started = self.queue.start_next(now, &mut out).is_some();
        self.emit_all(out);
        started
    }

    /// Drops a flight nobody waits for anymore, before any work started on it.
    fn drop_orphan_flight(&mut self, now: Instant) {
        if self.req.is_none() && self.echo_phase.is_none() && self.queue.flight().is_some_and(|e| e.keys.is_empty()) {
            self.finish_flight(St::Translating, now);
        }
    }

    fn step_echo(&mut self, now: Instant) {
        if let Some((due, phase)) = self.echo_phase {
            if now >= due {
                match phase {
                    EchoPhase::Load => {
                        self.echo_loaded = true;
                        self.flight_update(St::Translating);
                        self.echo_phase = Some((now + self.t.echo_work, EchoPhase::Work));
                    }
                    EchoPhase::Work => {
                        self.echo_phase = None;
                        if let Some(e) = self.queue.flight() {
                            let s = crate::echo::output(e.work.text.as_str());
                            let ck = e.cache_key;
                            self.cache.put(ck, Cached::Done(s.clone()));
                            self.finish_flight(St::Done(s), now);
                        }
                    }
                }
            }
        }
        if self.echo_phase.is_none() {
            if let Some(reason) = self.pending_unload.take() {
                self.echo_loaded = false;
                self.log(EngineEvent::Unload { reason });
            }
            self.drop_orphan_flight(now);
            if self.next_flight(now) {
                if self.echo_loaded {
                    self.flight_update(St::Translating);
                    self.echo_phase = Some((now + self.t.echo_work, EchoPhase::Work));
                } else {
                    self.flight_update(St::Loading { first_gpu_use: false });
                    self.echo_phase = Some((now + self.t.echo_load, EchoPhase::Load));
                }
            } else if self.echo_loaded && !self.keep_loaded && now.duration_since(self.last_used) >= self.idle_unload {
                self.echo_loaded = false;
                self.log(EngineEvent::Unload { reason: "idle" });
            }
        }
    }

    /// Serves the job in flight: starts a server when needed, else sends the request.
    fn drive_flight(&mut self, now: Instant) {
        if self.req.is_some() {
            return;
        }
        self.drop_orphan_flight(now);
        if self.queue.flight().is_none() {
            if self.pending_unload.is_some() && !matches!(self.srv, Srv::None) {
                // Unload first (backend change, VRAM guard, tray); the next job restarts it.
                return;
            }
            if !self.next_flight(now) {
                return;
            }
        }
        if self.route.is_none() {
            self.route = Some(match &self.remote {
                Some(c) => Route::Remote(c.clone()),
                None => Route::Local,
            });
        }
        if let Some(Route::Remote(cfg)) = &self.route {
            let cfg = cfg.clone();
            self.start_remote(cfg, now);
            if self.req.is_some() || self.queue.flight().is_none() || matches!(self.route, Some(Route::Remote(_))) {
                return;
            }
            // Fell back before anything was sent (daily cap): served locally below.
        }
        if self.pending_unload.is_some() && !matches!(self.srv, Srv::None) {
            return;
        }
        match &self.srv {
            Srv::Ready { .. } => {
                let Some(e) = self.queue.flight() else { return };
                let raw_pairs: Vec<(&str, &str)> = e.work.pairs.iter().map(|(t, m)| (t.as_str(), m.as_str())).collect();
                match pipeline::prepare(&e.work.target, e.work.text.as_str(), &raw_pairs, prompt::DEFAULT) {
                    Prep::Nothing => self.finish_flight(St::Nothing, now),
                    Prep::Same => self.finish_flight(St::Same, now),
                    Prep::Ready(p) => self.send_request(p, false, now),
                }
            }
            Srv::Starting { .. } | Srv::Warming { .. } => {
                // A job that became the flight while the server was already starting.
                if !matches!(self.flight_status, St::Loading { .. }) {
                    self.flight_update(St::Loading { first_gpu_use: self.loading_first_gpu });
                }
            }
            Srv::None => {
                if let Err(st) = self.start_server(now) {
                    self.finish_flight(st, now);
                }
            }
        }
    }

    fn send_request(&mut self, p: Prepared, retried: bool, now: Instant) {
        let Srv::Ready { client, backend, .. } = &self.srv else {
            self.finish_flight(St::Failed(FailKind::ServerDied), now);
            return;
        };
        let gpu = *backend != Backend::Cpu;
        let limit = self.t.request_deadline(gpu, p.n_predict);
        self.gen += 1;
        let gen = self.gen;
        let body = client::completion_body(&p.prompt, p.n_predict, Some(p.grammar), !gpu);
        let client = client.clone();
        let tx = self.tx.clone();
        let spawned = std::thread::Builder::new().name("tp-engine-req".into()).spawn(move || {
            let tx2 = tx.clone();
            let mut on_partial = |s: &str| {
                let _ = tx2.send(Msg::Ev(Ev::Partial { gen, text: s.to_string() }));
            };
            // The worker enforces the deadline; this is only a backstop.
            let result = client.completion(&body, limit + Duration::from_secs(2), &mut on_partial);
            let _ = tx.send(Msg::Ev(Ev::Done { gen, result }));
        });
        if spawned.is_err() {
            self.finish_flight(St::Failed(FailKind::Http), now);
            return;
        }
        self.req = Some(Req { gen, deadline: now + limit, prepared: p, gpu, remote: false, sent_chars: 0 });
        if !retried {
            self.flight_update(St::Translating);
        }
    }

    // ---------------------------------------------------------------- remote service

    /// First request of a job routed to the remote service.
    fn start_remote(&mut self, cfg: Arc<RemoteConfig>, now: Instant) {
        let Some(e) = self.queue.flight() else { return };
        let raw_pairs: Vec<(&str, &str)> = e.work.pairs.iter().map(|(t, m)| (t.as_str(), m.as_str())).collect();
        match pipeline::prepare(&e.work.target, e.work.text.as_str(), &raw_pairs, prompt::DEFAULT) {
            Prep::Nothing => self.finish_flight(St::Nothing, now),
            Prep::Same => self.finish_flight(St::Same, now),
            // more links, IDs and addresses than placeholders: the extra ones would go in clear
            Prep::Ready(p) if p.protected.tokens_in_clear > 0 => self.keep_local(&cfg),
            Prep::Ready(p) => self.send_remote(cfg, p, false, now),
        }
    }

    /// The line is translated locally for privacy (nothing more is sent).
    fn keep_local(&mut self, cfg: &RemoteConfig) {
        let fb = Fallback { provider: cfg.provider, kind: FallbackKind::KeptLocal, status: None, message: String::new() };
        self.fall_back(fb, cfg, 0, 0);
    }

    /// Sends one request to the service on its own thread (the daily cap is checked first; the
    /// characters are counted when the service answers). `follow_up`: the Brackets or insist
    /// retry of the same job.
    fn send_remote(&mut self, cfg: Arc<RemoteConfig>, p: Prepared, follow_up: bool, now: Instant) {
        let msg = remote::OwnedMessage::of(&p);
        let chars = msg.message().counted_chars();
        let day = remote::usage::today();
        if !self.usage.allows(&day, cfg.daily_char_cap, chars) {
            // the cap in force goes with the reason (the badge shows it, whatever it is later)
            let fb = Fallback { provider: cfg.provider, kind: FallbackKind::DailyCap, status: None, message: cfg.daily_char_cap.to_string() };
            self.fall_back(fb, &cfg, 0, 0);
            return;
        }
        self.gen += 1;
        let gen = self.gen;
        for k in self.queue.flight_keys() {
            self.log(EngineEvent::RemoteRequest { req_id: k.req_id, provider: cfg.provider.id(), model: cfg.model_label(), chars, follow_up });
        }
        let policy = self.t.remote;
        let transport = self.deps.transport.clone();
        let tx = self.tx.clone();
        let c2 = cfg.clone();
        let spawned = std::thread::Builder::new().name("tp-engine-remote".into()).spawn(move || {
            let result = remote::call(&c2, &msg.message(), &*transport, policy);
            let _ = tx.send(Msg::Ev(Ev::RemoteDone { gen, result }));
        });
        if spawned.is_err() {
            let fb = Fallback { provider: cfg.provider, kind: FallbackKind::Network, status: None, message: String::new() };
            self.fall_back(fb, &cfg, 0, 0);
            return;
        }
        // The call enforces the total itself; this is only a backstop.
        self.req = Some(Req { gen, deadline: now + policy.total + Duration::from_secs(2), prepared: p, gpu: false, remote: true, sent_chars: chars });
        if !follow_up {
            self.flight_update(St::Translating);
        }
    }

    fn on_remote_result(&mut self, r: Req, result: Result<remote::Answer, remote::CallError>, now: Instant) {
        let Some(Route::Remote(cfg)) = self.route.clone() else { return };
        let (provider, model) = (cfg.provider.id(), cfg.model_label());
        // The daily cap counts what a service answered for: the characters sent and the answer's
        // (an answer the engine cannot use was billed too). A request that got an error status,
        // no connection or no answer in time counts nothing.
        let billed = match &result {
            Ok(a) => Some(r.sent_chars.saturating_add(a.text.chars().count().min(u32::MAX as usize) as u32)),
            Err(e) if matches!(e.fallback.kind, FallbackKind::Refused | FallbackKind::Truncated | FallbackKind::BadAnswer) => Some(r.sent_chars),
            Err(_) => None,
        };
        if let Some(n) = billed {
            self.usage.add(&remote::usage::today(), n);
        }
        match result {
            Ok(a) => {
                let out_chars = a.text.chars().count().min(u32::MAX as usize) as u32;
                for k in self.queue.flight_keys() {
                    self.log(EngineEvent::RemoteDone { req_id: k.req_id, provider, model, status: a.status, ms: a.ms, attempts: a.attempts, out_chars });
                }
                // The same post-processing and at most one follow-up as the local path.
                let fin = pipeline::finish(&r.prepared, &a.text);
                let marked = self.queue.flight().map(|e| e.work.text.as_str().to_string()).unwrap_or_default();
                match pipeline::next(&r.prepared, fin, &marked) {
                    // The Brackets form writes URLs, e-mail addresses and IDs inline: a line that
                    // holds one is not asked again at the service (translated locally instead).
                    Step::Again(p) if p.attempt == Attempt::Brackets && (p.protected.verbatim.iter().any(|v| *v) || p.protected.tokens_in_clear > 0) => {
                        self.keep_local(&cfg);
                    }
                    Step::Again(p) => {
                        for k in self.queue.flight_keys() {
                            self.log(match p.attempt {
                                Attempt::Insist => EngineEvent::InsistRetry { req_id: k.req_id },
                                _ => EngineEvent::PlaceholderRetry { req_id: k.req_id },
                            });
                        }
                        self.send_remote(cfg, p, true, now);
                    }
                    Step::Final(Finish::Done(s)) => {
                        if let Some(ck) = self.flight_cache_key(&cfg.identity()) {
                            self.cache.put(ck, Cached::Done(s.clone()));
                        }
                        self.finish_flight(St::Done(s), now);
                    }
                    Step::Final(Finish::Same) => {
                        if let Some(ck) = self.flight_cache_key(&cfg.identity()) {
                            self.cache.put(ck, Cached::Same);
                        }
                        self.finish_flight(St::Same, now);
                    }
                    Step::Final(_) => {
                        let fb = Fallback { provider: cfg.provider, kind: FallbackKind::BadAnswer, status: None, message: String::new() };
                        self.fall_back(fb, &cfg, a.ms, a.attempts);
                    }
                }
            }
            Err(e) => self.fall_back(e.fallback, &cfg, e.ms, e.attempts),
        }
    }

    /// The remote request passed its backstop deadline (the call should have ended first).
    fn remote_timeout(&mut self) {
        if self.req.take().is_none() {
            return;
        }
        self.gen += 1;
        let Some(Route::Remote(cfg)) = self.route.clone() else { return };
        let fb = Fallback { provider: cfg.provider, kind: FallbackKind::Timeout, status: None, message: String::new() };
        let ms = (self.t.remote.total + Duration::from_secs(2)).as_millis() as u64;
        self.fall_back(fb, &cfg, ms, 1);
    }

    /// The job meant for the service is translated locally from now on (the next `drive_flight`
    /// serves it); every later update carries `fb`.
    fn fall_back(&mut self, fb: Fallback, cfg: &RemoteConfig, ms: u64, attempts: u8) {
        for k in self.queue.flight_keys() {
            self.log(EngineEvent::RemoteFallback {
                req_id: k.req_id,
                provider: cfg.provider.id(),
                model: cfg.model_label(),
                kind: fb.kind.code(),
                status: fb.status.unwrap_or(0),
                ms,
                attempts,
            });
        }
        self.route = Some(Route::FellBack(fb));
    }

    // ---------------------------------------------------------------- server lifecycle

    fn set_killer(&self, k: Option<Killer>) {
        *self.killer.lock().unwrap_or_else(|e| e.into_inner()) = k;
    }

    fn delete_key_file(&mut self) {
        if let Some(k) = self.key_file.take() {
            let _ = std::fs::remove_file(k);
        }
    }

    fn enter_error(&mut self, now: Instant, hold: Duration) {
        self.error_until = Some(now + hold);
        self.log(EngineEvent::Watchdog { action: "error_state" });
    }

    /// Starts a server for the job in flight. Err carries the job's final status.
    fn start_server(&mut self, now: Instant) -> Result<(), St> {
        if !self.installed(now) {
            return Err(St::NotInstalled);
        }
        if let Some(u) = self.error_until {
            if now < u {
                return Err(St::Failed(FailKind::ServerDied));
            }
            self.error_until = None;
        }
        let d = self.discovery.clone().expect("installed");
        let rt = d.runtime.clone().expect("installed");
        let model = d.model.clone().expect("installed");
        if self.features.is_none() {
            match (self.deps.flags)(&rt).and_then(|(f, cached)| f.usable().map(|_| (f, cached))) {
                Ok((f, cached)) => {
                    self.log(EngineEvent::FlagsProbe {
                        cached,
                        lm_none: f.lm_none(),
                        fit: f.has("--fit"),
                        api_key_file: f.has("--api-key-file"),
                        no_ui: f.no_ui_flag().is_some(),
                        offline: f.has("--offline"),
                        dev: f.has("-dev"),
                    });
                    self.features = Some(f);
                }
                Err(reason) => {
                    self.log(EngineEvent::FlagsProbeFailed { reason });
                    self.enter_error(now, self.t.error_retry);
                    return Err(St::Failed(FailKind::ServerDied));
                }
            }
        }
        let f = self.features.clone().expect("probed");
        let snap = self.deps.devices.snapshot();
        let input = policy::Input {
            pref: if self.cpu_once { BackendPref::Cpu } else { self.pref },
            gpu_blocked: self.gpu_blocked,
            adapter: snap.adapter.as_ref().map(device::AdapterInfo::summary),
            free_vram_mb: snap.free_vram_mb,
            avail_ram_mb: snap.avail_ram_mb,
            physical_cores: snap.physical_cores,
            cpu_threads: self.cpu_threads,
            gpu_margin_mb: self.gpu_margin_mb,
            has_vulkan: rt.vulkan,
            has_cuda: rt.cuda.is_some(),
        };
        self.cpu_once = false;
        let dec = policy::choose(&input);
        let (backend, threads) = match dec.choice {
            Choice::Vulkan { threads } => (Backend::Vulkan, threads),
            Choice::Cuda { threads } => (Backend::Cuda, threads),
            Choice::Cpu { threads } => (Backend::Cpu, threads),
            Choice::LowMemory => (Backend::Cpu, 0),
        };
        self.log(EngineEvent::DeviceChoice {
            backend: if dec.choice == Choice::LowMemory { BackendPref::Cpu } else { pref_of(backend) },
            free_vram_mb: snap.free_vram_mb.unwrap_or(0),
            avail_ram_mb: snap.avail_ram_mb,
            threads,
            reason: dec.reason,
        });
        if dec.choice == Choice::LowMemory {
            // Waiting jobs fail too, unless a remote service may still translate them.
            if self.remote.is_none() {
                let mut out = Emit::new();
                self.queue.fail_waiting(St::LowMemory, &mut out);
                self.emit_all(out);
            }
            return Err(St::LowMemory);
        }
        let vulkan_dev = match (backend, &snap.adapter) {
            (Backend::Vulkan, Some(a)) if f.has("-dev") => self.deps.devices.vulkan_device(&rt, &a.name),
            _ => None,
        };
        let Ok(port) = server::free_port() else {
            return self.start_failed(now, backend, "port", None);
        };
        let key = server::new_api_key();
        self.delete_key_file();
        let key_file = self.data_dir.join("run").join(format!("key-{}.txt", std::process::id()));
        let key_file = (|| {
            std::fs::create_dir_all(key_file.parent()?).ok()?;
            std::fs::write(&key_file, format!("{key}\n")).ok()?;
            Some(key_file)
        })();
        self.key_file = key_file.clone();
        let Some(spec) = server::launch_spec(&rt, &model.path, &f, dec.choice, vulkan_dev.as_deref(), port, &key, key_file.as_deref()) else {
            return self.start_failed(now, backend, "spec", None);
        };
        self.log(EngineEvent::ServerStarting {
            backend: pref_of(backend),
            port,
            threads,
            gpu_layers: if backend == Backend::Cpu { 0 } else { server::GPU_LAYERS_ALL },
        });
        let proc = match self.deps.spawner.spawn(&spec) {
            Ok(p) => p,
            Err(reason) => return self.start_failed(now, backend, reason, None),
        };
        self.set_killer(Some(proc.killer()));
        let first_vulkan = backend == Backend::Vulkan && !device::vulkan_marker(&self.data_dir).exists();
        let limit = if first_vulkan { self.t.ready_deadline_first_vulkan } else { self.t.ready_deadline };
        let client = Client::new(proc.port(), &key);
        self.srv = Srv::Starting { proc, client, backend, started: now, deadline: now + limit, next_health: now };
        self.loading_first_gpu = first_vulkan;
        self.flight_update(St::Loading { first_gpu_use: first_vulkan });
        Ok(())
    }

    /// A start failed: GPU falls back to CPU once; CPU enters the Error state (retry after 60 s).
    fn start_failed(&mut self, now: Instant, backend: Backend, reason: &'static str, proc: Option<Box<dyn ServerProc>>) -> Result<(), St> {
        self.log(EngineEvent::ServerStartFailed { backend: pref_of(backend), ms: 0, reason });
        if let Some(mut p) = proc {
            for line in p.ring() {
                self.log(EngineEvent::ServerOutput { line });
            }
            p.kill();
        }
        self.set_killer(None);
        self.delete_key_file();
        if backend != Backend::Cpu {
            self.gpu_failure();
            self.cpu_once = true;
            return self.start_server(now);
        }
        self.enter_error(now, self.t.error_retry);
        Err(St::Failed(FailKind::ServerDied))
    }

    fn gpu_failure(&mut self) {
        self.gpu_failures += 1;
        if self.gpu_failures >= self.t.gpu_failure_limit && !self.gpu_blocked {
            self.gpu_blocked = true;
            self.log(EngineEvent::GpuBlocked { reason: "failures" });
        }
    }

    /// Health polling while starting, and the readiness deadline.
    fn poll_start(&mut self, now: Instant) {
        enum Act {
            Wait,
            Ready(Instant, Instant),
            Fail(&'static str),
        }
        let act = match &mut self.srv {
            Srv::Starting { client, deadline, next_health, started, .. } => {
                if now >= *next_health {
                    *next_health = now + self.t.health_every;
                    if client.health(self.t.health_timeout) == Some(200) {
                        Act::Ready(*started, *deadline)
                    } else if Instant::now() > *deadline {
                        Act::Fail("health_timeout")
                    } else {
                        Act::Wait
                    }
                } else if now > *deadline {
                    Act::Fail("health_timeout")
                } else {
                    Act::Wait
                }
            }
            Srv::Warming { deadline, .. } if now > *deadline => Act::Fail("warmup_timeout"),
            _ => Act::Wait,
        };
        match act {
            Act::Wait => {}
            Act::Ready(started, deadline) => {
                self.log(EngineEvent::ServerReady { ms: Instant::now().duration_since(started).as_millis() as u64 });
                self.begin_warmup(started, deadline);
            }
            Act::Fail(reason) => match std::mem::replace(&mut self.srv, Srv::None) {
                Srv::Starting { proc, backend, .. } | Srv::Warming { proc, backend, .. } => {
                    self.restart_after_start_failure(now, backend, reason, proc);
                }
                other => self.srv = other,
            },
        }
    }

    fn restart_after_start_failure(&mut self, now: Instant, backend: Backend, reason: &'static str, proc: Box<dyn ServerProc>) {
        if let Err(st) = self.start_failed(now, backend, reason, Some(proc)) {
            self.finish_flight(st, now);
        }
    }

    /// One `/completion` with n_predict 1 and the target's system prefix: compiles the Vulkan
    /// shaders and fills the prefix cache.
    fn begin_warmup(&mut self, started: Instant, deadline: Instant) {
        let Srv::Starting { proc, client, backend, .. } = std::mem::replace(&mut self.srv, Srv::None) else { return };
        self.gen += 1;
        let gen = self.gen;
        let target = self.queue.flight().map(|e| e.work.target.clone()).unwrap_or_else(|| "en".into());
        let body = client::completion_body(&prompt::system_prefix(&target, prompt::DEFAULT), 1, None, false);
        let c = client.clone();
        let tx = self.tx.clone();
        let limit = deadline.saturating_duration_since(Instant::now()) + Duration::from_secs(1);
        let _ = std::thread::Builder::new().name("tp-engine-warm".into()).spawn(move || {
            let ok = c.completion(&body, limit, &mut |_| {}).is_ok();
            let _ = tx.send(Msg::Ev(Ev::Warm { gen, ok }));
        });
        self.srv = Srv::Warming { proc, client, backend, started, deadline, gen };
    }

    /// Detects a server that exited on its own.
    fn check_exit(&mut self, now: Instant) {
        let Some(code) = self.srv.proc_mut().and_then(|p| p.exit_code()) else { return };
        match std::mem::replace(&mut self.srv, Srv::None) {
            Srv::Starting { proc, backend, .. } | Srv::Warming { proc, backend, .. } => {
                self.log(EngineEvent::ServerExited { code, expected: false });
                self.restart_after_start_failure(now, backend, "exited", proc);
            }
            Srv::Ready { proc, backend, client } => {
                self.srv = Srv::Ready { proc, backend, client };
                self.server_crashed(now, self.req.as_ref().is_some_and(|r| !r.remote));
            }
            Srv::None => {}
        }
    }

    /// The ready server died (crash, or killed by the watchdog).
    fn server_crashed(&mut self, now: Instant, in_request: bool) {
        let Srv::Ready { mut proc, backend, .. } = std::mem::replace(&mut self.srv, Srv::None) else { return };
        let code = proc.exit_code().unwrap_or(-1);
        self.log(EngineEvent::ServerExited { code, expected: false });
        for line in proc.ring() {
            self.log(EngineEvent::ServerOutput { line });
        }
        proc.kill();
        self.set_killer(None);
        self.delete_key_file();
        self.count_crash(now, backend);
        if in_request {
            self.req = None;
            self.gen += 1;
            self.finish_flight(St::Failed(FailKind::ServerDied), now);
        }
    }

    fn count_crash(&mut self, now: Instant, backend: Backend) {
        if backend != Backend::Cpu {
            self.gpu_failure();
        }
        self.crashes.push_back(now);
        while self.crashes.front().is_some_and(|&t| now.duration_since(t) > self.t.crash_window) {
            self.crashes.pop_front();
        }
        if self.crashes.len() >= self.t.crash_limit {
            self.crashes.clear();
            self.enter_error(now, self.t.crash_hold);
        }
    }

    fn request_timeout(&mut self, now: Instant) {
        let Some(r) = self.req.take() else { return };
        self.gen += 1;
        self.log(EngineEvent::Watchdog { action: "request_timeout" });
        let backend = self.srv.backend().unwrap_or(if r.gpu { Backend::Vulkan } else { Backend::Cpu });
        self.kill_server("timeout");
        self.count_crash(now, backend);
        self.finish_flight(St::Failed(FailKind::Timeout), now);
    }

    fn kill_server(&mut self, reason: &'static str) {
        let had = !matches!(self.srv, Srv::None);
        if let Some(p) = self.srv.proc_mut() {
            p.kill();
        }
        self.srv = Srv::None;
        self.set_killer(None);
        self.delete_key_file();
        if had {
            self.log(EngineEvent::Unload { reason });
        }
    }

    fn housekeeping(&mut self, now: Instant) {
        // A remote request does not use the local server.
        let idle = self.req.as_ref().is_none_or(|r| r.remote);
        if let Some(reason) = self.pending_unload {
            if idle && !matches!(self.srv, Srv::Starting { .. } | Srv::Warming { .. }) {
                self.pending_unload = None;
                self.kill_server(reason);
            }
        }
        if let Srv::Ready { backend, .. } = &self.srv {
            let backend = *backend;
            if idle && self.queue.is_idle() && !self.keep_loaded && now.duration_since(self.last_used) >= self.idle_unload {
                self.kill_server("idle");
                return;
            }
            if idle && self.wow_absent_since.is_some_and(|t| now.duration_since(t) >= self.t.wow_gone_unload) {
                self.kill_server("wow_gone");
                return;
            }
            if backend != Backend::Cpu && now >= self.next_vram {
                self.next_vram = now + self.t.vram_every;
                if let Some(free) = self.deps.devices.free_vram_mb() {
                    if free < self.t.vram_min_mb {
                        self.log(EngineEvent::VramGuard { free_mb: free, switched_to_cpu: true });
                        if !self.gpu_blocked {
                            self.gpu_blocked = true;
                            self.log(EngineEvent::GpuBlocked { reason: "vram" });
                        }
                        // After the request in flight (drive_flight waits for the unload).
                        self.pending_unload = Some("vram");
                    }
                }
            }
        }
    }
}

/// Builds a job (tests).
#[cfg(test)]
pub(crate) fn job(session: u16, req_id: u16, target: &str, text: &str, pairs: &[(&str, &str)], no_cache: bool) -> Job {
    Job {
        key: JobKey { session, req_id },
        target: target.to_string(),
        text: crate::ChatText::new(text.to_string()),
        pairs: pairs.iter().map(|(t, m)| (crate::ChatText::new(t.to_string()), crate::ChatText::new(m.to_string()))).collect(),
        no_cache,
        urgent: false,
    }
}

#[cfg(test)]
mod tests;
