//! E1 prompt evaluation (CPU): runs tests/eval/chat_samples.tsv through llama-server and writes a
//! report of numbers and ids only (design section P3, E1b tuning in docs/ENGINE.md).
//!
//! It uses the engine's own building blocks (discovery, flags probe, launch spec, job object,
//! pipeline with its follow-up rule, grammar, post-processing), so it measures exactly what
//! ships. Each batch of lines gets its own server under a hard watchdog (default 60 s per
//! server); the server is killed after each batch and the run checks that it exited.
//!
//! Usage (from helper\engine):
//!   cargo run --offline --release --bin evalprompt -- --runtime DIR --cpu --threads 4 --out FILE
//! Options: --model FILE, --samples FILE (default: tests\eval\chat_samples.tsv found upward from
//! the current folder), --batch N (6), --watchdog SECS (60), --variant placeholders|brackets,
//! --tuning default|v0|v1|v1+v2+v3+v4... (pipeline::Tuning; default: the shipped one),
//! --gpu (optional pass; refused unless more than 4000 MB of VRAM is free), --show (prints the
//! synthetic outputs to the console for a developer; never written to the report), --show-raw
//! (also the raw model answers of every attempt, console only), --selftest
//! (prints the engine selftest JSON and exits; never loads the model).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tp_engine::client::{self, Client};
use tp_engine::device::{self, Devices, SystemDevices};
use tp_engine::pipeline::{self, Attempt, Finish, Prep, Prepared, Step, Tuning};
use tp_engine::policy::Choice;
use tp_engine::prompt::{Variant, PROMPT_VERSION};
use tp_engine::server::{self, RealSpawner, ServerProc, Spawner};
use tp_bridge::marks;
use tp_engine::{discover, flags, post, protect};

struct Args {
    runtime: Option<PathBuf>,
    model: Option<PathBuf>,
    samples: Option<PathBuf>,
    out: Option<PathBuf>,
    threads: u32,
    batch: usize,
    watchdog: u64,
    gpu: bool,
    variant: Variant,
    tuning: Tuning,
    show: bool,
    show_raw: bool,
    selftest: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        runtime: std::env::var_os("TPROBE_LLAMA_RUNTIME").map(PathBuf::from),
        model: None,
        samples: None,
        out: None,
        threads: 4,
        batch: 6,
        watchdog: 60,
        gpu: false,
        variant: Variant::Placeholders,
        tuning: Tuning::DEFAULT,
        show: false,
        show_raw: false,
        selftest: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut val = || it.next().ok_or(format!("{k} needs a value"));
        match k.as_str() {
            "--runtime" => a.runtime = Some(val()?.into()),
            "--model" => a.model = Some(val()?.into()),
            "--samples" => a.samples = Some(val()?.into()),
            "--out" => a.out = Some(val()?.into()),
            "--threads" => a.threads = val()?.parse().map_err(|_| "bad --threads")?,
            "--batch" => a.batch = val()?.parse().map_err(|_| "bad --batch")?,
            "--watchdog" => a.watchdog = val()?.parse().map_err(|_| "bad --watchdog")?,
            "--variant" => {
                a.variant = match val()?.as_str() {
                    "placeholders" => Variant::Placeholders,
                    "brackets" => Variant::Brackets,
                    _ => return Err("bad --variant".into()),
                }
            }
            "--tuning" => a.tuning = Tuning::parse(&val()?).ok_or("bad --tuning")?,
            "--cpu" => a.gpu = false,
            "--gpu" => a.gpu = true,
            "--show" => a.show = true,
            "--show-raw" => {
                a.show = true;
                a.show_raw = true;
            }
            "--selftest" => a.selftest = true,
            _ => return Err(format!("unknown argument {k}")),
        }
    }
    a.batch = a.batch.clamp(1, 20);
    a.threads = a.threads.clamp(1, 16);
    Ok(a)
}

struct Sample {
    id: String,
    target: String,
    marked: String,
    pairs: Vec<(String, String)>,
    same: bool,
    /// Must-use pairs: (term, folded alternatives; one must appear in the shown text).
    uses: Vec<(String, Vec<String>)>,
    /// Keep-allowed pair terms (no check: the term may stay as typed).
    keeps: Vec<String>,
    /// Official localized names (dungeons, zones, cities): one alternative of each group.
    names: Vec<Vec<String>>,
    /// Content words that show the line was translated: one alternative of each group.
    words: Vec<Vec<String>>,
    /// Tokens the shown text must hold exactly as written (IDs, URLs, e-mail addresses).
    copies: Vec<String>,
    /// Tokens the engine must leave to the model (levels, prices, times written in another
    /// language's shorthand): never a placeholder.
    frees: Vec<String>,
}

/// `\2`, `\3` and `\4` in the corpus stand for the 0x02/0x03 marks and the 0x04 colour prefix.
fn decode_marks(s: &str) -> String {
    s.replace("\\2", "\u{2}").replace("\\3", "\u{3}").replace("\\4", "\u{4}")
}

fn alternatives(s: &str) -> Vec<String> {
    s.split('|').map(fold).filter(|a| !a.is_empty()).collect()
}

/// Expectation grammar (column 5, ";"-separated): `same` or `tr`; `use:<term>=a|b` (must-use
/// pair), `keep:<term>` (keep-allowed pair), `name=a|b` (official name), `word=a|b` (content
/// word), `copy=<token>` (an ID, URL or e-mail address the shown text must hold exactly as
/// written), `free=<token>` (a level, price or time the engine must not protect: the model
/// translates it). Every pair must be classified exactly once.
fn parse_expect(e: &str, smp: &mut Sample) -> Result<(), String> {
    let mut kind = 0;
    let Sample { same, uses, keeps, names, words, copies, frees, pairs, .. } = smp;
    for part in e.split(';') {
        if part == "same" {
            *same = true;
            kind += 1;
        } else if part == "tr" {
            kind += 1;
        } else if let Some(rest) = part.strip_prefix("use:") {
            let (term, alts) = rest.split_once('=').ok_or("use: needs =")?;
            let alts = alternatives(alts);
            if alts.is_empty() {
                return Err("use: without alternatives".into());
            }
            uses.push((term.to_string(), alts));
        } else if let Some(term) = part.strip_prefix("keep:") {
            keeps.push(term.to_string());
        } else if let Some(alts) = part.strip_prefix("name=") {
            names.push(alternatives(alts));
        } else if let Some(alts) = part.strip_prefix("word=") {
            words.push(alternatives(alts));
        } else if let Some(token) = part.strip_prefix("copy=") {
            if token.is_empty() || token.contains(char::is_whitespace) {
                return Err("copy= needs one token".into());
            }
            copies.push(token.to_string());
        } else if let Some(token) = part.strip_prefix("free=") {
            if token.is_empty() || token.contains(char::is_whitespace) {
                return Err("free= needs one token".into());
            }
            frees.push(token.to_string());
        } else {
            return Err(format!("unknown expectation {part:?}"));
        }
    }
    if kind != 1 {
        return Err("needs exactly one of same/tr".into());
    }
    for (t, _) in pairs.iter() {
        let n = uses.iter().filter(|(u, _)| u.eq_ignore_ascii_case(t)).count() + keeps.iter().filter(|k| k.eq_ignore_ascii_case(t)).count();
        if n != 1 {
            return Err(format!("pair {t:?} classified {n} times"));
        }
    }
    for term in uses.iter().map(|(u, _)| u).chain(keeps.iter()) {
        if !pairs.iter().any(|(t, _)| t.eq_ignore_ascii_case(term)) {
            return Err(format!("{term:?} is not a pair"));
        }
    }
    Ok(())
}

fn load_samples(path: &Path) -> Result<Vec<Sample>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read samples: {e}"))?;
    let mut v = Vec::new();
    for (n, line) in text.lines().enumerate() {
        if line.starts_with('#') || line.starts_with("id\t") || line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() != 5 {
            return Err(format!("line {}: {} columns", n + 1, f.len()));
        }
        let pairs: Vec<(String, String)> = serde_json::from_str::<Vec<(String, String)>>(f[3]).map_err(|_| format!("line {}: pairs", n + 1))?;
        let mut smp = Sample { id: f[0].into(), target: f[1].into(), marked: decode_marks(f[2]), pairs, same: false, uses: vec![], keeps: vec![], names: vec![], words: vec![], copies: vec![], frees: vec![] };
        parse_expect(f[4], &mut smp).map_err(|e| format!("line {}: {e}", n + 1))?;
        v.push(smp);
    }
    Ok(v)
}

/// Lowercase, Latin accents removed.
fn fold(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| match c {
            'à' | 'â' | 'ä' | 'á' | 'ã' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' | 'í' => 'i',
            'ô' | 'ö' | 'ó' | 'õ' => 'o',
            'ù' | 'û' | 'ü' | 'ú' => 'u',
            'ç' => 'c',
            'ñ' => 'n',
            'ß' => 's',
            c => c,
        })
        .collect()
}

fn find_upward(rel: &str) -> Option<PathBuf> {
    let mut d = std::env::current_dir().ok()?;
    loop {
        let p = d.join(rel);
        if p.exists() {
            return Some(p);
        }
        if !d.pop() {
            return None;
        }
    }
}

#[derive(Default)]
struct Scripts {
    latin: usize,
    cyrillic: usize,
    han: usize,
    kana: usize,
    hangul: usize,
}

fn scripts(s: &str) -> Scripts {
    let mut k = Scripts::default();
    for c in s.chars().filter(|c| c.is_alphabetic()) {
        let u = c as u32;
        if protect::is_hangul(c) {
            k.hangul += 1;
        } else if (0x3040..=0x30FF).contains(&u) {
            k.kana += 1;
        } else if (0x3400..=0x4DBF).contains(&u) || (0x4E00..=0x9FFF).contains(&u) || (0xF900..=0xFAFF).contains(&u) {
            k.han += 1;
        } else if (0x0400..=0x04FF).contains(&u) {
            k.cyrillic += 1;
        } else if c.is_ascii_alphabetic() || (0x00C0..=0x024F).contains(&u) {
            k.latin += 1;
        }
    }
    k
}

/// The output uses the target's script (original span texts removed first).
fn script_ok(target: &str, output: &str, spans: &[String]) -> bool {
    let mut s = output.to_string();
    for sp in spans {
        s = s.replace(sp.as_str(), " ");
    }
    let k = scripts(&s);
    match target {
        "ru" => k.cyrillic > 0 && k.han + k.kana + k.hangul == 0,
        "zh" | "zh-TW" => k.han > 0 && k.kana + k.hangul + k.cyrillic == 0,
        "ko" => k.hangul > 0 && k.kana + k.han + k.cyrillic == 0,
        _ => k.han + k.kana + k.hangul + k.cyrillic == 0,
    }
}

/// One started server for a batch.
struct Session {
    proc: Box<dyn ServerProc>,
    client: Client,
    gpu: bool,
}

#[allow(clippy::too_many_arguments)]
fn start_session(rt: &discover::Runtime, model: &Path, f: &flags::Features, choice: Choice, vulkan_dev: Option<&str>, data: &Path, deadline: Instant) -> Result<Session, String> {
    let port = server::free_port().map_err(|_| "no port")?;
    let key = server::new_api_key();
    let key_file = data.join(format!("key-{}.txt", std::process::id()));
    std::fs::write(&key_file, format!("{key}\n")).map_err(|_| "key file")?;
    let spec = server::launch_spec(rt, model, f, choice, vulkan_dev, port, &key, Some(&key_file)).ok_or("spec")?;
    let mut proc = RealSpawner.spawn(&spec).map_err(|e| e.to_string())?;
    let client = Client::new(port, &key);
    while Instant::now() < deadline {
        if let Some(code) = proc.exit_code() {
            return Err(format!("server exited early (code {code})"));
        }
        if client.health(Duration::from_millis(500)) == Some(200) {
            return Ok(Session { proc, client, gpu: !matches!(choice, Choice::Cpu { .. }) });
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    proc.kill();
    Err("server not ready before the watchdog".into())
}

fn complete(s: &Session, p: &Prepared) -> Result<String, client::ReqError> {
    let body = client::completion_body(&p.prompt, p.n_predict, Some(p.grammar), false);
    // The engine's own deadlines (D.4): CPU 6 s + n/8 s, GPU 4 s + n/40 s.
    let limit = if s.gpu { 4.0 + p.n_predict as f64 / 40.0 } else { 6.0 + p.n_predict as f64 / 8.0 };
    s.client.completion(&body, Duration::from_secs_f64(limit), &mut |_| {})
}

#[derive(Default)]
struct LineResult {
    outcome: &'static str,
    spans: usize,
    first_valid: Option<bool>,
    brackets_retry: bool,
    insisted: bool,
    restored: Option<bool>,
    /// Lines with `copy=` tokens: every one is in the shown text exactly as written.
    copy_ok: Option<bool>,
    /// Lines with `free=` tokens: none of them is a protected span (the engine's own decision,
    /// checked without the model).
    free_ok: Option<bool>,
    /// Lines that need a translation: the shown text (output, or the source for `Same`) is in
    /// the target's script.
    script_ok: Option<bool>,
    must_ok: Option<bool>,
    names_ok: Option<bool>,
    words_ok: Option<bool>,
    same_ok: Option<bool>,
    /// A line that needs a translation answered `Same`.
    wrong_same: bool,
    /// A line that needs a translation answered `Done` with its own text again.
    unchanged: bool,
    calls: u32,
    ms: u64,
}

fn all_groups(shown: &str, groups: &[Vec<String>]) -> Option<bool> {
    if groups.is_empty() {
        return None;
    }
    let f = fold(shown);
    Some(groups.iter().all(|alts| alts.iter().any(|a| f.contains(a.as_str()))))
}

fn run_line(s: &Session, smp: &Sample, variant: Variant, tuning: Tuning, show: bool, show_raw: bool) -> LineResult {
    let t = Instant::now();
    let refs: Vec<(&str, &str)> = smp.pairs.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let mut r = LineResult::default();
    if !smp.frees.is_empty() {
        let prot = protect::protect(&smp.marked, variant);
        r.free_ok = Some(smp.frees.iter().all(|t| !prot.spans.iter().any(|s| s == t)));
    }
    let source = protect::unmarked(&smp.marked);
    let mut p = match pipeline::prepare_with(&smp.target, &smp.marked, &refs, variant, tuning) {
        Prep::Nothing => {
            r.outcome = "nothing";
            r.same_ok = smp.same.then_some(false);
            return r;
        }
        Prep::Same => {
            r.outcome = "same";
            r.same_ok = Some(smp.same);
            r.wrong_same = !smp.same;
            return r;
        }
        Prep::Ready(p) => p,
    };
    r.spans = p.protected.spans.len();
    // every protected span (item names, and IDs or URLs when the engine protects them)
    let spans = p.protected.spans.clone();
    // the item names: the corpus's marked spans (at most 9 placeholders)
    let names: Vec<String> = marks::split(&smp.marked)
        .into_iter()
        .filter_map(|x| match x {
            marks::Piece::Span { text, .. } => Some(text.trim().to_string()),
            marks::Piece::Plain(_) => None,
        })
        .filter(|t| !t.is_empty())
        .take(protect::MAX_SPANS)
        .collect();
    // The engine's flow: first request, then at most one follow-up (pipeline::next).
    let fin = loop {
        r.calls += 1;
        let raw = match complete(s, &p) {
            Ok(raw) => raw,
            Err(e) => {
                r.outcome = if e == client::ReqError::Timeout { "timeout" } else { "http" };
                r.ms = t.elapsed().as_millis() as u64;
                return r;
            }
        };
        if show_raw {
            println!("  {} raw {:?}: {}", smp.id, p.attempt, raw.trim());
        }
        if p.attempt == Attempt::First && variant == Variant::Placeholders && r.spans > 0 {
            r.first_valid = Some(post::clean(&raw, &p.protected.text).is_some_and(|c| protect::placeholders_valid(&c, r.spans)));
        }
        let fin = pipeline::finish(&p, &raw);
        match pipeline::next(&p, fin, &smp.marked) {
            Step::Again(p2) => {
                match p2.attempt {
                    Attempt::Insist => r.insisted = true,
                    _ => r.brackets_retry = true,
                }
                p = p2;
            }
            Step::Final(f) => break f,
        }
    };
    let shown = match &fin {
        Finish::Done(out) => {
            r.outcome = "done";
            // the text the card shows (link colours removed)
            let out = &marks::plain(out);
            if !names.is_empty() {
                r.restored = Some(names.iter().all(|sp| out.contains(sp.as_str())));
            }
            if smp.same {
                r.same_ok = Some(false);
            } else {
                // Compared without the item names, so a long name cannot hide a change.
                let strip = |s: &str| spans.iter().fold(s.to_string(), |acc, sp| acc.replace(sp.as_str(), " "));
                r.unchanged = post::is_same(&strip(&source), &strip(out));
            }
            Some(out.clone())
        }
        Finish::Same => {
            r.outcome = "same";
            r.same_ok = Some(smp.same);
            r.wrong_same = !smp.same;
            // The card says "Already in ...": the reader keeps the original line.
            Some(source.clone())
        }
        Finish::Retry | Finish::Insist(_) | Finish::Bad => {
            r.outcome = "bad";
            if !names.is_empty() {
                r.restored = Some(false);
            }
            None
        }
    };
    if !smp.copies.is_empty() {
        let text = shown.as_deref().unwrap_or("");
        r.copy_ok = Some(shown.is_some() && smp.copies.iter().all(|c| text.contains(c.as_str())));
    }
    if !smp.same {
        let text = shown.as_deref().unwrap_or("");
        let have = shown.is_some();
        r.script_ok = have.then(|| script_ok(&smp.target, text, &spans));
        let uses: Vec<Vec<String>> = smp.uses.iter().map(|(_, a)| a.clone()).collect();
        r.must_ok = all_groups(text, &uses).map(|ok| ok && have);
        r.names_ok = all_groups(text, &smp.names).map(|ok| ok && have);
        r.words_ok = all_groups(text, &smp.words).map(|ok| ok && have);
    }
    if show {
        let tag = format!("{}{}", if r.insisted { " (insisted)" } else { "" }, if r.brackets_retry { " (brackets)" } else { "" });
        match &fin {
            Finish::Same => println!("  {} -> (same){tag}", smp.id),
            _ => println!("  {} -> {}{tag}", smp.id, shown.as_deref().unwrap_or("(bad)")),
        }
    }
    r.ms = t.elapsed().as_millis() as u64;
    r
}

fn ratio(n: usize, d: usize) -> Value {
    if d == 0 {
        Value::Null
    } else {
        json!((n as f64 / d as f64 * 1000.0).round() / 1000.0)
    }
}

/// ok/of over the lines where `f` gives a verdict.
fn count(results: &[(usize, LineResult)], f: impl Fn(&LineResult) -> Option<bool>) -> (usize, usize) {
    let v: Vec<bool> = results.iter().filter_map(|(_, r)| f(r)).collect();
    (v.iter().filter(|&&b| b).count(), v.len())
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("evalprompt: {e}");
            std::process::exit(2);
        }
    };
    if args.selftest {
        // Same JSON as WoWTranslateCompanion.exe --selftest (never loads the model).
        let data = std::env::temp_dir().join(format!("tp_evalprompt_{}", std::process::id()));
        let cfg = tp_engine::EngineConfig {
            runtime_dir: args.runtime.clone(),
            model_path: args.model.clone(),
            backend: if args.gpu { tp_engine::BackendPref::Auto } else { tp_engine::BackendPref::Cpu },
            gpu_margin_mb: 1500,
            cpu_threads: args.threads,
            idle_unload: Duration::from_secs(600),
            keep_loaded: false,
            translator: tp_engine::Translator::Llama,
            data_dir: data.clone(),
            log: Arc::new(|_| {}),
        };
        println!("{}", serde_json::to_string_pretty(&tp_engine::selftest::run(&cfg)).unwrap_or_default());
        let _ = std::fs::remove_dir_all(&data);
        return;
    }
    let samples_path = args.samples.clone().or_else(|| find_upward("tests/eval/chat_samples.tsv"));
    let Some(samples_path) = samples_path else {
        eprintln!("evalprompt: tests/eval/chat_samples.tsv not found (use --samples)");
        std::process::exit(2);
    };
    let samples = load_samples(&samples_path).unwrap_or_else(|e| {
        eprintln!("evalprompt: {e}");
        std::process::exit(2);
    });
    let disc = discover::discover(args.runtime.as_deref(), args.model.as_deref());
    let (Some(rt), Some(model)) = (disc.runtime.clone(), disc.model.clone()) else {
        eprintln!("evalprompt: runtime found: {}, model found: {}", disc.runtime.is_some(), disc.model.is_some());
        std::process::exit(2);
    };
    let data = std::env::temp_dir().join(format!("tp_evalprompt_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&data);
    let (features, _) = flags::probe(&rt.exe, &rt.dir, &data).unwrap_or_else(|e| {
        eprintln!("evalprompt: flags probe failed: {e}");
        std::process::exit(2);
    });
    let ram = device::avail_ram_mb();
    let mut devs = SystemDevices::default();
    let snap = devs.snapshot();
    let (choice, vulkan_dev) = if args.gpu {
        let free = snap.free_vram_mb.unwrap_or(0);
        if free <= 4000 || !rt.vulkan {
            eprintln!("evalprompt: GPU pass refused (free VRAM {free} MB, vulkan {})", rt.vulkan);
            std::process::exit(2);
        }
        let name = snap.adapter.as_ref().map(|a| a.name.clone()).unwrap_or_default();
        (Choice::Vulkan { threads: args.threads }, devs.vulkan_device(&rt, &name))
    } else {
        if ram < tp_engine::policy::CPU_RAM_MB {
            eprintln!("evalprompt: only {ram} MB of RAM available (needs {})", tp_engine::policy::CPU_RAM_MB);
            std::process::exit(2);
        }
        (Choice::Cpu { threads: args.threads }, None)
    };
    println!(
        "evalprompt: {} lines, batches of {}, {} with {} threads, variant {:?}, tuning {}, watchdog {} s per server",
        samples.len(),
        args.batch,
        if args.gpu { "vulkan" } else { "cpu" },
        args.threads,
        args.variant,
        args.tuning.label(),
        args.watchdog
    );

    let t0 = Instant::now();
    let mut results: Vec<(usize, LineResult)> = Vec::new();
    let mut watchdog_fired = 0;
    let mut start_failures = 0;
    let mut exits_checked = 0;
    for (bi, chunk) in samples.chunks(args.batch).enumerate() {
        let deadline = Instant::now() + Duration::from_secs(args.watchdog);
        let session = match start_session(&rt, &model.path, &features, choice, vulkan_dev.as_deref(), &data, deadline) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("evalprompt: batch {bi}: {e}");
                start_failures += 1;
                for (i, _) in chunk.iter().enumerate() {
                    results.push((bi * args.batch + i, LineResult { outcome: "no_server", ..Default::default() }));
                }
                continue;
            }
        };
        // Hard watchdog: kills the server when this batch outlives its budget.
        let killer = session.proc.killer();
        let done = Arc::new(AtomicBool::new(false));
        let fired = Arc::new(AtomicBool::new(false));
        let (d2, f2) = (done.clone(), fired.clone());
        let wd = std::thread::spawn(move || {
            while Instant::now() < deadline {
                if d2.load(Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            f2.store(true, Ordering::SeqCst);
            killer();
        });
        for (i, smp) in chunk.iter().enumerate() {
            let r = if fired.load(Ordering::SeqCst) { LineResult { outcome: "watchdog", ..Default::default() } } else { run_line(&session, smp, args.variant, args.tuning, args.show, args.show_raw) };
            let notes = format!("{}{}", if r.insisted { "(insisted)" } else { "" }, if r.brackets_retry { "(brackets)" } else { "" });
            println!("  {:5} {:6} {:5} ms {}", smp.id, r.outcome, r.ms, notes);
            results.push((bi * args.batch + i, r));
        }
        done.store(true, Ordering::SeqCst);
        let _ = wd.join();
        if fired.load(Ordering::SeqCst) {
            watchdog_fired += 1;
        }
        let mut proc = session.proc;
        proc.kill();
        if proc.exit_code().is_some() {
            exits_checked += 1;
        }
    }
    let _ = std::fs::remove_dir_all(&data);

    // Metrics.
    let (script_good, script_n) = count(&results, |r| r.script_ok);
    let (fp_good, fp_n) = count(&results, |r| r.first_valid);
    let (rest_good, rest_n) = count(&results, |r| r.restored);
    let (copy_good, copy_n) = count(&results, |r| r.copy_ok);
    let (free_good, free_n) = count(&results, |r| r.free_ok);
    let (must_good, must_n) = count(&results, |r| r.must_ok);
    let (names_good, names_n) = count(&results, |r| r.names_ok);
    let (words_good, words_n) = count(&results, |r| r.words_ok);
    let same_lines = samples.iter().filter(|s| s.same).count();
    let same_good = results.iter().filter(|(i, r)| samples[*i].same && r.same_ok == Some(true)).count();
    let wrong_same = results.iter().filter(|(_, r)| r.wrong_same).count();
    let unchanged = results.iter().filter(|(_, r)| r.unchanged).count();
    let failed = results.iter().filter(|(_, r)| !matches!(r.outcome, "done" | "same" | "nothing")).count();
    let lines_with_pairs = samples.iter().filter(|s| !s.pairs.is_empty()).count();

    let mut reasons = Vec::new();
    if script_good != script_n {
        reasons.push(format!("output script correct {script_good}/{script_n} (rule 100%)"));
    }
    // A rule with no line to judge (a set without item names or must-use pairs) passes.
    if fp_n > 0 && (fp_good as f64) < 0.9 * fp_n as f64 {
        reasons.push(format!("first-pass placeholder validity {fp_good}/{fp_n} (rule >= 90%)"));
    }
    if rest_good != rest_n {
        reasons.push(format!("item names restored {rest_good}/{rest_n} (rule 100%)"));
    }
    if copy_good != copy_n {
        reasons.push(format!("IDs, URLs and addresses kept {copy_good}/{copy_n} lines (rule 100%)"));
    }
    if free_good != free_n {
        reasons.push(format!("levels, prices and times left to the model {free_good}/{free_n} lines (rule 100%)"));
    }
    if must_n > 0 && (must_good as f64) < 0.8 * must_n as f64 {
        reasons.push(format!("must-use glossary meaning present {must_good}/{must_n} lines (rule >= 80%)"));
    }
    if wrong_same > 0 {
        reasons.push(format!("{wrong_same} lines answered Same although they needed a translation (rule 0)"));
    }
    if same_good != same_lines {
        reasons.push(format!("same detected {same_good}/{same_lines} (rule all)"));
    }
    if failed > 0 {
        reasons.push(format!("{failed} lines without a result"));
    }
    let pass = reasons.is_empty() && args.variant == Variant::Placeholders;
    let ids = |f: &dyn Fn(&LineResult) -> bool| -> Vec<String> { results.iter().filter(|(_, r)| f(r)).map(|(i, _)| samples[*i].id.clone()).collect() };

    let ms: Vec<u64> = results.iter().map(|(_, r)| r.ms).filter(|&m| m > 0).collect();
    let ms_mean = if ms.is_empty() { 0 } else { ms.iter().sum::<u64>() / ms.len() as u64 };
    let ms_max = ms.iter().copied().max().unwrap_or(0);
    let calls: u32 = results.iter().map(|(_, r)| r.calls).sum();
    let report = json!({
        "promptVersion": PROMPT_VERSION,
        "variantTested": format!("{:?}", args.variant),
        "tuning": args.tuning.label(),
        "backend": if args.gpu { "vulkan" } else { "cpu" },
        "threads": args.threads,
        "lines": samples.len(),
        "linesWithPairs": lines_with_pairs,
        "linesWithMustUsePairs": samples.iter().filter(|s| !s.uses.is_empty()).count(),
        "batches": samples.len().div_ceil(args.batch),
        "totalSeconds": (t0.elapsed().as_secs_f64() * 10.0).round() / 10.0,
        "serverStartFailures": start_failures,
        "watchdogFired": watchdog_fired,
        "serversKilledAndExited": exits_checked,
        "metrics": {
            "scriptCorrect": {"ok": script_good, "of": script_n, "ratio": ratio(script_good, script_n)},
            "firstPassPlaceholders": {"ok": fp_good, "of": fp_n, "ratio": ratio(fp_good, fp_n)},
            "itemNamesRestored": {"ok": rest_good, "of": rest_n, "ratio": ratio(rest_good, rest_n)},
            "verbatimKept": {"ok": copy_good, "of": copy_n, "ratio": ratio(copy_good, copy_n)},
            "freeTokens": {"ok": free_good, "of": free_n, "ratio": ratio(free_good, free_n)},
            "mustUseGlossary": {"ok": must_good, "of": must_n, "ratio": ratio(must_good, must_n)},
            "officialNames": {"ok": names_good, "of": names_n, "ratio": ratio(names_good, names_n)},
            "contentWords": {"ok": words_good, "of": words_n, "ratio": ratio(words_good, words_n)},
            "sameDetected": {"ok": same_good, "of": same_lines},
            "wrongSame": wrong_same,
            "unchangedShown": unchanged,
            "bracketsRetries": results.iter().filter(|(_, r)| r.brackets_retry).count(),
            "insistRetries": results.iter().filter(|(_, r)| r.insisted).count(),
            "modelCalls": calls,
            "failedLines": failed,
            "msPerModelLine": {"mean": ms_mean, "max": ms_max},
        },
        "failingIds": {
            "script": ids(&|r| r.script_ok == Some(false)),
            "firstPassPlaceholders": ids(&|r| r.first_valid == Some(false)),
            "restored": ids(&|r| r.restored == Some(false)),
            "verbatim": ids(&|r| r.copy_ok == Some(false)),
            "free": ids(&|r| r.free_ok == Some(false)),
            "mustUse": ids(&|r| r.must_ok == Some(false)),
            "names": ids(&|r| r.names_ok == Some(false)),
            "words": ids(&|r| r.words_ok == Some(false)),
            "sameMissed": results.iter().filter(|(i, r)| samples[*i].same && r.same_ok != Some(true)).map(|(i, _)| samples[*i].id.clone()).collect::<Vec<_>>(),
            "wrongSame": ids(&|r| r.wrong_same),
            "unchangedShown": ids(&|r| r.unchanged),
            "noResult": ids(&|r| !matches!(r.outcome, "done" | "same" | "nothing")),
        },
        "perLine": results.iter().map(|(i, r)| json!({
            "id": samples[*i].id, "target": samples[*i].target, "outcome": r.outcome, "spans": r.spans,
            "firstPassValid": r.first_valid, "bracketsRetry": r.brackets_retry, "insisted": r.insisted,
            "restored": r.restored, "verbatimOk": r.copy_ok, "freeOk": r.free_ok, "scriptOk": r.script_ok, "mustUseOk": r.must_ok, "namesOk": r.names_ok,
            "wordsOk": r.words_ok, "sameOk": r.same_ok, "wrongSame": r.wrong_same, "unchanged": r.unchanged,
            "calls": r.calls, "ms": r.ms,
        })).collect::<Vec<_>>(),
        "rule": "E1b + L4: script 100%, first-pass placeholders >= 90%, item names restored 100%, IDs/URLs/addresses kept 100%, levels/prices/times left to the model 100%, must-use glossary >= 80%, wrong Same 0, same detected on every same line, no line without a result",
        "pass": pass,
        "reasons": reasons,
    });
    let text = serde_json::to_string_pretty(&report).unwrap_or_default();
    let out = args.out.clone().unwrap_or_else(|| samples_path.parent().unwrap_or(Path::new(".")).join("out").join("report.json"));
    if let Some(dir) = out.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&out, &text) {
        eprintln!("evalprompt: cannot write report: {e}");
    }
    println!("{}", serde_json::to_string_pretty(&report["metrics"]).unwrap_or_default());
    println!("{}", serde_json::to_string_pretty(&report["failingIds"]).unwrap_or_default());
    println!("pass: {} {:?}", report["pass"], report["reasons"]);
    println!("report: {}", out.display());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus() -> Vec<Sample> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/eval/chat_samples.tsv");
        load_samples(&p).unwrap()
    }

    fn blank(pairs: &[(&str, &str)]) -> Sample {
        let pairs = pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        Sample { id: "x".into(), target: "fr".into(), marked: String::new(), pairs, same: false, uses: vec![], keeps: vec![], names: vec![], words: vec![], copies: vec![], frees: vec![] }
    }

    #[test]
    fn samples_file_parses() {
        let s = corpus();
        assert_eq!(s.len(), 59);
        assert_eq!(s.iter().filter(|x| x.same).count(), 5);
        assert!(s.iter().any(|x| x.marked.contains('\u{2}')));
        assert!(s.iter().filter(|x| !x.uses.is_empty()).count() >= 8, "enough must-use lines");
        assert!(s.iter().filter(|x| x.same).all(|x| x.uses.is_empty() && x.names.is_empty() && x.words.is_empty() && x.copies.is_empty()));
        // L4: 15 lines with IDs, URLs and addresses (2 long Chinese yells, a full-width ID); one
        // coloured link; 4 lines with levels, prices and times the engine must not protect
        assert_eq!(s.iter().filter(|x| !x.copies.is_empty()).count(), 15);
        assert_eq!(s.iter().map(|x| x.copies.len()).sum::<usize>(), 23);
        assert_eq!(s.iter().filter(|x| !x.frees.is_empty()).count(), 4);
        let de07 = s.iter().find(|x| x.id == "de07").unwrap();
        assert!(de07.marked.contains("\u{2}\u{4}FFFFFF[Linen Cloth]\u{3}"));
        let mut ids: Vec<&str> = s.iter().map(|x| x.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 59);
    }

    #[test]
    fn echo_set_parses() {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/eval/echo_samples.tsv");
        let s = load_samples(&p).unwrap();
        assert_eq!(s.len(), 14);
        assert!(s.iter().all(|x| !x.same));
    }

    #[test]
    fn corpus_policy_matches_the_engine() {
        // The corpus marks each pair must-use or keep-allowed; the engine decides the same way
        // (prompt::keep_allowed), so V2 and V4 act on exactly the pairs E1b checks.
        let echo = load_samples(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/eval/echo_samples.tsv")).unwrap();
        for smp in corpus().into_iter().chain(echo) {
            for (t, _) in &smp.uses {
                assert!(tp_engine::prompt::must_use(t), "{}: {t} marked must-use", smp.id);
            }
            for t in &smp.keeps {
                assert!(tp_engine::prompt::keep_allowed(t), "{}: {t} marked keep-allowed", smp.id);
            }
        }
    }

    #[test]
    fn expectation_grammar() {
        let pairs = [("wts", "vend"), ("cheap", "pas cher")];
        let mut smp = blank(&pairs);
        parse_expect("tr;keep:wts;use:cheap=pas cher|Bon Marché;name=Mortemines;word=a|b;copy=Abc_12;copy=x.example.com/Q", &mut smp).unwrap();
        assert_eq!(smp.uses, vec![("cheap".to_string(), vec!["pas cher".to_string(), "bon marche".to_string()])]);
        assert_eq!(smp.keeps, vec!["wts".to_string()]);
        assert_eq!(smp.names, vec![vec!["mortemines".to_string()]]);
        assert_eq!(smp.words.len(), 1);
        // copy= is exact (case kept, not folded)
        assert_eq!(smp.copies, vec!["Abc_12".to_string(), "x.example.com/Q".to_string()]);
        assert!(parse_expect("tr;keep:wts;use:cheap=x;copy=", &mut blank(&pairs)).is_err());
        let mut f = blank(&pairs);
        parse_expect("tr;keep:wts;use:cheap=x;free=60лвл;free=2g50", &mut f).unwrap();
        assert_eq!(f.frees, vec!["60лвл".to_string(), "2g50".to_string()]);
        assert!(parse_expect("tr;keep:wts;use:cheap=x;free=", &mut blank(&pairs)).is_err());
        assert_eq!(decode_marks("a\\2\\4FF0000b\\3"), "a\u{2}\u{4}FF0000b\u{3}");
        let bad = [
            "tr;keep:wts",
            "tr;keep:wts;keep:cheap;use:cheap=x",
            "keep:wts;use:cheap=x",
            "tr;same;keep:wts;use:cheap=x",
            "tr;keep:wts;use:cheap=",
            "tr;keep:wts;use:cheap=x;keep:lfm",
            "tr;keep:wts;use:cheap=x;gloss=y",
        ];
        for e in bad {
            assert!(parse_expect(e, &mut blank(&pairs)).is_err(), "{e}");
        }
    }

    #[test]
    fn script_checks() {
        assert!(script_ok("fr", "Vends [Linen Cloth] pas cher", &[]));
        assert!(!script_ok("fr", "Vends 布 pas cher", &[]));
        assert!(script_ok("zh", "出售[Linen Cloth]，便宜", &["[Linen Cloth]".into()]));
        assert!(!script_ok("zh", "selling cheap", &[]));
        assert!(script_ok("ru", "Ищу группу в [Molten Core]", &["[Molten Core]".into()]));
        assert!(script_ok("ko", "탱커 구해요", &[]));
        assert!(!script_ok("en", "Ищу группу", &[]));
        assert_eq!(fold("Félicitations À"), "felicitations a");
        assert_eq!(all_groups("Je reviens vite", &[vec!["reviens".into()]]), Some(true));
        assert_eq!(all_groups("brb", &[vec!["reviens".into()]]), Some(false));
        assert_eq!(all_groups("x", &[]), None);
    }
}
